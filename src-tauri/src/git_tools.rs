use crate::git::run;
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Submodule {
    pub name: String,
    pub path: String,
    pub url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitToolsInfo {
    pub submodules: Vec<Submodule>,
    pub sparse_enabled: bool,
    pub sparse_cone: bool,
    pub sparse_directories: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSigningInfo {
    pub enabled: bool,
    pub format: String,
    pub key: String,
    pub head_signature: String,
    pub head_signer: String,
    pub allowed_signers_file: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSignatureEntry {
    pub commit: String,
    pub subject: String,
    pub status: String,
    pub signer: String,
}

fn signature_status(code: &str) -> &'static str {
    match code {
        "G" => "Valid signature",
        "U" => "Valid signature, untrusted key",
        "B" => "Invalid signature",
        "E" => "Could not verify signature",
        "X" | "Y" | "R" => "Expired or revoked signature",
        "N" => "Unsigned commit",
        _ => "Signature status unavailable",
    }
}

#[tauri::command]
pub fn git_signing_audit(path: String, skip: usize) -> Result<Vec<GitSignatureEntry>, String> {
    if skip > 1_000_000 {
        return Err(
            "Signature history offset is too large. Refresh the audit and try again.".into(),
        );
    }
    let path = repository_root(&path)?;
    if run(&path, &["rev-list", "--all", "--max-count=1"])?
        .trim()
        .is_empty()
    {
        return Ok(Vec::new());
    }
    let skip_arg = format!("--skip={skip}");
    let output = run(
        &path,
        &[
            "log",
            "--all",
            "--max-count=50",
            &skip_arg,
            "--format=%H%x1f%s%x1f%G?%x1f%GS%x1e",
        ],
    )?;
    let mut entries = Vec::new();
    for record in output
        .split('\x1e')
        .filter(|record| !record.trim().is_empty())
    {
        let fields: Vec<_> = record.trim_start_matches('\n').splitn(4, '\x1f').collect();
        if fields.len() != 4 {
            return Err("Git returned a signature audit entry that Weave could not read.".into());
        }
        entries.push(GitSignatureEntry {
            commit: fields[0].to_owned(),
            subject: fields[1].to_owned(),
            status: signature_status(fields[2]).to_owned(),
            signer: fields[3].trim_end_matches('\n').to_owned(),
        });
    }
    Ok(entries)
}

#[tauri::command]
pub fn git_signing_info(path: String) -> Result<GitSigningInfo, String> {
    let path = repository_root(&path)?;
    let enabled = run(&path, &["config", "--bool", "commit.gpgsign"])
        .is_ok_and(|value| value.trim() == "true");
    let format = run(&path, &["config", "--get", "gpg.format"])
        .unwrap_or_else(|_| "openpgp".into())
        .trim()
        .to_owned();
    let key = run(&path, &["config", "--get", "user.signingkey"])
        .unwrap_or_default()
        .trim()
        .to_owned();
    let allowed_signers_file = run(&path, &["config", "--get", "gpg.ssh.allowedSignersFile"])
        .unwrap_or_default()
        .trim()
        .to_owned();
    let (head_signature, head_signer) = if run(&path, &["rev-parse", "--verify", "HEAD"]).is_ok() {
        let output = run(&path, &["log", "-1", "--format=%G?%n%GS"])?;
        let mut lines = output.lines();
        let status = signature_status(lines.next().unwrap_or("N"));
        (status.to_owned(), lines.next().unwrap_or("").to_owned())
    } else {
        ("No commits yet".into(), String::new())
    };
    Ok(GitSigningInfo {
        enabled,
        format,
        key,
        head_signature,
        head_signer,
        allowed_signers_file,
    })
}

#[tauri::command]
pub fn git_signing_trust_add(
    path: String,
    principal: String,
    public_key_path: String,
) -> Result<GitSigningInfo, String> {
    let path = repository_root(&path)?;
    let principal = principal.trim();
    if principal.is_empty()
        || principal.len() > 254
        || !principal
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"@._+-".contains(&c))
    {
        return Err("Enter one email address or account name without spaces or wildcards.".into());
    }
    let key_path = Path::new(public_key_path.trim());
    if !key_path.is_absolute() || !key_path.is_file() {
        return Err("Choose an existing SSH public key file using its full path.".into());
    }
    let public_key = fs::read_to_string(key_path)
        .map_err(|error| format!("Could not read the SSH public key: {error}"))?;
    let public_key = public_key.trim();
    if public_key.lines().count() != 1 || public_key.len() > 8192 {
        return Err("The SSH public key file must contain exactly one key line.".into());
    }
    let checked = crate::platform::command("ssh-keygen")
        .args(["-l", "-f"])
        .arg(key_path)
        .output()
        .map_err(|error| {
            format!("Could not inspect the SSH public key with ssh-keygen: {error}")
        })?;
    if !checked.status.success() {
        return Err("ssh-keygen could not read a valid SSH public key from that file.".into());
    }

    let git_path = run(&path, &["rev-parse", "--git-path", "weave-allowed-signers"])?;
    let git_path = Path::new(git_path.trim());
    let trust_path = if git_path.is_absolute() {
        git_path.to_path_buf()
    } else {
        Path::new(&path).join(git_path)
    };
    let parent = trust_path
        .parent()
        .ok_or("Could not locate this repository's Git directory.")?;
    let parent = parent
        .canonicalize()
        .map_err(|error| format!("Could not open this repository's Git directory: {error}"))?;
    let trust_path = parent.join("weave-allowed-signers");
    let current =
        run(&path, &["config", "--get", "gpg.ssh.allowedSignersFile"]).unwrap_or_default();
    if !current.trim().is_empty() && Path::new(current.trim()) != trust_path {
        return Err("This repository already uses another SSH trusted signers file. Add the key there, or remove that Git setting before using Weave's managed file.".into());
    }
    if let Ok(metadata) = fs::symlink_metadata(&trust_path) {
        if !metadata.file_type().is_file() {
            return Err("The managed trusted signers path is not a regular file.".into());
        }
    }
    let entry = format!("{principal} {public_key}");
    let existing = if trust_path.exists() {
        fs::read_to_string(&trust_path)
            .map_err(|error| format!("Could not read trusted signers: {error}"))?
    } else {
        String::new()
    };
    if !existing.lines().any(|line| line == entry) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&trust_path)
            .map_err(|error| format!("Could not update trusted signers: {error}"))?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            writeln!(file).map_err(|error| format!("Could not update trusted signers: {error}"))?;
        }
        writeln!(file, "{entry}")
            .map_err(|error| format!("Could not update trusted signers: {error}"))?;
    }
    run(
        &path,
        &[
            "config",
            "--local",
            "gpg.ssh.allowedSignersFile",
            trust_path
                .to_str()
                .ok_or("The trust file path is not valid UTF-8.")?,
        ],
    )?;
    git_signing_info(path)
}

#[tauri::command]
pub fn git_signing_save(
    path: String,
    enabled: bool,
    format: String,
    key: String,
) -> Result<GitSigningInfo, String> {
    let path = repository_root(&path)?;
    let key = key.trim();
    if format != "openpgp" && format != "ssh" {
        return Err("Choose OpenPGP or SSH as the commit signing format.".into());
    }
    if enabled && key.is_empty() {
        return Err(
            "Enter a signing key or SSH public key file path before enabling signing.".into(),
        );
    }
    if key.starts_with('-') || key.contains(['\n', '\r', '\0']) {
        return Err("The signing key must not start with a dash or contain a line break.".into());
    }
    run(&path, &["config", "--local", "gpg.format", &format])?;
    if !key.is_empty() {
        run(&path, &["config", "--local", "user.signingkey", key])?;
    }
    run(
        &path,
        &[
            "config",
            "--local",
            "commit.gpgsign",
            if enabled { "true" } else { "false" },
        ],
    )?;
    git_signing_info(path)
}

fn generate_signing_key_in(
    path: String,
    principal: String,
    ssh_dir: &Path,
) -> Result<GitSigningInfo, String> {
    let root = repository_root(&path)?;
    let principal = principal.trim().to_owned();
    if principal.is_empty()
        || principal.len() > 254
        || !principal
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"@._+-".contains(&c))
    {
        return Err("Enter one email address or account name without spaces or wildcards.".into());
    }
    fs::create_dir_all(ssh_dir)
        .map_err(|error| format!("Could not create {}: {error}", ssh_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(ssh_dir, fs::Permissions::from_mode(0o700));
    }
    let key = (0..100)
        .map(|index| {
            ssh_dir.join(if index == 0 {
                "weave_signing_ed25519".to_owned()
            } else {
                format!("weave_signing_ed25519_{index}")
            })
        })
        .find(|candidate| !candidate.exists() && !candidate.with_extension("pub").exists())
        .ok_or("Too many Weave signing keys exist. Remove unused ones from ~/.ssh.")?;
    let output = crate::platform::command("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", &principal, "-f"])
        .arg(&key)
        .output()
        .map_err(|error| format!("Could not run ssh-keygen: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "ssh-keygen could not create a key: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let public = format!("{}.pub", key.display());
    git_signing_save(root.clone(), true, "ssh".into(), public.clone())?;
    git_signing_trust_add(root, principal, public)
}

/// Creates an Ed25519 SSH signing key in ~/.ssh, enables SSH commit signing
/// for this repository, and trusts the new key for verification.
#[tauri::command]
pub fn git_signing_generate_key(path: String, principal: String) -> Result<GitSigningInfo, String> {
    let ssh_dir = dirs::home_dir()
        .ok_or("Your home folder is unavailable.")?
        .join(".ssh");
    generate_signing_key_in(path, principal, &ssh_dir)
}

fn valid_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('-')
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn repository_root(path: &str) -> Result<String, String> {
    Ok(run(path, &["rev-parse", "--show-toplevel"])?
        .trim()
        .to_owned())
}

#[tauri::command]
pub fn git_tools_info(path: String) -> Result<GitToolsInfo, String> {
    let path = repository_root(&path)?;
    let mut submodules = Vec::new();
    let modules_file = Path::new(&path).join(".gitmodules");
    if let Ok(metadata) = fs::symlink_metadata(&modules_file) {
        if !metadata.file_type().is_file() {
            return Err(".gitmodules must be a regular file inside this repository.".into());
        }
        let entries = run(
            &path,
            &[
                "config",
                "--no-includes",
                "--file",
                ".gitmodules",
                "--null",
                "--list",
            ],
        )?;
        let values: Vec<(&str, &str)> = entries
            .split('\0')
            .filter_map(|entry| entry.split_once('\n'))
            .collect();
        for (key, submodule_path) in &values {
            if let Some(name) = key
                .strip_prefix("submodule.")
                .and_then(|key| key.strip_suffix(".path"))
            {
                let url_key = format!("submodule.{name}.url");
                let url = values
                    .iter()
                    .find(|(key, _)| *key == url_key)
                    .map(|(_, value)| *value)
                    .unwrap_or("");
                submodules.push(Submodule {
                    name: name.to_owned(),
                    path: (*submodule_path).to_owned(),
                    url: url.to_owned(),
                });
            }
        }
    }
    submodules.sort_by(|left, right| left.path.cmp(&right.path));
    let sparse_enabled = run(&path, &["config", "--bool", "core.sparseCheckout"])
        .is_ok_and(|value| value.trim() == "true");
    let sparse_cone = run(&path, &["config", "--bool", "core.sparseCheckoutCone"])
        .is_ok_and(|value| value.trim() == "true");
    let sparse_directories = if sparse_enabled {
        run(&path, &["sparse-checkout", "list"])?
            .lines()
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    Ok(GitToolsInfo {
        submodules,
        sparse_enabled,
        sparse_cone,
        sparse_directories,
    })
}

#[tauri::command]
pub async fn git_submodule_add(
    path: String,
    url: String,
    directory: String,
) -> Result<GitToolsInfo, String> {
    tauri::async_runtime::spawn_blocking(move || add_submodule(path, url, directory))
        .await
        .map_err(|error| format!("Submodule add stopped unexpectedly: {error}"))?
}

fn add_submodule(path: String, url: String, directory: String) -> Result<GitToolsInfo, String> {
    let path = repository_root(&path)?;
    let url = url.trim();
    let directory = directory.trim();
    if !valid_relative_path(directory) {
        return Err(
            "Enter a submodule folder relative to the repository, without '..' or a leading dash."
                .into(),
        );
    }
    if url.is_empty() || url.starts_with('-') || url.chars().any(char::is_whitespace) {
        return Err(
            "Enter a valid Git repository URL without spaces or embedded credentials.".into(),
        );
    }
    if let Ok(parsed) = url::Url::parse(url) {
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("Remove the username and password from the submodule URL. Use Git credentials or SSH instead.".into());
        }
    }
    if url.starts_with("file://") {
        run(
            &path,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--",
                url,
                directory,
            ],
        )?;
    } else {
        run(&path, &["submodule", "add", "--", url, directory])?;
    }
    git_tools_info(path)
}

#[tauri::command]
pub async fn git_submodules_update(path: String) -> Result<GitToolsInfo, String> {
    tauri::async_runtime::spawn_blocking(move || update_submodules(path))
        .await
        .map_err(|error| format!("Submodule update stopped unexpectedly: {error}"))?
}

fn update_submodules(path: String) -> Result<GitToolsInfo, String> {
    let path = repository_root(&path)?;
    run(&path, &["submodule", "sync", "--recursive"])?;
    run(&path, &["submodule", "update", "--init", "--recursive"])?;
    git_tools_info(path)
}

#[tauri::command]
pub async fn git_submodule_remove(path: String, directory: String) -> Result<GitToolsInfo, String> {
    tauri::async_runtime::spawn_blocking(move || remove_submodule(path, directory))
        .await
        .map_err(|error| format!("Submodule removal stopped unexpectedly: {error}"))?
}

fn remove_submodule(path: String, directory: String) -> Result<GitToolsInfo, String> {
    let path = repository_root(&path)?;
    let directory = directory.trim();
    if !valid_relative_path(directory)
        || !git_tools_info(path.clone())?
            .submodules
            .iter()
            .any(|submodule| submodule.path == directory)
    {
        return Err("Choose a listed submodule to remove.".into());
    }
    if !run(
        &path,
        &["status", "--porcelain", "--", ".gitmodules", directory],
    )?
    .is_empty()
    {
        return Err(
            "Commit or stash changes in .gitmodules and the submodule before removing it.".into(),
        );
    }
    let checkout = Path::new(&path).join(directory);
    if checkout.join(".git").exists()
        && !run(
            checkout.to_string_lossy().as_ref(),
            &["status", "--porcelain"],
        )?
        .is_empty()
    {
        return Err(
            "Commit, stash, or remove untracked files inside the submodule before removing it."
                .into(),
        );
    }
    run(&path, &["rm", "--", directory])?;
    git_tools_info(path)
}

#[tauri::command]
pub fn git_sparse_set(path: String, directories: Vec<String>) -> Result<GitToolsInfo, String> {
    let path = repository_root(&path)?;
    if directories.is_empty() {
        return Err("Choose at least one repository directory for sparse checkout.".into());
    }
    if directories
        .iter()
        .any(|directory| !valid_relative_path(directory))
    {
        return Err(
            "Sparse checkout folders must be relative paths without '..' or a leading dash.".into(),
        );
    }
    if !run(&path, &["status", "--porcelain"])?.is_empty() {
        return Err(
            "Commit or stash local changes before changing sparse checkout folders.".into(),
        );
    }
    let mut args = vec!["sparse-checkout", "set", "--cone", "--"];
    args.extend(directories.iter().map(String::as_str));
    run(&path, &args)?;
    git_tools_info(path)
}

#[tauri::command]
pub fn git_sparse_set_patterns(
    path: String,
    patterns: Vec<String>,
) -> Result<GitToolsInfo, String> {
    let path = repository_root(&path)?;
    if patterns.is_empty() || patterns.len() > 500 {
        return Err("Enter between 1 and 500 sparse checkout patterns.".into());
    }
    if patterns.iter().any(|pattern| {
        pattern.is_empty()
            || pattern.starts_with('-')
            || pattern.contains(['\n', '\r', '\0'])
            || pattern.len() > 4096
    }) {
        return Err("Each sparse pattern must be a single line, at most 4096 characters, and cannot begin with a dash.".into());
    }
    if !run(&path, &["status", "--porcelain"])?.is_empty() {
        return Err(
            "Commit or stash local changes before changing sparse checkout patterns.".into(),
        );
    }
    let mut args = vec!["sparse-checkout", "set", "--no-cone", "--"];
    args.extend(patterns.iter().map(String::as_str));
    run(&path, &args)?;
    git_tools_info(path)
}

#[tauri::command]
pub fn git_sparse_disable(path: String) -> Result<GitToolsInfo, String> {
    let path = repository_root(&path)?;
    if !run(&path, &["status", "--porcelain"])?.is_empty() {
        return Err("Commit or stash local changes before disabling sparse checkout.".into());
    }
    run(&path, &["sparse-checkout", "disable"])?;
    git_tools_info(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manages_submodules_and_sparse_checkout() {
        let root = std::env::temp_dir().join(format!("weave-git-tools-{}", uuid::Uuid::new_v4()));
        let parent = root.join("parent");
        let dependency = root.join("dependency");
        fs::create_dir_all(&parent).unwrap();
        fs::create_dir_all(&dependency).unwrap();
        let path = parent.to_string_lossy().to_string();
        let dependency_path = dependency.to_string_lossy().to_string();
        for repo in [&path, &dependency_path] {
            run(repo, &["init"]).unwrap();
            run(repo, &["config", "user.name", "Weave Test"]).unwrap();
            run(repo, &["config", "user.email", "weave@example.test"]).unwrap();
        }
        fs::create_dir(parent.join("app")).unwrap();
        fs::create_dir(parent.join("docs")).unwrap();
        fs::write(parent.join("app/main.txt"), "app\n").unwrap();
        fs::write(parent.join("docs/readme.txt"), "docs\n").unwrap();
        fs::write(dependency.join("lib.txt"), "library\n").unwrap();
        for repo in [&path, &dependency_path] {
            run(repo, &["add", "."]).unwrap();
            run(repo, &["commit", "-m", "Initial"]).unwrap();
        }
        let url = format!("file://{dependency_path}");
        assert!(add_submodule(path.clone(), url.clone(), "../escape".into()).is_err());
        let info = add_submodule(path.clone(), url, "vendor/lib".into()).unwrap();
        assert_eq!(info.submodules.len(), 1);
        assert_eq!(info.submodules[0].path, "vendor/lib");
        assert!(parent.join("vendor/lib/lib.txt").exists());
        run(&path, &["commit", "-am", "Add dependency"]).unwrap();
        let info = git_sparse_set(path.clone(), vec!["app".into(), "vendor".into()]).unwrap();
        assert!(info.sparse_enabled);
        assert!(info.sparse_directories.contains(&"app".into()));
        assert!(!parent.join("docs/readme.txt").exists());
        assert!(git_sparse_set(path.clone(), vec!["../escape".into()]).is_err());
        let info = git_sparse_disable(path.clone()).unwrap();
        assert!(!info.sparse_enabled);
        assert!(parent.join("docs/readme.txt").exists());
        assert!(git_sparse_set_patterns(path.clone(), vec!["--bad".into()]).is_err());
        let info = git_sparse_set_patterns(path.clone(), vec!["/app/".into()]).unwrap();
        assert!(info.sparse_enabled);
        assert!(!info.sparse_cone);
        assert!(parent.join("app/main.txt").exists());
        assert!(!parent.join("docs/readme.txt").exists());
        git_sparse_disable(path.clone()).unwrap();
        update_submodules(path.clone()).unwrap();
        assert!(remove_submodule(path.clone(), "not-listed".into()).is_err());
        fs::write(parent.join("vendor/lib/local.txt"), "Do not delete\n").unwrap();
        assert!(remove_submodule(path.clone(), "vendor/lib".into()).is_err());
        assert!(parent.join("vendor/lib/local.txt").exists());
        fs::remove_file(parent.join("vendor/lib/local.txt")).unwrap();
        let info = remove_submodule(path.clone(), "vendor/lib".into()).unwrap();
        assert!(info.submodules.is_empty());
        assert!(!parent.join("vendor/lib").exists());
        assert!(run(&path, &["diff", "--cached", "--name-only"])
            .unwrap()
            .contains(".gitmodules"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configures_signing_and_reports_unsigned_commits() {
        let root = std::env::temp_dir().join(format!("weave-signing-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        assert_eq!(
            git_signing_info(path.clone()).unwrap().head_signature,
            "No commits yet"
        );
        assert!(git_signing_audit(path.clone(), 0).unwrap().is_empty());
        assert!(git_signing_save(path.clone(), true, "ssh".into(), "".into()).is_err());
        assert!(git_signing_save(path.clone(), true, "invalid".into(), "key".into()).is_err());
        let info = git_signing_save(
            path.clone(),
            false,
            "ssh".into(),
            "~/.ssh/id_ed25519.pub".into(),
        )
        .unwrap();
        assert_eq!(info.format, "ssh");
        assert!(!info.enabled);
        fs::write(root.join("readme.txt"), "hello\n").unwrap();
        run(&path, &["add", "."]).unwrap();
        run(&path, &["commit", "-m", "Unsigned"]).unwrap();
        assert_eq!(
            git_signing_info(path.clone()).unwrap().head_signature,
            "Unsigned commit"
        );
        let first = git_signing_audit(path.clone(), 0).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].subject, "Unsigned");
        assert_eq!(first[0].status, "Unsigned commit");
        assert!(first[0].signer.is_empty());
        assert!(git_signing_audit(path.clone(), 1).unwrap().is_empty());
        assert!(git_signing_audit(path, 1_000_001).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generates_a_signing_key_and_signs_verified_commits() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let ssh_dir = root.path().join("ssh");
        fs::create_dir(&repo).unwrap();
        let path = repo.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        assert!(generate_signing_key_in(path.clone(), "bad name".into(), &ssh_dir).is_err());
        let info =
            generate_signing_key_in(path.clone(), "weave@example.test".into(), &ssh_dir).unwrap();
        assert!(info.enabled);
        assert_eq!(info.format, "ssh");
        assert!(ssh_dir.join("weave_signing_ed25519").is_file());
        assert!(info.key.ends_with("weave_signing_ed25519.pub"));
        fs::write(repo.join("signed.txt"), "signed\n").unwrap();
        run(&path, &["add", "."]).unwrap();
        run(&path, &["commit", "-m", "Signed"]).unwrap();
        assert_eq!(
            git_signing_info(path.clone()).unwrap().head_signature,
            "Valid signature"
        );
        // A second key never overwrites the first.
        generate_signing_key_in(path, "weave@example.test".into(), &ssh_dir).unwrap();
        assert!(ssh_dir.join("weave_signing_ed25519_1").is_file());
    }

    #[test]
    fn signature_status_explains_git_verification_codes() {
        assert_eq!(signature_status("G"), "Valid signature");
        assert_eq!(signature_status("U"), "Valid signature, untrusted key");
        assert_eq!(signature_status("B"), "Invalid signature");
        assert_eq!(signature_status("E"), "Could not verify signature");
        assert_eq!(signature_status("R"), "Expired or revoked signature");
        assert_eq!(signature_status("N"), "Unsigned commit");
    }

    #[test]
    fn audits_trusted_ssh_signed_commit() {
        let root =
            std::env::temp_dir().join(format!("weave-signed-audit-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        let key = root.join("signing-key");
        let output = crate::platform::command("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .output()
            .unwrap();
        assert!(output.status.success(), "ssh-keygen failed");
        let public = key.with_extension("pub");
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        run(&path, &["config", "gpg.format", "ssh"]).unwrap();
        run(
            &path,
            &["config", "user.signingkey", public.to_str().unwrap()],
        )
        .unwrap();
        assert!(git_signing_trust_add(
            path.clone(),
            "bad principal".into(),
            public.to_string_lossy().to_string()
        )
        .is_err());
        let info = git_signing_trust_add(
            path.clone(),
            "weave@example.test".into(),
            public.to_string_lossy().to_string(),
        )
        .unwrap();
        let trusted = fs::read_to_string(&info.allowed_signers_file).unwrap();
        assert!(trusted.starts_with("weave@example.test ssh-ed25519 "));
        git_signing_trust_add(
            path.clone(),
            "weave@example.test".into(),
            public.to_string_lossy().to_string(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&info.allowed_signers_file).unwrap(),
            trusted
        );
        fs::write(root.join("readme.txt"), "hello\n").unwrap();
        run(&path, &["add", "readme.txt"]).unwrap();
        run(&path, &["commit", "-S", "-m", "Signed"]).unwrap();
        let audit = git_signing_audit(path.clone(), 0).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].status, "Valid signature");
        assert_eq!(audit[0].signer, "weave@example.test");
        fs::remove_dir_all(root).unwrap();
    }
}
