use crate::git::{git_lfs_info, run, GitLfsInfo};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

#[derive(Debug, Deserialize)]
struct LockOwner {
    name: String,
}

#[derive(Debug, Deserialize)]
struct LockRecord {
    id: String,
    path: String,
    owner: Option<LockOwner>,
    locked_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LockResponse {
    #[serde(default)]
    locks: Vec<LockRecord>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LfsLock {
    pub id: String,
    pub path: String,
    pub owner: String,
    pub locked_at: String,
    pub ours: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LfsMigrationResult {
    pub recovery_branch: String,
    pub info: GitLfsInfo,
}

fn repository_root(path: &str) -> Result<String, String> {
    let root = run(path, &["rev-parse", "--show-toplevel"])?;
    git_lfs_info(root.trim().to_owned())?;
    Ok(root.trim().to_owned())
}

fn relative_file(path: &str) -> Result<&str, String> {
    let path = path.trim();
    if path.is_empty()
        || path.starts_with('-')
        || path.contains(['\n', '\r', '\0'])
        || !Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(
            "Choose a file path inside this repository, without '..' or a leading dash.".into(),
        );
    }
    Ok(path)
}

fn parse_locks(output: &str, ours: bool) -> Result<Vec<LfsLock>, String> {
    let response: LockResponse = serde_json::from_str(output)
        .map_err(|error| format!("Git LFS returned an unreadable lock list: {error}"))?;
    Ok(response
        .locks
        .into_iter()
        .map(|lock| LfsLock {
            id: lock.id,
            path: lock.path,
            owner: lock.owner.map_or_else(String::new, |owner| owner.name),
            locked_at: lock.locked_at.unwrap_or_default(),
            ours,
        })
        .collect())
}

#[tauri::command]
pub async fn git_lfs_locks(path: String, remote: bool) -> Result<Vec<LfsLock>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = repository_root(&path)?;
        if remote {
            let mut locks = parse_locks(&run(&path, &["lfs", "locks", "--json"])?, false)?;
            let local = parse_locks(&run(&path, &["lfs", "locks", "--local", "--json"])?, true)?;
            for lock in &mut locks {
                lock.ours = local.iter().any(|owned| owned.id == lock.id);
            }
            Ok(locks)
        } else {
            parse_locks(&run(&path, &["lfs", "locks", "--local", "--json"])?, true)
        }
    })
    .await
    .map_err(|error| format!("Git LFS lock listing stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn git_lfs_change_lock(path: String, file: String, lock: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = repository_root(&path)?;
        let file = relative_file(&file)?;
        if lock {
            if !Path::new(&path).join(file).is_file() {
                return Err("The file must exist in this checkout before it can be locked.".into());
            }
            run(&path, &["lfs", "lock", file])?;
        } else {
            run(&path, &["lfs", "unlock", file])?;
        }
        Ok(())
    })
    .await
    .map_err(|error| format!("Git LFS lock operation stopped unexpectedly: {error}"))?
}

fn migration_context(path: &str, pattern: &str) -> Result<(String, String, String), String> {
    let path = repository_root(path)?;
    let pattern = pattern.trim();
    if pattern.is_empty()
        || pattern.starts_with('-')
        || pattern.contains(['\n', '\r', '\0'])
        || pattern.len() > 1024
    {
        return Err(
            "Enter one LFS file pattern, such as *.psd, without line breaks or a leading dash."
                .into(),
        );
    }
    let branch = run(&path, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map_err(|_| "Check out a local branch before migrating Git history.".to_owned())?
        .trim()
        .to_owned();
    if !run(&path, &["status", "--porcelain"])?.is_empty() {
        return Err("Commit or stash every local change before migrating Git LFS history.".into());
    }
    Ok((path, branch, pattern.to_owned()))
}

#[tauri::command]
pub async fn git_lfs_migrate_preview(path: String, pattern: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (path, branch, pattern) = migration_context(&path, &pattern)?;
        let include = format!("--include={pattern}");
        let reference = format!("--include-ref=refs/heads/{branch}");
        run(
            &path,
            &[
                "lfs",
                "migrate",
                "info",
                "--skip-fetch",
                &include,
                &reference,
            ],
        )
    })
    .await
    .map_err(|error| format!("Git LFS migration preview stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn git_lfs_migrate_import(
    path: String,
    pattern: String,
) -> Result<LfsMigrationResult, String> {
    tauri::async_runtime::spawn_blocking(move || migrate_import(path, pattern))
        .await
        .map_err(|error| format!("Git LFS migration stopped unexpectedly: {error}"))?
}

fn migrate_import(path: String, pattern: String) -> Result<LfsMigrationResult, String> {
    let (path, branch, pattern) = migration_context(&path, &pattern)?;
    let recovery_branch = format!(
        "weave/lfs-recovery-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let original_tip = run(&path, &["rev-parse", "HEAD"])?.trim().to_owned();
    run(&path, &["branch", &recovery_branch, "HEAD"])?;
    let include = format!("--include={pattern}");
    let reference = format!("--include-ref=refs/heads/{branch}");
    let migration = run(
        &path,
        &[
            "lfs",
            "migrate",
            "import",
            "--skip-fetch",
            &include,
            &reference,
        ],
    );
    let recovery_ref = format!("refs/heads/{recovery_branch}");
    run(&path, &["update-ref", &recovery_ref, &original_tip]).map_err(|error| {
        format!("Could not preserve the original branch tip after Git LFS migration: {error}. Original commit: {original_tip}")
    })?;
    migration.map_err(|error| format!("Git LFS migration stopped: {error}. The original branch tip is saved as {recovery_branch}."))?;
    let info = git_lfs_info(path).map_err(|error| {
        format!("Git LFS migration finished, but the refreshed file list is unavailable: {error}. The original branch tip is saved as {recovery_branch}.")
    })?;
    Ok(LfsMigrationResult {
        recovery_branch,
        info,
    })
}

#[cfg(test)]
#[cfg_attr(windows, allow(unused_imports, dead_code))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_locks_and_checks_paths() {
        let locks = parse_locks(
            r#"{"locks":[{"id":"1","path":"art.psd","owner":{"name":"A"},"locked_at":"today"}]}"#,
            true,
        )
        .unwrap();
        assert_eq!(locks[0].owner, "A");
        assert!(locks[0].ours);
        assert!(relative_file("../outside").is_err());
        assert!(relative_file("--force").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn migrates_local_history_with_recovery_branch() {
        if run("/tmp", &["lfs", "version"]).is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!("weave-lfs-migrate-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        let local_locks = run(&path, &["lfs", "locks", "--local", "--json"]).unwrap();
        assert!(parse_locks(&local_locks, true).unwrap().is_empty());
        fs::write(root.join("art.bin"), vec![1u8; 4096]).unwrap();
        run(&path, &["add", "."]).unwrap();
        run(&path, &["commit", "-m", "Add binary"]).unwrap();
        let original = run(&path, &["rev-parse", "HEAD"]).unwrap();
        let (_, branch, pattern) = migration_context(&path, "*.bin").unwrap();
        let include = format!("--include={pattern}");
        let reference = format!("--include-ref=refs/heads/{branch}");
        run(
            &path,
            &[
                "lfs",
                "migrate",
                "info",
                "--skip-fetch",
                &include,
                &reference,
            ],
        )
        .unwrap();
        let result = migrate_import(path.clone(), pattern).unwrap();
        assert_ne!(run(&path, &["rev-parse", "HEAD"]).unwrap(), original);
        assert_eq!(
            run(&path, &["rev-parse", &result.recovery_branch]).unwrap(),
            original
        );
        assert!(run(&path, &["lfs", "ls-files", "--name-only"])
            .unwrap()
            .contains("art.bin"));
        fs::remove_dir_all(root).unwrap();
    }
}
