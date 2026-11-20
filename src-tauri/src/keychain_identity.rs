//! Client certificates kept by the system: the Keychain on macOS and the
//! personal certificate store on Windows. The chosen identity is exported once
//! (the system may ask permission for its private key) into a private PEM file
//! in Weave's app data, which FTPS and WebDAV then use. Linux has no shared
//! store, so there a certificate file is chosen directly.

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeychainIdentity {
    /// SHA-256 of the certificate, used to pick it again.
    id: String,
    name: String,
}

#[cfg(target_os = "macos")]
mod system {
    use super::KeychainIdentity;
    use core_foundation::{base::TCFType, data::CFData, string::CFString};
    use security_framework::{
        identity::SecIdentity,
        item::{ItemClass, ItemSearchOptions, Limit, Reference, SearchResult},
        os::macos::keychain::SecKeychain,
    };
    use security_framework_sys::import_export::{
        kSecFormatPKCS12, SecItemExport, SecItemImportExportKeyParameters,
        SEC_KEY_IMPORT_EXPORT_PARAMS_VERSION,
    };
    use sha2::{Digest, Sha256};
    use std::{io::Write, process::Stdio};

    pub(super) fn identities(
        keychains: Option<&[SecKeychain]>,
    ) -> Result<Vec<(KeychainIdentity, SecIdentity)>, String> {
        let mut search = ItemSearchOptions::new();
        search
            .class(ItemClass::identity())
            .load_refs(true)
            .limit(Limit::All);
        if let Some(keychains) = keychains {
            search.keychains(keychains);
        }
        let results = match search.search() {
            Ok(results) => results,
            // errSecItemNotFound: there are none.
            Err(error) if error.code() == -25300 => return Ok(Vec::new()),
            Err(error) => {
                return Err(format!(
                    "Could not read identities from the Keychain: {error}"
                ))
            }
        };
        let mut found = Vec::new();
        for result in results {
            let SearchResult::Ref(Reference::Identity(identity)) = result else {
                continue;
            };
            let Ok(certificate) = identity.certificate() else {
                continue;
            };
            let id = format!("{:x}", Sha256::digest(certificate.to_der()));
            if found
                .iter()
                .any(|(item, _): &(KeychainIdentity, SecIdentity)| item.id == id)
            {
                continue;
            }
            found.push((
                KeychainIdentity {
                    id,
                    name: certificate.subject_summary(),
                },
                identity,
            ));
        }
        found.sort_by_key(|item| item.0.name.to_lowercase());
        Ok(found)
    }

    /// Exports one identity as PEM (certificate and unencrypted key).
    pub(super) fn export_pem(identity: &SecIdentity) -> Result<Vec<u8>, String> {
        let password = uuid::Uuid::new_v4().simple().to_string();
        let passphrase = CFString::new(&password);
        let prompt =
            CFString::new("Weave needs this certificate's private key to sign in to a server.");
        let parameters = SecItemImportExportKeyParameters {
            version: SEC_KEY_IMPORT_EXPORT_PARAMS_VERSION,
            flags: 0,
            passphrase: passphrase.as_CFTypeRef(),
            alertTitle: std::ptr::null(),
            alertPrompt: prompt.as_concrete_TypeRef(),
            accessRef: std::ptr::null_mut(),
            keyUsage: std::ptr::null(),
            keyAttributes: std::ptr::null(),
        };
        let mut data = std::ptr::null();
        let status = unsafe {
            SecItemExport(
                identity.as_CFTypeRef(),
                kSecFormatPKCS12,
                0,
                &parameters,
                &mut data,
            )
        };
        if status != 0 || data.is_null() {
            return Err(match status {
                -128 => "Exporting the certificate was cancelled.".to_string(),
                -25260 | -25308 => "macOS did not allow its private key to be exported. Allow it when asked, or check that the key is exportable.".to_string(),
                _ => format!("macOS could not export the certificate (error {status})."),
            });
        }
        let pkcs12 = unsafe { CFData::wrap_under_create_rule(data) }
            .bytes()
            .to_vec();
        let mut child = crate::platform::command("openssl")
            .args(["pkcs12", "-nodes", "-passin", "env:WEAVE_P12_PASSWORD"])
            .env("WEAVE_P12_PASSWORD", &password)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Could not convert the certificate: {error}"))?;
        child
            .stdin
            .take()
            .ok_or("Could not convert the certificate.")?
            .write_all(&pkcs12)
            .map_err(|error| format!("Could not convert the certificate: {error}"))?;
        let output = child
            .wait_with_output()
            .map_err(|error| format!("Could not convert the certificate: {error}"))?;
        let pem = output.stdout;
        let text = String::from_utf8_lossy(&pem);
        if !output.status.success()
            || !text.contains("BEGIN CERTIFICATE")
            || !text.contains("PRIVATE KEY")
        {
            return Err("The exported certificate could not be read.".into());
        }
        Ok(pem)
    }

    pub fn list() -> Result<Vec<KeychainIdentity>, String> {
        Ok(identities(None)?
            .into_iter()
            .map(|(item, _)| item)
            .collect())
    }

    pub fn export(id: &str) -> Result<Vec<u8>, String> {
        let (_, identity) = identities(None)?
            .into_iter()
            .find(|(item, _)| item.id == id)
            .ok_or("That certificate is no longer in the Keychain.")?;
        export_pem(&identity)
    }
}

#[cfg(windows)]
mod system {
    use super::KeychainIdentity;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use sha2::{Digest, Sha256};

    fn powershell(script: &str, password: Option<&str>) -> Result<String, String> {
        let mut command = crate::platform::command("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
        if let Some(password) = password {
            command.env("WEAVE_P12_PASSWORD", password);
        }
        let output = command
            .output()
            .map_err(|error| format!("Could not read the certificate store: {error}"))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(
                if detail.contains("not exportable") || detail.contains("exportable") {
                    "Windows did not allow this certificate's private key to be exported. Mark the key as exportable when importing it.".into()
                } else {
                    format!(
                        "Windows could not read the certificate: {}",
                        detail.trim().lines().next().unwrap_or("unknown error")
                    )
                },
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Thumbprint, SHA-256 id and name of each certificate with a private key.
    fn certificates() -> Result<Vec<(String, KeychainIdentity)>, String> {
        let text = powershell(
            "Get-ChildItem Cert:\\CurrentUser\\My | Where-Object { $_.HasPrivateKey } | ForEach-Object { $_.Thumbprint + \"`t\" + [Convert]::ToBase64String($_.RawData) + \"`t\" + $_.GetNameInfo('SimpleName', $false) }",
            None,
        )?;
        Ok(parse_list(&text))
    }

    pub(super) fn parse_list(text: &str) -> Vec<(String, KeychainIdentity)> {
        let mut found: Vec<(String, KeychainIdentity)> = Vec::new();
        for line in text.lines() {
            let mut parts = line.trim_end_matches('\r').splitn(3, '\t');
            let (Some(thumbprint), Some(der), name) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if !thumbprint.chars().all(|c| c.is_ascii_hexdigit()) || thumbprint.is_empty() {
                continue;
            }
            let Ok(der) = STANDARD.decode(der.trim()) else {
                continue;
            };
            let id = format!("{:x}", Sha256::digest(&der));
            if found.iter().any(|(_, item)| item.id == id) {
                continue;
            }
            let name = name.unwrap_or("").trim();
            found.push((
                thumbprint.to_string(),
                KeychainIdentity {
                    id,
                    name: if name.is_empty() {
                        "Certificate".into()
                    } else {
                        name.into()
                    },
                },
            ));
        }
        found.sort_by_key(|item| item.1.name.to_lowercase());
        found
    }

    pub fn list() -> Result<Vec<KeychainIdentity>, String> {
        Ok(certificates()?.into_iter().map(|(_, item)| item).collect())
    }

    pub fn export(id: &str) -> Result<Vec<u8>, String> {
        let (thumbprint, _) = certificates()?
            .into_iter()
            .find(|(_, item)| item.id == id)
            .ok_or("That certificate is no longer in the Windows certificate store.")?;
        let password = uuid::Uuid::new_v4().simple().to_string();
        let script = format!(
            "$c = Get-Item Cert:\\CurrentUser\\My\\{thumbprint}; [Convert]::ToBase64String($c.Export('Pfx', $env:WEAVE_P12_PASSWORD))"
        );
        let pfx = STANDARD
            .decode(powershell(&script, Some(&password))?.trim())
            .map_err(|_| "Windows returned an unreadable certificate.")?;
        pem_from_pkcs12(&pfx, &password)
    }

    fn pem_block(label: &str, der: &[u8]) -> String {
        let encoded = STANDARD.encode(der);
        let mut out = format!("-----BEGIN {label}-----\n");
        for chunk in encoded.as_bytes().chunks(64) {
            out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
            out.push('\n');
        }
        out.push_str(&format!("-----END {label}-----\n"));
        out
    }

    /// Converts a PKCS#12 file into the PEM Weave's TLS code reads.
    pub(super) fn pem_from_pkcs12(data: &[u8], password: &str) -> Result<Vec<u8>, String> {
        use p12_keystore::{KeyStore, KeyStoreEntry, Pkcs12ImportPolicy};
        let store = KeyStore::from_pkcs12(data, password, Pkcs12ImportPolicy::Relaxed)
            .map_err(|error| format!("The exported certificate could not be read: {error}"))?;
        for (_, entry) in store.entries() {
            if let KeyStoreEntry::PrivateKeyChain(chain) = entry {
                if chain.certs().is_empty() {
                    continue;
                }
                let mut pem = String::new();
                for certificate in chain.certs() {
                    pem.push_str(&pem_block("CERTIFICATE", certificate.as_der()));
                }
                pem.push_str(&pem_block("PRIVATE KEY", chain.key().as_der()));
                return Ok(pem.into_bytes());
            }
        }
        Err("The exported certificate has no private key.".into())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod system {
    use super::KeychainIdentity;

    pub fn list() -> Result<Vec<KeychainIdentity>, String> {
        Ok(Vec::new())
    }

    pub fn export(_id: &str) -> Result<Vec<u8>, String> {
        Err("Linux has no shared certificate store. Choose a PEM file with the certificate and its private key.".into())
    }
}

fn save_private(folder: &Path, id: &str, pem: &[u8]) -> Result<PathBuf, String> {
    crate::platform::create_private_dir(folder)
        .map_err(|error| format!("Could not prepare the certificate folder: {error}"))?;
    let path = folder.join(format!("{}.pem", &id[..16]));
    crate::platform::write_private(&path, pem)
        .map_err(|error| format!("Could not save the certificate: {error}"))?;
    Ok(path)
}

#[tauri::command]
pub async fn keychain_identities() -> Result<Vec<KeychainIdentity>, String> {
    tauri::async_runtime::spawn_blocking(system::list)
        .await
        .map_err(|error| format!("Reading the certificate store stopped unexpectedly: {error}"))?
}

/// Exports the identity and returns the path to use as the client certificate.
#[tauri::command]
pub async fn keychain_identity_export(app: AppHandle, id: String) -> Result<String, String> {
    let folder = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not find Weave's data folder: {error}"))?
        .join("client-certificates");
    tauri::async_runtime::spawn_blocking(move || {
        if id.len() < 16 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("Choose a certificate from the list.".to_string());
        }
        let path = save_private(&folder, &id, &system::export(&id)?)?;
        Ok(path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|error| format!("Exporting the certificate stopped unexpectedly: {error}"))?
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::system::*;
    use super::*;
    use security_framework::os::macos::keychain::SecKeychain;

    #[test]
    fn lists_and_exports_an_identity_from_a_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let output = crate::platform::command(args[0])
                .args(&args[1..])
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&[
            "/usr/bin/openssl",
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "key.pem",
            "-out",
            "cert.pem",
            "-days",
            "1",
            "-subj",
            "/CN=Weave Test Client",
        ]);
        run(&[
            "/usr/bin/openssl",
            "pkcs12",
            "-export",
            "-inkey",
            "key.pem",
            "-in",
            "cert.pem",
            "-out",
            "id.p12",
            "-passout",
            "pass:import",
        ]);
        let keychain_path = dir.path().join("test.keychain");
        run(&[
            "/usr/bin/security",
            "create-keychain",
            "-p",
            "keychain password",
            keychain_path.to_str().unwrap(),
        ]);
        run(&[
            "/usr/bin/security",
            "unlock-keychain",
            "-p",
            "keychain password",
            keychain_path.to_str().unwrap(),
        ]);
        run(&[
            "/usr/bin/security",
            "import",
            "id.p12",
            "-k",
            keychain_path.to_str().unwrap(),
            "-P",
            "import",
            "-A",
        ]);
        let keychain = SecKeychain::open(&keychain_path).unwrap();
        let found = identities(Some(&[keychain])).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.name, "Weave Test Client");
        let pem = export_pem(&found[0].1).unwrap();
        let saved = save_private(&dir.path().join("out"), &found[0].0.id, &pem).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(crate::webdav::load_identity(&saved).is_ok());
        let _ = crate::platform::command("/usr/bin/security")
            .args(["delete-keychain", keychain_path.to_str().unwrap()])
            .output();
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::system::*;

    #[test]
    fn reads_the_certificate_list() {
        let found = parse_list("ABCDEF\tAAEC\tWeave Test\r\nnot a line\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.name, "Weave Test");
    }

    #[test]
    fn refuses_a_file_that_is_not_pkcs12() {
        assert!(pem_from_pkcs12(b"nope", "x").is_err());
    }
}
