use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownHost {
    pub hosts: String,
    pub key_type: String,
    pub fingerprint: String,
    pub marker: Option<String>,
    pub hashed: bool,
}

fn path() -> Result<std::path::PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or("Your home folder is unavailable.")?
        .join(".ssh/known_hosts"))
}

fn list(file: &Path) -> Result<Vec<KnownHost>, String> {
    if !file.exists() {
        return Ok(Vec::new());
    }
    let size = fs::metadata(file)
        .map_err(|error| format!("Could not inspect known hosts: {error}"))?
        .len();
    if size > 8 * 1024 * 1024 {
        return Err(
            "The known hosts file is over 8 MB. Open it in a text editor to inspect it.".into(),
        );
    }
    let content =
        fs::read_to_string(file).map_err(|error| format!("Could not read known hosts: {error}"))?;
    Ok(content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let mut fields = line.split_whitespace();
            let first = fields.next()?;
            let (marker, hosts) = if first.starts_with('@') {
                (Some(first.to_string()), fields.next()?)
            } else {
                (None, first)
            };
            let key_type = fields.next()?;
            let key = STANDARD.decode(fields.next()?).ok()?;
            let fingerprint =
                base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(&key));
            Some(KnownHost {
                hosts: hosts.into(),
                key_type: key_type.into(),
                fingerprint: format!("SHA256:{fingerprint}"),
                marker,
                hashed: hosts.starts_with("|1|"),
            })
        })
        .collect())
}

fn remove(file: &Path, host: &str) -> Result<(), String> {
    if host.is_empty()
        || host.starts_with('-')
        || host.chars().any(char::is_whitespace)
        || host.contains(['\0', '\r', '\n'])
    {
        return Err("Enter one host name or [host]:port without spaces or options.".into());
    }
    if !file.exists() {
        return Err("There is no known hosts file on this computer.".into());
    }
    let found = crate::platform::command("ssh-keygen")
        .args(["-F", host, "-f"])
        .arg(file)
        .output()
        .map_err(|error| format!("Could not search known hosts: {error}"))?;
    if !found.status.success() || found.stdout.is_empty() {
        return Err(format!("No saved SSH key was found for {host}."));
    }
    let output = crate::platform::command("ssh-keygen")
        .args(["-R", host, "-f"])
        .arg(file)
        .output()
        .map_err(|error| format!("Could not run ssh-keygen: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Could not forget {host}: {}", detail.trim()));
    }
    Ok(())
}

#[tauri::command]
pub async fn known_hosts_list() -> Result<Vec<KnownHost>, String> {
    tauri::async_runtime::spawn_blocking(|| list(&path()?))
        .await
        .map_err(|error| format!("Reading known hosts stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn known_hosts_remove(host: String, confirmed: bool) -> Result<(), String> {
    if !confirmed {
        return Err("Confirm that you want to forget this SSH host key.".into());
    }
    tauri::async_runtime::spawn_blocking(move || remove(&path()?, &host))
        .await
        .map_err(|error| format!("Removing the SSH host key stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_fingerprints_and_removes_named_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("known_hosts");
        let key = STANDARD.encode(b"disposable public key bytes");
        fs::write(
            &file,
            format!("# comment\nnas.local ssh-ed25519 {key}\n@cert-authority *.example.test ssh-ed25519 {key}\n|1|AAAAAAAAAAAAAAAAAAAAAAAAAAA=|AAAAAAAAAAAAAAAAAAAAAAAAAAA= ssh-rsa {key}\n"),
        )
        .unwrap();
        let entries = list(&file).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].hosts, "nas.local");
        assert!(entries[0].fingerprint.starts_with("SHA256:"));
        assert_eq!(entries[1].marker.as_deref(), Some("@cert-authority"));
        assert!(entries[2].hashed);
        remove(&file, "nas.local").unwrap();
        assert_eq!(list(&file).unwrap().len(), 2);
        assert!(remove(&file, "missing.example")
            .unwrap_err()
            .contains("No saved SSH key"));
        assert!(remove(&file, "-oProxyCommand=bad").is_err());
    }
}
