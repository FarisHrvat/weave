//! OpenPGP keys for commit signing, managed through GnuPG: create a signing
//! key, import other people's public keys, and certify them locally so Git
//! reports their signatures as trusted.

use crate::{git::run, git_tools, remote_access::find_tool};
use serde::Serialize;
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
};

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenPgpKey {
    pub fingerprint: String,
    pub user_id: String,
    pub validity: String,
    pub secret: bool,
}

fn gpg_path() -> Result<PathBuf, String> {
    find_tool("gpg").ok_or_else(|| {
        if cfg!(target_os = "macos") {
            "GnuPG is not installed. Install it with `brew install gnupg`, then try again."
        } else if cfg!(windows) {
            "GnuPG is not installed. Install Gpg4win from gpg4win.org, then try again."
        } else {
            "GnuPG is not installed. Install the gnupg package, then try again."
        }
        .into()
    })
}

fn gpg(home: Option<&Path>, args: &[&str], input: Option<&str>) -> Result<String, String> {
    let mut command = crate::platform::command(gpg_path()?);
    command.arg("--batch").args(args);
    if let Some(home) = home {
        command.env("GNUPGHOME", home);
    }
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start GnuPG: {error}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin
            .write_all(text.as_bytes())
            .map_err(|error| format!("Could not send data to GnuPG: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("GnuPG stopped unexpectedly: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let line = detail
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("no details")
            .trim_start_matches("gpg: ");
        return Err(format!("GnuPG could not finish: {line}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn validity_label(code: &str) -> &'static str {
    match code {
        "u" => "Your key",
        "f" => "Trusted",
        "m" => "Partly trusted",
        "r" => "Revoked",
        "e" => "Expired",
        "n" => "Not trusted",
        _ => "Not verified",
    }
}

pub(crate) fn list_keys(home: Option<&Path>) -> Result<Vec<OpenPgpKey>, String> {
    let secret = gpg(home, &["--with-colons", "--list-secret-keys"], None).unwrap_or_default();
    let secret: Vec<&str> = secret
        .lines()
        .filter_map(|line| line.strip_prefix("fpr:"))
        .map(|rest| rest.trim_matches(':'))
        .collect();
    let output = gpg(home, &["--with-colons", "--list-keys"], None)?;
    let mut keys: Vec<OpenPgpKey> = Vec::new();
    for line in output.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        match fields.first().copied() {
            Some("pub") => {
                keys.push(OpenPgpKey {
                    fingerprint: String::new(),
                    user_id: String::new(),
                    validity: validity_label(fields.get(1).unwrap_or(&"")).into(),
                    secret: false,
                });
            }
            Some("fpr") => {
                if let Some(key) = keys.last_mut().filter(|key| key.fingerprint.is_empty()) {
                    key.fingerprint = fields.get(9).unwrap_or(&"").to_string();
                    key.secret = secret.contains(&key.fingerprint.as_str());
                }
            }
            Some("uid") => {
                if let Some(key) = keys.last_mut().filter(|key| key.user_id.is_empty()) {
                    key.user_id = fields.get(9).unwrap_or(&"").replace("\\x3a", ":");
                }
            }
            _ => {}
        }
    }
    Ok(keys)
}

fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 100 && !name.contains(['<', '>', '\n', '\r', '\0'])
}

fn valid_email(email: &str) -> bool {
    let parts: Vec<&str> = email.split('@').collect();
    parts.len() == 2
        && !parts[0].is_empty()
        && parts[1].contains('.')
        && email.len() <= 200
        && !email.contains(char::is_whitespace)
        && !email.contains(['<', '>'])
}

pub(crate) fn generate_key(
    home: Option<&Path>,
    repository: &str,
    name: &str,
    email: &str,
    passphrase: &str,
) -> Result<git_tools::GitSigningInfo, String> {
    let root = run(repository, &["rev-parse", "--show-toplevel"])?
        .trim()
        .to_owned();
    if !valid_name(name) {
        return Err("Enter your name without angle brackets.".into());
    }
    if !valid_email(email) {
        return Err("Enter a valid email address.".into());
    }
    let user_id = format!("{} <{}>", name.trim(), email.trim());
    gpg(
        home,
        &[
            "--pinentry-mode",
            "loopback",
            "--passphrase-fd",
            "0",
            "--quick-generate-key",
            &user_id,
            "ed25519",
            "sign",
            "2y",
        ],
        Some(passphrase),
    )?;
    let fingerprint = list_keys(home)?
        .into_iter()
        .rev()
        .find(|key| key.secret && key.user_id == user_id)
        .map(|key| key.fingerprint)
        .ok_or("GnuPG created the key but Weave could not find it.")?;
    let program = gpg_path()?;
    // Git started from Finder cannot find Homebrew's gpg without a full path.
    run(
        &root,
        &[
            "config",
            "--local",
            "gpg.program",
            &program.to_string_lossy(),
        ],
    )?;
    git_tools::git_signing_save(root, true, "openpgp".into(), fingerprint)
}

pub(crate) fn import_and_trust(home: Option<&Path>, key_file: &Path) -> Result<String, String> {
    if !key_file.is_file() {
        return Err("Choose an exported OpenPGP public key file.".into());
    }
    if !list_keys(home)?.iter().any(|key| key.secret) {
        return Err(
            "Create or import your own OpenPGP key first. Weave marks other keys as trusted by certifying them with yours.".into(),
        );
    }
    let before: Vec<String> = list_keys(home)?
        .into_iter()
        .map(|key| key.fingerprint)
        .collect();
    gpg(home, &["--import", &key_file.to_string_lossy()], None)?;
    let added: Vec<OpenPgpKey> = list_keys(home)?
        .into_iter()
        .filter(|key| !before.contains(&key.fingerprint))
        .collect();
    if added.is_empty() {
        return Err(
            "That key is already in your keyring. Use Trust on it in the list instead.".into(),
        );
    }
    for key in &added {
        trust(home, &key.fingerprint)?;
    }
    Ok(added
        .iter()
        .map(|key| key.user_id.clone())
        .collect::<Vec<_>>()
        .join(", "))
}

pub(crate) fn trust(home: Option<&Path>, fingerprint: &str) -> Result<(), String> {
    if fingerprint.len() != 40 || !fingerprint.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Choose a key from the list.".into());
    }
    gpg(home, &["--yes", "--quick-lsign-key", fingerprint], None).map(|_| ())
}

#[tauri::command]
pub async fn git_openpgp_keys() -> Result<Vec<OpenPgpKey>, String> {
    tauri::async_runtime::spawn_blocking(|| list_keys(None))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn git_openpgp_generate_key(
    path: String,
    name: String,
    email: String,
    passphrase: String,
) -> Result<git_tools::GitSigningInfo, String> {
    let passphrase = zeroize::Zeroizing::new(passphrase);
    tauri::async_runtime::spawn_blocking(move || {
        generate_key(None, &path, &name, &email, &passphrase)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn git_openpgp_import(key_file: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || import_and_trust(None, Path::new(&key_file)))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn git_openpgp_trust(fingerprint: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || trust(None, &fingerprint))
        .await
        .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyring() -> tempfile::TempDir {
        let home = tempfile::Builder::new()
            .prefix("wg")
            .tempdir_in("/tmp")
            .unwrap();
        crate::platform::create_private_dir(home.path()).unwrap();
        home
    }

    #[test]
    #[ignore = "requires GnuPG"]
    fn generates_a_key_signs_commits_and_trusts_imported_keys() {
        let home = keyring();
        let repo = tempfile::tempdir().unwrap();
        let path = repo.path().to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "me@example.test"]).unwrap();
        assert!(generate_key(Some(home.path()), &path, "Me", "not-an-email", "").is_err());
        let info = generate_key(
            Some(home.path()),
            &path,
            "Weave Test",
            "me@example.test",
            "",
        )
        .unwrap();
        assert!(info.enabled);
        assert_eq!(info.format, "openpgp");
        assert_eq!(info.key.len(), 40);

        std::fs::write(repo.path().join("a.txt"), "signed").unwrap();
        run(&path, &["add", "."]).unwrap();
        let commit = crate::platform::command("git")
            .args(["-C", &path, "commit", "-m", "Signed"])
            .env("GNUPGHOME", home.path())
            .output()
            .unwrap();
        assert!(
            commit.status.success(),
            "{}",
            String::from_utf8_lossy(&commit.stderr)
        );
        let status = crate::platform::command("git")
            .args(["-C", &path, "log", "-1", "--format=%G?"])
            .env("GNUPGHOME", home.path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "G");

        // A colleague's key, created in a separate keyring.
        let other = keyring();
        gpg(
            Some(other.path()),
            &[
                "--pinentry-mode",
                "loopback",
                "--passphrase",
                "",
                "--quick-generate-key",
                "Colleague <colleague@example.test>",
                "ed25519",
                "sign",
                "1y",
            ],
            None,
        )
        .unwrap();
        let exported = other.path().join("colleague.asc");
        gpg(
            Some(other.path()),
            &[
                "--armor",
                "--output",
                &exported.to_string_lossy(),
                "--export",
                "colleague@example.test",
            ],
            None,
        )
        .unwrap();
        assert_eq!(
            import_and_trust(Some(home.path()), &exported).unwrap(),
            "Colleague <colleague@example.test>"
        );
        let keys = list_keys(Some(home.path())).unwrap();
        let colleague = keys
            .iter()
            .find(|key| key.user_id.starts_with("Colleague"))
            .unwrap();
        assert_eq!(colleague.validity, "Trusted");
        assert!(!colleague.secret);
        assert!(import_and_trust(Some(home.path()), &exported)
            .unwrap_err()
            .contains("already"));
        assert!(trust(Some(home.path()), "nope").is_err());
    }

    #[test]
    fn validates_names_and_emails() {
        assert!(valid_name("Ada Lovelace"));
        assert!(!valid_name("Ada <x>"));
        assert!(valid_email("ada@example.test"));
        assert!(!valid_email("ada@localhost"));
        assert!(!valid_email("a da@example.test"));
    }
}
