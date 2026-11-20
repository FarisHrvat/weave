//! Encrypted settings vault for moving Weave setup between Macs.
//!
//! A vault file holds host profiles, saved workspaces and the app's local
//! settings (snippets, scripts, triggers, saved connections, bookmarks and
//! terminal appearance). It never includes passwords, S3 session tokens or
//! forge tokens, which stay in this Mac's Keychain. The contents are
//! encrypted with AES-256-GCM using a key derived from a passphrase with
//! Argon2id, so the file can be kept in iCloud Drive, Dropbox or a USB drive.

use crate::{profiles, workspaces, AppState};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};
use tauri::State;
use zeroize::Zeroizing;

const FORMAT: &str = "weave-settings-vault";
const MAX_VAULT_BYTES: u64 = 20 * 1024 * 1024;
const ARGON_MEMORY_KIB: u32 = 64 * 1024;
const ARGON_ITERATIONS: u32 = 3;

#[derive(Serialize, Deserialize)]
struct Envelope {
    format: String,
    version: u32,
    kdf: Kdf,
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize, Deserialize)]
struct Kdf {
    algorithm: String,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    salt: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VaultContents {
    pub created_at: i64,
    pub profiles: Vec<profiles::Profile>,
    pub workspaces: Vec<VaultWorkspace>,
    /// Frontend settings keyed by their local storage key.
    pub local: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct VaultWorkspace {
    pub id: String,
    pub name: String,
    pub layout: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultImportReport {
    pub profiles: usize,
    pub workspaces: usize,
    pub skipped: Vec<String>,
    pub local: BTreeMap<String, String>,
}

fn validate_passphrase(passphrase: &str) -> Result<(), String> {
    if passphrase.chars().count() < 10 {
        return Err("Use a passphrase of at least 10 characters for the vault.".into());
    }
    if passphrase.len() > 1024 {
        return Err("The passphrase is too long.".into());
    }
    Ok(())
}

fn derive_key(passphrase: &str, kdf: &Kdf) -> Result<Zeroizing<[u8; 32]>, String> {
    if kdf.algorithm != "argon2id"
        || !(8 * 1024..=1024 * 1024).contains(&kdf.memory_kib)
        || !(1..=10).contains(&kdf.iterations)
        || !(1..=8).contains(&kdf.parallelism)
    {
        return Err("This vault uses unsupported encryption settings.".into());
    }
    let salt = STANDARD
        .decode(&kdf.salt)
        .map_err(|_| "The vault file is damaged.")?;
    let params = Params::new(kdf.memory_kib, kdf.iterations, kdf.parallelism, Some(32))
        .map_err(|error| format!("Invalid vault settings: {error}"))?;
    let mut key = Zeroizing::new([0_u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), &salt, key.as_mut())
        .map_err(|error| format!("Could not derive the vault key: {error}"))?;
    Ok(key)
}

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    use aes_gcm::aead::{rand_core::RngCore, OsRng};
    let mut bytes = [0_u8; N];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|error| format!("Could not get random bytes: {error}"))?;
    Ok(bytes)
}

pub(crate) fn seal<T: Serialize>(contents: &T, passphrase: &str) -> Result<String, String> {
    validate_passphrase(passphrase)?;
    let kdf = Kdf {
        algorithm: "argon2id".into(),
        memory_kib: ARGON_MEMORY_KIB,
        iterations: ARGON_ITERATIONS,
        parallelism: 1,
        salt: STANDARD.encode(random_bytes::<16>()?),
    };
    let key = derive_key(passphrase, &kdf)?;
    let nonce = random_bytes::<12>()?;
    let plaintext = Zeroizing::new(
        serde_json::to_vec(contents)
            .map_err(|error| format!("Could not prepare vault: {error}"))?,
    );
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| "Invalid vault key.")?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext.as_slice())
        .map_err(|_| "Could not encrypt the vault.")?;
    serde_json::to_string_pretty(&Envelope {
        format: FORMAT.into(),
        version: 1,
        kdf,
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    })
    .map_err(|error| format!("Could not write vault: {error}"))
}

pub(crate) fn open<T: serde::de::DeserializeOwned>(
    text: &str,
    passphrase: &str,
) -> Result<T, String> {
    let envelope: Envelope =
        serde_json::from_str(text).map_err(|_| "This file is not a Weave settings vault.")?;
    if envelope.format != FORMAT {
        return Err("This file is not a Weave settings vault.".into());
    }
    if envelope.version != 1 {
        return Err("This vault was made by a newer version of Weave. Update Weave first.".into());
    }
    let key = derive_key(passphrase, &envelope.kdf)?;
    let nonce = STANDARD
        .decode(&envelope.nonce)
        .ok()
        .filter(|nonce| nonce.len() == 12)
        .ok_or("The vault file is damaged.")?;
    let ciphertext = STANDARD
        .decode(&envelope.ciphertext)
        .map_err(|_| "The vault file is damaged.")?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| "Invalid vault key.")?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext.as_slice())
            .map_err(|_| "Wrong passphrase, or the vault file was changed.")?,
    );
    serde_json::from_slice(&plaintext).map_err(|_| "The vault contents are damaged.".into())
}

fn allowed_local_key(key: &str) -> bool {
    key.starts_with("weave-") && key != "weave-terminal-history" && key.len() <= 200
}

#[tauri::command]
pub fn settings_vault_export(
    state: State<'_, AppState>,
    path: String,
    passphrase: String,
    local: BTreeMap<String, String>,
) -> Result<String, String> {
    let passphrase = Zeroizing::new(passphrase);
    let destination = Path::new(&path);
    if !destination.is_absolute()
        || destination
            .extension()
            .is_none_or(|ext| ext != "weavevault")
    {
        return Err("Choose a file name ending in .weavevault.".into());
    }
    let contents = VaultContents {
        created_at: chrono::Utc::now().timestamp(),
        profiles: profiles::profiles_list(state.clone())?,
        workspaces: workspaces::workspaces_list(state)?
            .into_iter()
            .map(|workspace| VaultWorkspace {
                id: workspace.id,
                name: workspace.name,
                layout: workspace.layout,
            })
            .collect(),
        local: local
            .into_iter()
            .filter(|(key, value)| allowed_local_key(key) && value.len() <= 2 * 1024 * 1024)
            .collect(),
    };
    let sealed = seal(&contents, &passphrase)?;
    let temporary = destination.with_extension("weavevault.tmp");
    fs::write(&temporary, sealed)
        .and_then(|()| fs::rename(&temporary, destination))
        .map_err(|error| format!("Could not save the vault: {error}"))?;
    Ok(format!(
        "Saved {} hosts, {} workspaces and {} settings groups.",
        contents.profiles.len(),
        contents.workspaces.len(),
        contents.local.len()
    ))
}

#[tauri::command]
pub fn settings_vault_import(
    state: State<'_, AppState>,
    path: String,
    passphrase: String,
) -> Result<VaultImportReport, String> {
    let passphrase = Zeroizing::new(passphrase);
    let metadata =
        fs::metadata(&path).map_err(|error| format!("Could not open the vault: {error}"))?;
    if metadata.len() > MAX_VAULT_BYTES {
        return Err("This vault is larger than 20 MB.".into());
    }
    let text =
        fs::read_to_string(&path).map_err(|error| format!("Could not read the vault: {error}"))?;
    let contents: VaultContents = open(&text, &passphrase)?;
    let mut report = VaultImportReport {
        profiles: 0,
        workspaces: 0,
        skipped: Vec::new(),
        local: contents
            .local
            .into_iter()
            .filter(|(key, _)| allowed_local_key(key))
            .collect(),
    };
    for profile in contents.profiles {
        let name = profile.name.clone();
        match profiles::profiles_save(state.clone(), profile) {
            Ok(_) => report.profiles += 1,
            Err(error) => report.skipped.push(format!("Host {name}: {error}")),
        }
    }
    for workspace in contents.workspaces {
        let name = workspace.name.clone();
        match workspaces::workspaces_save(
            state.clone(),
            Some(workspace.id),
            workspace.name,
            workspace.layout,
        ) {
            Ok(_) => report.workspaces += 1,
            Err(error) => report.skipped.push(format!("Workspace {name}: {error}")),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> VaultContents {
        VaultContents {
            created_at: 1,
            profiles: vec![profiles::Profile {
                id: "p1".into(),
                name: "NAS".into(),
                host: "nas.local".into(),
                protocol: "ssh".into(),
                port: 22,
                username: "alice".into(),
                key_path: Some("~/.ssh/id_ed25519".into()),
                jump_host: None,
                color: "#938cff".into(),
                favorite: true,
                tls_pin_sha256: None,
                s3_region: None,
            }],
            workspaces: vec![],
            local: BTreeMap::from([("weave-snippets".into(), "[]".into())]),
        }
    }

    #[test]
    fn seals_and_opens_with_the_right_passphrase_only() {
        let sealed = seal(&sample(), "correct horse battery").unwrap();
        assert!(!sealed.contains("nas.local"));
        assert!(!sealed.contains("alice"));
        let opened: VaultContents = open(&sealed, "correct horse battery").unwrap();
        assert_eq!(opened.profiles[0].host, "nas.local");
        assert_eq!(opened.local["weave-snippets"], "[]");
        assert!(open::<VaultContents>(&sealed, "wrong passphrase!!")
            .err()
            .unwrap()
            .contains("Wrong passphrase"));
        assert!(seal(&sample(), "short").is_err());
    }

    #[test]
    fn rejects_tampering_and_foreign_files() {
        let sealed = seal(&sample(), "correct horse battery").unwrap();
        let mut envelope: Envelope = serde_json::from_str(&sealed).unwrap();
        let mut bytes = STANDARD.decode(&envelope.ciphertext).unwrap();
        bytes[0] ^= 1;
        envelope.ciphertext = STANDARD.encode(bytes);
        let tampered = serde_json::to_string(&envelope).unwrap();
        assert!(open::<VaultContents>(&tampered, "correct horse battery").is_err());
        assert!(open::<VaultContents>("{}", "correct horse battery").is_err());
        envelope.kdf.memory_kib = 1;
        assert!(open::<VaultContents>(
            &serde_json::to_string(&envelope).unwrap(),
            "correct horse battery"
        )
        .err()
        .unwrap()
        .contains("unsupported"));
    }

    #[test]
    fn keeps_history_out_of_vaults() {
        assert!(allowed_local_key("weave-snippets"));
        assert!(!allowed_local_key("weave-terminal-history"));
        assert!(!allowed_local_key("other-app"));
    }
}
