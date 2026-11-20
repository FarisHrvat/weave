//! Cryptomator vaults: create (format 8, SIV_GCM), and unlock, browse, add,
//! decrypt, rename and delete in formats 7 and 8 with either SIV_GCM or the
//! older SIV_CTRMAC (AES-CTR with HMAC-SHA256) file encryption. The vault folder can live anywhere on
//! this Mac, including folders synced by iCloud Drive, Dropbox or OneDrive.

use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng, Payload},
    Aes256Gcm, Nonce,
};
use aes_kw::KekAes256;
use aes_siv::siv::Aes256Siv;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

const CHUNK_SIZE: usize = 32 * 1024;
const DEFAULT_THRESHOLD: usize = 220;

type Ctr = ctr::Ctr128BE<aes::Aes256>;
type Key = Zeroizing<[u8; 32]>;

/// How file contents are encrypted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cipher {
    /// AES-GCM chunks (format 8 default).
    Gcm,
    /// AES-CTR with HMAC-SHA256 (format 7, and format 8 vaults made before 1.6).
    CtrMac,
}

impl Cipher {
    fn header_size(self) -> usize {
        match self {
            Self::Gcm => 68,
            Self::CtrMac => 88,
        }
    }

    fn nonce_size(self) -> usize {
        match self {
            Self::Gcm => 12,
            Self::CtrMac => 16,
        }
    }

    fn overhead(self) -> usize {
        match self {
            Self::Gcm => 28,
            Self::CtrMac => 48,
        }
    }
}

fn ctr_apply(key: &[u8], nonce: &[u8], data: &mut [u8]) {
    use ctr::cipher::{KeyIvInit, StreamCipher};
    Ctr::new(key.into(), nonce.into()).apply_keystream(data);
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MasterkeyFile {
    version: u32,
    scrypt_salt: String,
    scrypt_cost_param: u32,
    scrypt_block_size: u32,
    primary_master_key: String,
    hmac_master_key: String,
    version_mac: String,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VaultEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

pub struct Vault {
    root: PathBuf,
    enc_key: Zeroizing<[u8; 32]>,
    mac_key: Zeroizing<[u8; 32]>,
    threshold: usize,
    cipher: Cipher,
}

fn decode_any(text: &str) -> Result<Vec<u8>, String> {
    let trimmed = text.trim().trim_end_matches('=');
    URL_SAFE_NO_PAD
        .decode(trimmed)
        .or_else(|_| STANDARD.decode(text.trim()))
        .map_err(|_| "The vault configuration is damaged.".into())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn kek(
    passphrase: &str,
    salt: &[u8],
    cost: u32,
    block: u32,
) -> Result<Zeroizing<[u8; 32]>, String> {
    if !cost.is_power_of_two() || !(1024..=1 << 20).contains(&cost) || !(1..=16).contains(&block) {
        return Err("The vault uses unsupported key settings.".into());
    }
    let params = scrypt::Params::new(cost.trailing_zeros() as u8, block, 1, 32)
        .map_err(|_| "The vault uses unsupported key settings.")?;
    let mut key = Zeroizing::new([0_u8; 32]);
    scrypt::scrypt(
        passphrase.nfc().collect::<String>().as_bytes(),
        salt,
        &params,
        key.as_mut(),
    )
    .map_err(|_| "Could not derive the vault key.")?;
    Ok(key)
}

fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let mut buffer = 0_u32;
    let mut bits = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            out.push(ALPHABET[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0_u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn check_name(name: &str) -> Result<String, String> {
    let name: String = name.nfc().collect();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err(format!("“{name}” is not a valid file name."));
    }
    Ok(name)
}

fn components(path: &str) -> Result<Vec<String>, String> {
    path.split('/')
        .filter(|part| !part.is_empty())
        .map(check_name)
        .collect()
}

impl Vault {
    fn siv(&self) -> Aes256Siv {
        let mut key = Zeroizing::new([0_u8; 64]);
        key[..32].copy_from_slice(self.mac_key.as_ref());
        key[32..].copy_from_slice(self.enc_key.as_ref());
        Aes256Siv::new_from_slice(key.as_ref()).expect("64-byte SIV key")
    }

    fn raw_key(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new([self.enc_key.as_slice(), self.mac_key.as_slice()].concat())
    }

    pub fn create(folder: &Path, passphrase: &str) -> Result<Vault, String> {
        if passphrase.chars().count() < 8 {
            return Err("Use a vault password of at least 8 characters.".into());
        }
        fs::create_dir_all(folder)
            .map_err(|error| format!("Could not create the vault folder: {error}"))?;
        if fs::read_dir(folder)
            .map_err(|error| error.to_string())?
            .next()
            .is_some()
        {
            return Err("Choose an empty folder for the new vault.".into());
        }
        let vault = Vault {
            root: folder.to_path_buf(),
            enc_key: Zeroizing::new(random()),
            mac_key: Zeroizing::new(random()),
            threshold: DEFAULT_THRESHOLD,
            cipher: Cipher::Gcm,
        };
        let salt: [u8; 8] = random();
        let (cost, block) = (32768, 8);
        let key = kek(passphrase, &salt, cost, block)?;
        let kek = KekAes256::from(*key);
        let wrap = |data: &[u8]| -> Result<String, String> {
            Ok(STANDARD.encode(
                kek.wrap_vec(data)
                    .map_err(|_| "Could not protect the vault keys.")?,
            ))
        };
        let master = MasterkeyFile {
            version: 999,
            scrypt_salt: STANDARD.encode(salt),
            scrypt_cost_param: cost,
            scrypt_block_size: block,
            primary_master_key: wrap(vault.enc_key.as_ref())?,
            hmac_master_key: wrap(vault.mac_key.as_ref())?,
            version_mac: STANDARD
                .encode(hmac_sha256(vault.mac_key.as_ref(), &999_u32.to_be_bytes())),
        };
        let header = URL_SAFE_NO_PAD
            .encode(r#"{"kid":"masterkeyfile:masterkey.cryptomator","typ":"JWT","alg":"HS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(
            r#"{{"format":8,"shorteningThreshold":220,"jti":"{}","cipherCombo":"SIV_GCM"}}"#,
            uuid::Uuid::new_v4()
        ));
        let signed = format!("{header}.{payload}");
        let signature = URL_SAFE_NO_PAD.encode(hmac_sha256(&vault.raw_key(), signed.as_bytes()));
        let write = |name: &str, text: &str| {
            fs::write(folder.join(name), text)
                .map_err(|error| format!("Could not write {name}: {error}"))
        };
        write(
            "masterkey.cryptomator",
            &serde_json::to_string_pretty(&master).map_err(|error| error.to_string())?,
        )?;
        write("vault.cryptomator", &format!("{signed}.{signature}"))?;
        let root = vault.dir_path("");
        fs::create_dir_all(&root)
            .map_err(|error| format!("Could not create the vault: {error}"))?;
        vault.write_encrypted(&root.join("dirid.c9r"), &mut &b""[..])?;
        Ok(vault)
    }

    /// Reads the master keys from `key_file`, returning them with its version.
    fn master_keys(
        folder: &Path,
        key_file: &str,
        passphrase: &str,
    ) -> Result<(Key, Key, MasterkeyFile), String> {
        let master: MasterkeyFile = serde_json::from_str(
            &fs::read_to_string(folder.join(key_file))
                .map_err(|_| format!("The vault is missing {key_file}."))?,
        )
        .map_err(|_| format!("{key_file} is damaged."))?;
        let key = kek(
            passphrase,
            &decode_any(&master.scrypt_salt)?,
            master.scrypt_cost_param,
            master.scrypt_block_size,
        )?;
        let kek = KekAes256::from(*key);
        let unwrap = |text: &str| -> Result<Zeroizing<[u8; 32]>, String> {
            let bytes = kek
                .unwrap_vec(&decode_any(text)?)
                .map_err(|_| "Wrong vault password.".to_string())?;
            let mut key = Zeroizing::new([0_u8; 32]);
            key.copy_from_slice(bytes.get(..32).ok_or("The vault keys are damaged.")?);
            Ok(key)
        };
        let enc = unwrap(&master.primary_master_key)?;
        let mac = unwrap(&master.hmac_master_key)?;
        Ok((enc, mac, master))
    }

    pub fn unlock(folder: &Path, passphrase: &str) -> Result<Vault, String> {
        let vault = match fs::read_to_string(folder.join("vault.cryptomator")) {
            Ok(config) => Self::unlock_format8(folder, &config, passphrase)?,
            Err(_) if folder.join("masterkey.cryptomator").is_file() => {
                Self::unlock_format7(folder, passphrase)?
            }
            Err(_) => {
                return Err("This folder is not a Cryptomator vault. Choose the folder that contains vault.cryptomator or masterkey.cryptomator.".into())
            }
        };
        if !vault.dir_path("").is_dir() {
            return Err(
                "The vault's root folder is missing. The vault may be incomplete or still syncing."
                    .into(),
            );
        }
        Ok(vault)
    }

    /// Format 7 keeps its settings in masterkey.cryptomator and uses SIV_CTRMAC.
    fn unlock_format7(folder: &Path, passphrase: &str) -> Result<Vault, String> {
        let (enc_key, mac_key, master) =
            Self::master_keys(folder, "masterkey.cryptomator", passphrase)?;
        match master.version {
            7 => {}
            0..=6 => {
                return Err(format!(
                    "This vault uses Cryptomator format {}, which Weave cannot open. Open it once in Cryptomator 1.5 or newer to upgrade it.",
                    master.version
                ))
            }
            _ => return Err("The vault is missing vault.cryptomator.".into()),
        }
        let expected = hmac_sha256(mac_key.as_ref(), &master.version.to_be_bytes());
        if decode_any(&master.version_mac)? != expected {
            return Err(
                "masterkey.cryptomator does not match its keys. It may have been modified.".into(),
            );
        }
        Ok(Vault {
            root: folder.to_path_buf(),
            enc_key,
            mac_key,
            threshold: DEFAULT_THRESHOLD,
            cipher: Cipher::CtrMac,
        })
    }

    fn unlock_format8(folder: &Path, config: &str, passphrase: &str) -> Result<Vault, String> {
        let parts: Vec<&str> = config.trim().split('.').collect();
        let [header, payload, signature] = parts.as_slice() else {
            return Err("The vault configuration is damaged.".into());
        };
        let header_json: serde_json::Value = serde_json::from_slice(&decode_any(header)?)
            .map_err(|_| "The vault configuration is damaged.")?;
        let payload_json: serde_json::Value = serde_json::from_slice(&decode_any(payload)?)
            .map_err(|_| "The vault configuration is damaged.")?;
        if payload_json["format"] != 8 {
            return Err("This vault uses a Cryptomator format Weave does not know. Update Weave, or check the vault in Cryptomator.".into());
        }
        let cipher = match payload_json["cipherCombo"].as_str() {
            Some("SIV_GCM") => Cipher::Gcm,
            Some("SIV_CTRMAC") => Cipher::CtrMac,
            _ => return Err("This vault uses an encryption method Weave does not support.".into()),
        };
        if header_json["alg"] != "HS256" {
            return Err(
                "This vault's configuration is signed in a way Weave does not support.".into(),
            );
        }
        let key_file = header_json["kid"]
            .as_str()
            .and_then(|kid| kid.strip_prefix("masterkeyfile:"))
            .filter(|name| !name.contains('/') && !name.starts_with('.'))
            .ok_or("This vault uses a key source Weave does not support (for example Cryptomator Hub).")?;
        let (enc_key, mac_key, _) = Self::master_keys(folder, key_file, passphrase)?;
        let vault = Vault {
            root: folder.to_path_buf(),
            enc_key,
            mac_key,
            threshold: payload_json["shorteningThreshold"].as_u64().unwrap_or(220) as usize,
            cipher,
        };
        let expected = hmac_sha256(&vault.raw_key(), format!("{header}.{payload}").as_bytes());
        if decode_any(signature)? != expected {
            return Err(
                "The vault configuration does not match its keys. It may have been modified."
                    .into(),
            );
        }
        Ok(vault)
    }

    pub fn change_password(&self, old: &str, new: &str) -> Result<(), String> {
        Vault::unlock(&self.root, old)?;
        if new.chars().count() < 8 {
            return Err("Use a vault password of at least 8 characters.".into());
        }
        let path = self.root.join("masterkey.cryptomator");
        let mut master: MasterkeyFile =
            serde_json::from_str(&fs::read_to_string(&path).map_err(|e| e.to_string())?)
                .map_err(|_| "masterkey.cryptomator is damaged.")?;
        let salt: [u8; 8] = random();
        let key = kek(
            new,
            &salt,
            master.scrypt_cost_param,
            master.scrypt_block_size,
        )?;
        let kek = KekAes256::from(*key);
        master.scrypt_salt = STANDARD.encode(salt);
        master.primary_master_key = STANDARD.encode(
            kek.wrap_vec(self.enc_key.as_ref())
                .map_err(|_| "Could not protect the vault keys.")?,
        );
        master.hmac_master_key = STANDARD.encode(
            kek.wrap_vec(self.mac_key.as_ref())
                .map_err(|_| "Could not protect the vault keys.")?,
        );
        let temporary = path.with_extension("cryptomator.weave-tmp");
        fs::write(
            &temporary,
            serde_json::to_string_pretty(&master).map_err(|e| e.to_string())?,
        )
        .and_then(|()| fs::rename(&temporary, &path))
        .map_err(|error| format!("Could not save the new password: {error}"))
    }

    fn dir_path(&self, dir_id: &str) -> PathBuf {
        let encrypted = self
            .siv()
            .encrypt(std::iter::empty::<&[u8]>(), dir_id.as_bytes())
            .expect("SIV encryption");
        let hash = base32(&Sha1::digest(encrypted));
        self.root.join("d").join(&hash[..2]).join(&hash[2..])
    }

    fn encrypt_name(&self, dir_id: &str, name: &str) -> String {
        let encrypted = self
            .siv()
            .encrypt([dir_id.as_bytes()], name.as_bytes())
            .expect("SIV encryption");
        format!("{}.c9r", URL_SAFE.encode(encrypted))
    }

    fn decrypt_name(&self, dir_id: &str, encrypted: &str) -> Option<String> {
        let bytes = decode_any(encrypted.strip_suffix(".c9r")?).ok()?;
        let clear = self.siv().decrypt([dir_id.as_bytes()], &bytes).ok()?;
        String::from_utf8(clear).ok()
    }

    /// The on-disk node for `name` in a directory: `(path, is_shortened)`.
    fn node_path(&self, dir_id: &str, name: &str) -> (PathBuf, String) {
        let encrypted = self.encrypt_name(dir_id, name);
        let folder = self.dir_path(dir_id);
        if encrypted.len() > self.threshold {
            let short = format!(
                "{}.c9s",
                URL_SAFE.encode(Sha1::digest(encrypted.as_bytes()))
            );
            (folder.join(short), encrypted)
        } else {
            (folder.join(&encrypted), encrypted)
        }
    }

    fn is_shortened(path: &Path) -> bool {
        path.extension().is_some_and(|ext| ext == "c9s")
    }

    fn file_location(node: &Path) -> PathBuf {
        if Self::is_shortened(node) {
            node.join("contents.c9r")
        } else {
            node.to_path_buf()
        }
    }

    fn child_dir_id(node: &Path) -> Option<String> {
        let file = node.join("dir.c9r");
        let text = fs::read_to_string(file).ok()?;
        (text.len() <= 64).then(|| text.trim().to_string())
    }

    fn dir_id(&self, path: &str) -> Result<String, String> {
        let mut dir_id = String::new();
        for name in components(path)? {
            let (node, _) = self.node_path(&dir_id, &name);
            dir_id = Self::child_dir_id(&node)
                .ok_or_else(|| format!("The folder “{name}” does not exist in this vault."))?;
        }
        Ok(dir_id)
    }

    fn split(path: &str) -> Result<(String, String), String> {
        let mut parts = components(path)?;
        let name = parts.pop().ok_or("Choose an item inside the vault.")?;
        Ok((parts.join("/"), name))
    }

    pub fn list(&self, path: &str) -> Result<Vec<VaultEntry>, String> {
        let dir_id = self.dir_id(path)?;
        let folder = self.dir_path(&dir_id);
        let mut entries = Vec::new();
        for item in fs::read_dir(&folder)
            .map_err(|error| format!("Could not read the vault folder: {error}"))?
        {
            let item = item.map_err(|error| error.to_string())?;
            let file_name = item.file_name().to_string_lossy().to_string();
            let node = item.path();
            let encrypted = if Self::is_shortened(&node) {
                match fs::read_to_string(node.join("name.c9s")) {
                    Ok(name) => name.trim().to_string(),
                    Err(_) => continue,
                }
            } else if file_name.ends_with(".c9r") && file_name != "dirid.c9r" {
                file_name
            } else {
                continue;
            };
            let Some(name) = self.decrypt_name(&dir_id, &encrypted) else {
                continue;
            };
            if node.join("symlink.c9r").exists() {
                continue;
            }
            let is_dir = node.join("dir.c9r").is_file();
            let size = if is_dir {
                0
            } else {
                fs::metadata(Self::file_location(&node))
                    .map(|meta| cleartext_size(self.cipher, meta.len()))
                    .unwrap_or(0)
            };
            // Names are authenticated, but anyone sharing the vault holds the key.
            if !crate::transfer::safe_entry_name(&name) {
                continue;
            }
            entries.push(VaultEntry { name, is_dir, size });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    fn prepare_node(&self, parent_id: &str, name: &str) -> Result<PathBuf, String> {
        let (node, encrypted) = self.node_path(parent_id, name);
        if node.exists() {
            return Err(format!("“{name}” already exists in this folder."));
        }
        if Self::is_shortened(&node) {
            fs::create_dir(&node).map_err(|error| error.to_string())?;
            fs::write(node.join("name.c9s"), encrypted).map_err(|error| error.to_string())?;
        }
        Ok(node)
    }

    pub fn mkdir(&self, path: &str) -> Result<(), String> {
        let (parent, name) = Self::split(path)?;
        let parent_id = self.dir_id(&parent)?;
        let node = self.prepare_node(&parent_id, &name)?;
        let dir_id = uuid::Uuid::new_v4().to_string();
        let folder = self.dir_path(&dir_id);
        fs::create_dir_all(&node)
            .and_then(|()| fs::write(node.join("dir.c9r"), &dir_id))
            .and_then(|()| fs::create_dir_all(&folder))
            .map_err(|error| format!("Could not create the folder: {error}"))?;
        self.write_encrypted(&folder.join("dirid.c9r"), &mut dir_id.as_bytes())
    }

    /// The file header, which carries the per-file content key.
    fn seal_header(&self, content_key: &[u8; 32]) -> Result<Vec<u8>, String> {
        let mut payload = Zeroizing::new(vec![0xff_u8; 8]);
        payload.extend_from_slice(content_key);
        match self.cipher {
            Cipher::Gcm => {
                let nonce: [u8; 12] = random();
                let sealed = Aes256Gcm::new_from_slice(self.enc_key.as_ref())
                    .expect("32-byte key")
                    .encrypt(Nonce::from_slice(&nonce), payload.as_slice())
                    .map_err(|_| "Could not encrypt the file header.")?;
                Ok([nonce.as_slice(), &sealed].concat())
            }
            Cipher::CtrMac => {
                let nonce: [u8; 16] = random();
                let mut encrypted = payload.to_vec();
                ctr_apply(self.enc_key.as_ref(), &nonce, &mut encrypted);
                let mac = hmac_sha256(
                    self.mac_key.as_ref(),
                    &[nonce.as_slice(), &encrypted].concat(),
                );
                Ok([nonce.as_slice(), &encrypted, &mac].concat())
            }
        }
    }

    fn open_header(&self, header: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
        let damaged = "The encrypted file's header could not be verified. It may be damaged.";
        let payload = Zeroizing::new(match self.cipher {
            Cipher::Gcm => Aes256Gcm::new_from_slice(self.enc_key.as_ref())
                .expect("32-byte key")
                .decrypt(Nonce::from_slice(&header[..12]), &header[12..])
                .map_err(|_| damaged)?,
            Cipher::CtrMac => {
                let (body, mac) = header.split_at(56);
                if hmac_sha256(self.mac_key.as_ref(), body) != mac {
                    return Err(damaged.into());
                }
                let mut payload = body[16..].to_vec();
                ctr_apply(self.enc_key.as_ref(), &body[..16], &mut payload);
                payload
            }
        });
        let mut key = Zeroizing::new([0_u8; 32]);
        key.copy_from_slice(&payload[8..40]);
        Ok(key)
    }

    fn seal_chunk(
        &self,
        key: &[u8; 32],
        header_nonce: &[u8],
        index: u64,
        data: &[u8],
    ) -> Result<Vec<u8>, String> {
        match self.cipher {
            Cipher::Gcm => {
                let nonce: [u8; 12] = random();
                let aad = [index.to_be_bytes().as_slice(), header_nonce].concat();
                let sealed = Aes256Gcm::new_from_slice(key)
                    .expect("32-byte key")
                    .encrypt(
                        Nonce::from_slice(&nonce),
                        Payload {
                            msg: data,
                            aad: &aad,
                        },
                    )
                    .map_err(|_| "Could not encrypt the file.")?;
                Ok([nonce.as_slice(), &sealed].concat())
            }
            Cipher::CtrMac => {
                let nonce: [u8; 16] = random();
                let mut encrypted = data.to_vec();
                ctr_apply(key, &nonce, &mut encrypted);
                let mac = hmac_sha256(
                    self.mac_key.as_ref(),
                    &[header_nonce, &index.to_be_bytes(), &nonce, &encrypted].concat(),
                );
                Ok([nonce.as_slice(), &encrypted, &mac].concat())
            }
        }
    }

    fn open_chunk(
        &self,
        key: &[u8; 32],
        header_nonce: &[u8],
        index: u64,
        chunk: &[u8],
    ) -> Result<Vec<u8>, String> {
        let damaged = format!(
            "Part {} of the encrypted file could not be verified. It may be damaged.",
            index + 1
        );
        match self.cipher {
            Cipher::Gcm => {
                let aad = [index.to_be_bytes().as_slice(), header_nonce].concat();
                Aes256Gcm::new_from_slice(key)
                    .expect("32-byte key")
                    .decrypt(
                        Nonce::from_slice(&chunk[..12]),
                        Payload {
                            msg: &chunk[12..],
                            aad: &aad,
                        },
                    )
                    .map_err(|_| damaged)
            }
            Cipher::CtrMac => {
                let (body, mac) = chunk.split_at(chunk.len() - 32);
                let expected = hmac_sha256(
                    self.mac_key.as_ref(),
                    &[header_nonce, &index.to_be_bytes(), body].concat(),
                );
                if expected != mac {
                    return Err(damaged);
                }
                let mut clear = body[16..].to_vec();
                ctr_apply(key, &body[..16], &mut clear);
                Ok(clear)
            }
        }
    }

    fn write_encrypted(&self, target: &Path, source: &mut dyn Read) -> Result<(), String> {
        let content_key = Zeroizing::new(random::<32>());
        let header = self.seal_header(&content_key)?;
        let header_nonce = &header[..self.cipher.nonce_size()];
        let temporary = target.with_extension("c9r.weave-tmp");
        let mut out = fs::File::create(&temporary)
            .map_err(|error| format!("Could not write to the vault: {error}"))?;
        out.write_all(&header).map_err(|error| error.to_string())?;
        let mut buffer = vec![0_u8; CHUNK_SIZE];
        let mut index = 0_u64;
        loop {
            let mut filled = 0;
            while filled < CHUNK_SIZE {
                let count = source
                    .read(&mut buffer[filled..])
                    .map_err(|error| format!("Could not read the file: {error}"))?;
                if count == 0 {
                    break;
                }
                filled += count;
            }
            if filled == 0 {
                break;
            }
            let chunk = self.seal_chunk(&content_key, header_nonce, index, &buffer[..filled])?;
            out.write_all(&chunk).map_err(|error| error.to_string())?;
            index += 1;
            if filled < CHUNK_SIZE {
                break;
            }
        }
        drop(out);
        fs::rename(&temporary, target)
            .map_err(|error| format!("Could not finish writing to the vault: {error}"))
    }

    fn read_encrypted(&self, source: &Path, target: &mut dyn Write) -> Result<(), String> {
        let mut file = fs::File::open(source)
            .map_err(|error| format!("Could not open the encrypted file: {error}"))?;
        let mut header = vec![0_u8; self.cipher.header_size()];
        file.read_exact(&mut header)
            .map_err(|_| "The encrypted file is damaged (header is incomplete).")?;
        let content_key = self.open_header(&header)?;
        let header_nonce = &header[..self.cipher.nonce_size()];
        let mut buffer = vec![0_u8; CHUNK_SIZE + self.cipher.overhead()];
        let mut index = 0_u64;
        loop {
            let mut filled = 0;
            while filled < buffer.len() {
                let count = file
                    .read(&mut buffer[filled..])
                    .map_err(|error| error.to_string())?;
                if count == 0 {
                    break;
                }
                filled += count;
            }
            if filled == 0 {
                break;
            }
            if filled < self.cipher.overhead() {
                return Err("The encrypted file is damaged (it ends early).".into());
            }
            let clear = self.open_chunk(&content_key, header_nonce, index, &buffer[..filled])?;
            target
                .write_all(&clear)
                .map_err(|error| format!("Could not write the decrypted file: {error}"))?;
            index += 1;
            if filled < buffer.len() {
                break;
            }
        }
        Ok(())
    }

    /// Encrypts a local file or folder into the vault folder `destination`.
    pub fn add(&self, source: &Path, destination: &str) -> Result<(), String> {
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .ok_or("Choose a file or folder to add.")?;
        let target = if destination.trim_matches('/').is_empty() {
            name.clone()
        } else {
            format!("{}/{name}", destination.trim_end_matches('/'))
        };
        let metadata = fs::symlink_metadata(source)
            .map_err(|error| format!("Could not read {name}: {error}"))?;
        if metadata.is_dir() {
            self.mkdir(&target)?;
            for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
                self.add(&entry.map_err(|error| error.to_string())?.path(), &target)?;
            }
            return Ok(());
        }
        if !metadata.is_file() {
            return Ok(());
        }
        let (parent, name) = Self::split(&target)?;
        let node = self.prepare_node(&self.dir_id(&parent)?, &name)?;
        let mut input =
            fs::File::open(source).map_err(|error| format!("Could not open {name}: {error}"))?;
        self.write_encrypted(&Self::file_location(&node), &mut input)
    }

    /// Decrypts a vault file or folder into the local folder `destination`.
    pub fn extract(&self, path: &str, destination: &Path) -> Result<(), String> {
        let (parent, name) = Self::split(path)?;
        let (node, _) = self.node_path(&self.dir_id(&parent)?, &name);
        let target = destination.join(&name);
        if target.exists() {
            return Err(format!(
                "{} already exists. Choose another destination.",
                target.display()
            ));
        }
        if node.join("dir.c9r").is_file() {
            fs::create_dir(&target)
                .map_err(|error| format!("Could not create {}: {error}", target.display()))?;
            for entry in self.list(path)? {
                self.extract(
                    &format!("{}/{}", path.trim_end_matches('/'), entry.name),
                    &target,
                )?;
            }
            return Ok(());
        }
        let temporary = destination.join(format!(".{name}.weave-decrypting"));
        let mut output = fs::File::create(&temporary)
            .map_err(|error| format!("Could not create {}: {error}", target.display()))?;
        let result = self.read_encrypted(&Self::file_location(&node), &mut output);
        drop(output);
        match result {
            Ok(()) => fs::rename(&temporary, &target).map_err(|error| error.to_string()),
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                Err(error)
            }
        }
    }

    pub fn delete(&self, path: &str) -> Result<(), String> {
        let (parent, name) = Self::split(path)?;
        let (node, _) = self.node_path(&self.dir_id(&parent)?, &name);
        if let Some(dir_id) = Self::child_dir_id(&node) {
            for entry in self.list(path)? {
                self.delete(&format!("{}/{}", path.trim_end_matches('/'), entry.name))?;
            }
            let folder = self.dir_path(&dir_id);
            fs::remove_dir_all(&folder)
                .map_err(|error| format!("Could not remove the folder: {error}"))?;
            if let Some(parent) = folder.parent() {
                let _ = fs::remove_dir(parent);
            }
            fs::remove_dir_all(&node).map_err(|error| error.to_string())
        } else if node.is_dir() {
            fs::remove_dir_all(&node).map_err(|error| error.to_string())
        } else {
            fs::remove_file(&node).map_err(|_| format!("“{name}” does not exist in this vault."))
        }
    }

    pub fn rename(&self, path: &str, new_name: &str) -> Result<(), String> {
        let (parent, name) = Self::split(path)?;
        let new_name = check_name(new_name)?;
        let parent_id = self.dir_id(&parent)?;
        let (from, _) = self.node_path(&parent_id, &name);
        if !from.exists() {
            return Err(format!("“{name}” does not exist in this vault."));
        }
        let (to, encrypted) = self.node_path(&parent_id, &new_name);
        if to.exists() {
            return Err(format!("“{new_name}” already exists in this folder."));
        }
        let shortened_from = Self::is_shortened(&from);
        let shortened_to = Self::is_shortened(&to);
        if shortened_from == shortened_to {
            fs::rename(&from, &to).map_err(|error| error.to_string())?;
            if shortened_to {
                fs::write(to.join("name.c9s"), encrypted).map_err(|error| error.to_string())?;
            }
            return Ok(());
        }
        // Moving between short and long names changes the node layout.
        let is_dir = from.join("dir.c9r").is_file();
        if shortened_to {
            fs::create_dir(&to).map_err(|error| error.to_string())?;
            fs::write(to.join("name.c9s"), encrypted).map_err(|error| error.to_string())?;
            if is_dir {
                fs::rename(from.join("dir.c9r"), to.join("dir.c9r"))
                    .map_err(|error| error.to_string())?;
                fs::remove_dir_all(&from).map_err(|error| error.to_string())
            } else {
                fs::rename(&from, to.join("contents.c9r")).map_err(|error| error.to_string())
            }
        } else if is_dir {
            fs::create_dir(&to).map_err(|error| error.to_string())?;
            fs::rename(from.join("dir.c9r"), to.join("dir.c9r"))
                .map_err(|error| error.to_string())?;
            fs::remove_dir_all(&from).map_err(|error| error.to_string())
        } else {
            fs::rename(from.join("contents.c9r"), &to).map_err(|error| error.to_string())?;
            fs::remove_dir_all(&from).map_err(|error| error.to_string())
        }
    }
}

pub fn cleartext_size(cipher: Cipher, encrypted: u64) -> u64 {
    let Some(body) = encrypted.checked_sub(cipher.header_size() as u64) else {
        return 0;
    };
    let full = (CHUNK_SIZE + cipher.overhead()) as u64;
    let chunks = body / full;
    let rest = body % full;
    chunks * CHUNK_SIZE as u64 + rest.saturating_sub(cipher.overhead() as u64)
}

// ---------------------------------------------------------------------------
// Commands. Unlocked vaults stay in memory until locked or Weave quits.

fn unlocked() -> &'static Mutex<HashMap<String, Vault>> {
    static VAULTS: OnceLock<Mutex<HashMap<String, Vault>>> = OnceLock::new();
    VAULTS.get_or_init(Default::default)
}

fn with_vault<T>(id: &str, job: impl FnOnce(&Vault) -> Result<T, String>) -> Result<T, String> {
    let vaults = unlocked()
        .lock()
        .map_err(|_| "The vault is busy. Try again.")?;
    let vault = vaults
        .get(id)
        .ok_or("This vault is locked. Unlock it again.")?;
    job(vault)
}

fn remember(vault: Vault) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    unlocked()
        .lock()
        .map_err(|_| "The vault is busy. Try again.")?
        .insert(id.clone(), vault);
    Ok(id)
}

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| format!("The vault operation stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn cryptomator_unlock(folder: String, password: String) -> Result<String, String> {
    let password = Zeroizing::new(password);
    blocking(move || remember(Vault::unlock(Path::new(&folder), &password)?)).await
}

#[tauri::command]
pub async fn cryptomator_create(folder: String, password: String) -> Result<String, String> {
    let password = Zeroizing::new(password);
    blocking(move || remember(Vault::create(Path::new(&folder), &password)?)).await
}

#[tauri::command]
pub fn cryptomator_lock(id: String) {
    if let Ok(mut vaults) = unlocked().lock() {
        vaults.remove(&id);
    }
}

#[tauri::command]
pub async fn cryptomator_list(id: String, path: String) -> Result<Vec<VaultEntry>, String> {
    blocking(move || with_vault(&id, |vault| vault.list(&path))).await
}

#[tauri::command]
pub async fn cryptomator_mkdir(id: String, path: String) -> Result<(), String> {
    blocking(move || with_vault(&id, |vault| vault.mkdir(&path))).await
}

#[tauri::command]
pub async fn cryptomator_add(
    id: String,
    sources: Vec<String>,
    destination: String,
) -> Result<(), String> {
    blocking(move || {
        with_vault(&id, |vault| {
            sources
                .iter()
                .try_for_each(|source| vault.add(Path::new(source), &destination))
        })
    })
    .await
}

#[tauri::command]
pub async fn cryptomator_extract(
    id: String,
    paths: Vec<String>,
    destination: String,
) -> Result<(), String> {
    blocking(move || {
        with_vault(&id, |vault| {
            paths
                .iter()
                .try_for_each(|path| vault.extract(path, Path::new(&destination)))
        })
    })
    .await
}

#[tauri::command]
pub async fn cryptomator_delete(id: String, path: String) -> Result<(), String> {
    blocking(move || with_vault(&id, |vault| vault.delete(&path))).await
}

#[tauri::command]
pub async fn cryptomator_rename(id: String, path: String, name: String) -> Result<(), String> {
    blocking(move || with_vault(&id, |vault| vault.rename(&path, &name))).await
}

#[tauri::command]
pub async fn cryptomator_change_password(
    id: String,
    old: String,
    new: String,
) -> Result<(), String> {
    let (old, new) = (Zeroizing::new(old), Zeroizing::new(new));
    blocking(move || with_vault(&id, |vault| vault.change_password(&old, &new))).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::create(&dir.path().join("vault"), "correct horse").unwrap();
        (dir, vault)
    }

    #[test]
    fn base32_matches_rfc_4648() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32(&[0; 20]).len(), 32);
    }

    #[test]
    fn computes_cleartext_sizes() {
        assert_eq!(cleartext_size(Cipher::Gcm, 68), 0);
        assert_eq!(cleartext_size(Cipher::Gcm, 68 + 28 + 5), 5);
        assert_eq!(cleartext_size(Cipher::Gcm, 68 + 32796 + 28 + 1), 32769);
    }

    #[test]
    fn creates_unlocks_and_rejects_wrong_passwords() {
        let (dir, _) = sample_vault();
        let folder = dir.path().join("vault");
        assert!(Vault::unlock(&folder, "correct horse").is_ok());
        assert_eq!(
            Vault::unlock(&folder, "wrong horse").err().unwrap(),
            "Wrong vault password."
        );
        assert!(Vault::unlock(dir.path(), "x")
            .err()
            .unwrap()
            .contains("not a Cryptomator vault"));
        assert!(Vault::create(&folder, "correct horse")
            .err()
            .unwrap()
            .contains("empty folder"));
    }

    #[test]
    fn round_trips_files_folders_and_long_names() {
        let (dir, vault) = sample_vault();
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("Notes")).unwrap();
        let big: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
        fs::write(source.join("Notes/big.bin"), &big).unwrap();
        fs::write(source.join("Notes/empty.txt"), b"").unwrap();
        let long_name = format!("{}.txt", "long name ".repeat(20));
        fs::write(source.join(&long_name), b"long").unwrap();
        vault.add(&source.join("Notes"), "").unwrap();
        vault.add(&source.join(&long_name), "/Notes").unwrap();

        let root = vault.list("/").unwrap();
        assert_eq!(
            root,
            vec![VaultEntry {
                name: "Notes".into(),
                is_dir: true,
                size: 0
            }]
        );
        let notes = vault.list("/Notes").unwrap();
        let names: Vec<_> = notes
            .iter()
            .map(|entry| (entry.name.as_str(), entry.size))
            .collect();
        assert_eq!(
            names,
            [
                ("big.bin", 100_000),
                ("empty.txt", 0),
                (long_name.as_str(), 4)
            ]
        );

        let out = dir.path().join("out");
        fs::create_dir(&out).unwrap();
        vault.extract("/Notes", &out).unwrap();
        assert_eq!(fs::read(out.join("Notes/big.bin")).unwrap(), big);
        assert_eq!(
            fs::read(out.join("Notes").join(&long_name)).unwrap(),
            b"long"
        );
        assert!(vault
            .extract("/Notes", &out)
            .err()
            .unwrap()
            .contains("already exists"));

        vault
            .rename("/Notes/big.bin", &format!("{}.bin", "x".repeat(200)))
            .unwrap();
        vault
            .rename(&format!("/Notes/{}.bin", "x".repeat(200)), "small.bin")
            .unwrap();
        vault.mkdir("/Notes/Sub").unwrap();
        assert!(vault
            .mkdir("/Notes/Sub")
            .err()
            .unwrap()
            .contains("already exists"));
        vault.delete("/Notes").unwrap();
        assert!(vault.list("/").unwrap().is_empty());
    }

    #[test]
    fn detects_tampering_and_changes_the_password() {
        let (dir, vault) = sample_vault();
        let file = dir.path().join("secret.txt");
        fs::write(&file, b"top secret").unwrap();
        vault.add(&file, "/").unwrap();
        let (node, _) = vault.node_path("", "secret.txt");
        let mut bytes = fs::read(&node).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&node, bytes).unwrap();
        let out = tempfile::tempdir().unwrap();
        assert!(vault
            .extract("/secret.txt", out.path())
            .err()
            .unwrap()
            .contains("could not be verified"));
        assert!(!out.path().join("secret.txt").exists());

        let folder = dir.path().join("vault");
        vault
            .change_password("correct horse", "battery staple")
            .unwrap();
        assert!(Vault::unlock(&folder, "correct horse").is_err());
        assert!(Vault::unlock(&folder, "battery staple").is_ok());
    }

    /// Writes a vault for an external implementation to read, and reads one
    /// it wrote. Driven by test-infra/cryptomator_interop.py.
    #[test]
    #[ignore = "run by test-infra/run-cryptomator-tests.sh"]
    fn interoperates_with_another_implementation() {
        let base = PathBuf::from(std::env::var("WEAVE_CRYPTOMATOR_DIR").unwrap());
        match std::env::var("WEAVE_CRYPTOMATOR_STEP").unwrap().as_str() {
            "write" => {
                let vault = Vault::create(&base.join("weave-vault"), "interop password").unwrap();
                let source = base.join("weave-source");
                fs::create_dir_all(source.join("Folder")).unwrap();
                fs::write(source.join("Folder/hello.txt"), "hello from Weave").unwrap();
                let big: Vec<u8> = (0..70_000).map(|i| (i % 97) as u8).collect();
                fs::write(source.join("Folder/big.bin"), big).unwrap();
                fs::write(source.join(format!("{}.txt", "n".repeat(240))), "long").unwrap();
                for entry in fs::read_dir(&source).unwrap() {
                    vault.add(&entry.unwrap().path(), "/").unwrap();
                }
            }
            "read" => {
                let vault = Vault::unlock(&base.join("python-vault"), "interop password").unwrap();
                let names: Vec<_> = vault
                    .list("/")
                    .unwrap()
                    .into_iter()
                    .map(|entry| entry.name)
                    .collect();
                assert_eq!(names, ["Docs", "readme.txt"]);
                let out = base.join("weave-out");
                fs::create_dir_all(&out).unwrap();
                vault.extract("/Docs", &out).unwrap();
                vault.extract("/readme.txt", &out).unwrap();
                assert_eq!(
                    fs::read_to_string(out.join("readme.txt")).unwrap(),
                    "hello from Python"
                );
                let big: Vec<u8> = (0..70_000).map(|i| (i % 89) as u8).collect();
                assert_eq!(fs::read(out.join("Docs/data.bin")).unwrap(), big);
            }
            other => panic!("unknown step {other}"),
        }
    }

    #[test]
    #[ignore = "requires the format 7 reference script"]
    fn reads_and_writes_format_7_vaults_like_the_reference() {
        let base = PathBuf::from(std::env::var("WEAVE_CRYPTOMATOR_DIR").unwrap()).join("format7");
        let vault = Vault::unlock(&base, "format seven password").unwrap();
        assert_eq!(vault.cipher, Cipher::CtrMac);
        let names: Vec<_> = vault
            .list("/")
            .unwrap()
            .into_iter()
            .map(|entry| (entry.name, entry.size))
            .collect();
        assert_eq!(
            names,
            [
                ("big.bin".to_string(), 70_000),
                ("notes.txt".to_string(), 33)
            ]
        );
        let out = tempfile::tempdir().unwrap();
        vault.extract("/notes.txt", out.path()).unwrap();
        vault.extract("/big.bin", out.path()).unwrap();
        assert_eq!(
            fs::read_to_string(out.path().join("notes.txt")).unwrap(),
            "written by the format 7 reference"
        );
        assert_eq!(
            fs::read(out.path().join("big.bin")).unwrap(),
            (0..70_000).map(|i| (i % 83) as u8).collect::<Vec<_>>()
        );
        let source = tempfile::tempdir().unwrap();
        fs::write(
            source.path().join("from-weave.txt"),
            "written by Weave in format 7",
        )
        .unwrap();
        fs::write(
            source.path().join("weave-big.bin"),
            (0..70_000).map(|i| (i % 71) as u8).collect::<Vec<_>>(),
        )
        .unwrap();
        vault
            .add(&source.path().join("from-weave.txt"), "/")
            .unwrap();
        vault
            .add(&source.path().join("weave-big.bin"), "/")
            .unwrap();
        assert!(Vault::unlock(&base, "wrong password")
            .err()
            .unwrap()
            .contains("Wrong vault password"));
    }

    #[test]
    fn round_trips_ctrmac_files_and_detects_tampering() {
        let root = tempfile::tempdir().unwrap();
        let mut vault = Vault::create(&root.path().join("vault"), "password one").unwrap();
        vault.cipher = Cipher::CtrMac;
        let data: Vec<u8> = (0..(CHUNK_SIZE * 2 + 11))
            .map(|i| (i % 251) as u8)
            .collect();
        let file = root.path().join("file.c9r");
        vault.write_encrypted(&file, &mut data.as_slice()).unwrap();
        assert_eq!(
            cleartext_size(Cipher::CtrMac, fs::metadata(&file).unwrap().len()),
            data.len() as u64
        );
        let mut out = Vec::new();
        vault.read_encrypted(&file, &mut out).unwrap();
        assert_eq!(out, data);
        let mut bytes = fs::read(&file).unwrap();
        bytes[100] ^= 1;
        fs::write(&file, bytes).unwrap();
        assert!(vault
            .read_encrypted(&file, &mut Vec::new())
            .unwrap_err()
            .contains("Part 1"));
    }
}
