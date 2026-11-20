use crate::{profiles::Profile, AppState};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshPublicKey {
    pub name: String,
    pub public_path: String,
    pub key_type: String,
    pub fingerprint: String,
    pub comment: String,
    pub has_private: bool,
}

fn ssh_directory() -> Result<PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or("Your home folder is unavailable.")?
        .join(".ssh"))
}

fn list_keys(directory: &Path) -> Result<Vec<SshPublicKey>, String> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(directory)
        .map_err(|error| {
            format!(
                "Could not read SSH keys in {}: {error}",
                directory.display()
            )
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "pub"))
        .collect::<Vec<_>>();
    paths.sort();
    let mut keys = Vec::new();
    for path in paths.into_iter().take(128) {
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
        if !metadata.file_type().is_file() || metadata.len() > 16 * 1024 {
            continue;
        }
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        let mut fields = content.split_whitespace();
        let Some(key_type) = fields.next() else {
            continue;
        };
        let Some(_) = fields.next() else { continue };
        let output = crate::platform::command("ssh-keygen")
            .args(["-lf"])
            .arg(&path)
            .args(["-E", "sha256"])
            .output()
            .map_err(|error| format!("Could not inspect SSH key {}: {error}", path.display()))?;
        if !output.status.success() {
            continue;
        }
        let details = String::from_utf8_lossy(&output.stdout);
        let Some(fingerprint) = details.split_whitespace().nth(1) else {
            continue;
        };
        keys.push(SshPublicKey {
            name: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            public_path: path.to_string_lossy().into_owned(),
            key_type: key_type.into(),
            fingerprint: fingerprint.into(),
            comment: fields.collect::<Vec<_>>().join(" "),
            has_private: fs::symlink_metadata(path.with_extension(""))
                .is_ok_and(|metadata| metadata.file_type().is_file()),
        });
    }
    Ok(keys)
}

fn generate_key(directory: &Path, name: &str) -> Result<SshPublicKey, String> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("Use 1–64 letters, digits, underscores, or dashes for the key name.".into());
    }
    if !directory.exists() {
        fs::create_dir(directory)
            .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("Could not secure {}: {error}", directory.display()))?;
        }
    }
    let metadata = fs::symlink_metadata(directory)
        .map_err(|error| format!("Could not inspect {}: {error}", directory.display()))?;
    if !metadata.file_type().is_dir() {
        return Err("The SSH key folder must be a real directory, not a symbolic link.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err("Your ~/.ssh folder must belong to your account and have mode 0700 before creating a key.".into());
        }
    }
    let private = directory.join(format!("weave_{name}"));
    let public = private.with_extension("pub");
    if fs::symlink_metadata(&private).is_ok() || fs::symlink_metadata(&public).is_ok() {
        return Err(format!(
            "A key named weave_{name} already exists. Choose another name."
        ));
    }
    let output = crate::platform::command("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "Weave SSH key", "-f"])
        .arg(&private)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("Could not start ssh-keygen: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not generate the SSH key: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    list_keys(directory)?
        .into_iter()
        .find(|key| key.public_path == public.to_string_lossy())
        .ok_or_else(|| "ssh-keygen finished, but the new public key could not be read.".into())
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn install_command(
    profile: &Profile,
    public_path: &Path,
    directory: &Path,
) -> Result<String, String> {
    if profile.protocol != "ssh" {
        return Err("Choose a saved SSH host before installing a key.".into());
    }
    crate::profiles::validate_profile(profile)?;
    if !list_keys(directory)?
        .iter()
        .any(|key| Path::new(&key.public_path) == public_path && key.has_private)
    {
        return Err("Choose a public key with its matching private key in ~/.ssh.".into());
    }
    let mut options = vec!["-p".to_string(), profile.port.to_string()];
    if let Some(jump) = &profile.jump_host {
        options.extend(["-o".into(), format!("ProxyJump={jump}")]);
    }
    if let Some(key_path) = &profile.key_path {
        options.extend(["-o".into(), format!("IdentityFile={key_path}")]);
    }
    let target = format!("{}@{}", profile.username, profile.host);
    if cfg!(windows) {
        // Windows has no ssh-copy-id; the terminal there runs PowerShell.
        let ps = |value: &str| format!("'{}'", value.replace('\'', "''"));
        let ssh = options
            .iter()
            .chain(std::iter::once(&target))
            .map(|arg| ps(arg))
            .collect::<Vec<_>>()
            .join(" ");
        return Ok(format!(
            "Get-Content -Raw {} | ssh {ssh} 'umask 077; mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys'",
            ps(&public_path.to_string_lossy())
        ));
    }
    let mut args = vec![
        "ssh-copy-id".to_string(),
        "-i".into(),
        public_path.to_string_lossy().into_owned(),
    ];
    args.extend(options);
    args.push(target);
    Ok(args
        .iter()
        .map(|arg| quote(arg))
        .collect::<Vec<_>>()
        .join(" "))
}

#[tauri::command]
pub async fn ssh_keys_list() -> Result<Vec<SshPublicKey>, String> {
    tauri::async_runtime::spawn_blocking(|| list_keys(&ssh_directory()?))
        .await
        .map_err(|error| format!("Listing SSH keys stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn ssh_keys_generate(name: String) -> Result<SshPublicKey, String> {
    tauri::async_runtime::spawn_blocking(move || generate_key(&ssh_directory()?, &name))
        .await
        .map_err(|error| format!("Generating the SSH key stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn ssh_keys_install_command(
    state: State<'_, AppState>,
    profile_id: String,
    public_path: String,
) -> Result<String, String> {
    let profile = crate::profiles::get_profile(&state, &profile_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        install_command(&profile, Path::new(&public_path), &ssh_directory()?)
    })
    .await
    .map_err(|error| format!("Preparing key installation stopped unexpectedly: {error}"))?
}

#[cfg(test)]
#[cfg_attr(windows, allow(unused_imports, dead_code))]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn generates_lists_and_prepares_installation_without_shell_interpolation() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join(".ssh");
        assert!(generate_key(&directory, "bad name").is_err());
        let key = generate_key(&directory, "nas").unwrap();
        assert_eq!(key.name, "weave_nas");
        assert!(key.fingerprint.starts_with("SHA256:"));
        assert!(key.has_private);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(directory.join("weave_nas"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
        assert_eq!(list_keys(&directory).unwrap().len(), 1);
        assert!(generate_key(&directory, "nas").is_err());
        let profile = Profile {
            id: "one".into(),
            name: "NAS".into(),
            host: "nas.example.com".into(),
            protocol: "ssh".into(),
            port: 2222,
            username: "alice".into(),
            key_path: Some("/tmp/old key".into()),
            jump_host: Some("bob@jump.example.com".into()),
            color: "#938cff".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let command = install_command(&profile, Path::new(&key.public_path), &directory).unwrap();
        assert!(command.contains("'ProxyJump=bob@jump.example.com'"));
        assert!(command.contains("'IdentityFile=/tmp/old key'"));
        assert!(command.contains("'alice@nas.example.com'"));
        assert!(command.contains("'2222'"));
        assert!(install_command(&profile, Path::new("/tmp/other.pub"), &directory).is_err());
        assert_eq!(quote("a'b"), "'a'\\''b'");
    }
}
