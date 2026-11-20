use crate::git::run;
use serde::Serialize;
use std::{
    collections::HashSet,
    fs,
    io::ErrorKind,
    path::{Component, Path},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflogEntry {
    pub selector: String,
    pub commit: String,
    pub action: String,
}

#[tauri::command]
pub fn git_reflog(path: String) -> Result<Vec<ReflogEntry>, String> {
    run(&path, &["rev-parse", "--show-toplevel"])?;
    if run(&path, &["rev-parse", "--verify", "HEAD"]).is_err() {
        return Ok(Vec::new());
    }
    let output = run(
        &path,
        &["reflog", "show", "-n", "40", "--format=%gd%x1f%H%x1f%gs"],
    )?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\u{1f}');
            let selector = fields.next()?;
            let commit = fields.next()?;
            let action = fields.next()?;
            Some(ReflogEntry {
                selector: selector.into(),
                commit: commit.into(),
                action: action.into(),
            })
        })
        .collect())
}

pub(crate) fn operation_in_progress(path: &str) -> Result<bool, String> {
    for marker in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ] {
        let location = run(path, &["rev-parse", "--git-path", marker])?;
        let location = Path::new(location.trim());
        let location = if location.is_absolute() {
            location.to_path_buf()
        } else {
            Path::new(path).join(location)
        };
        if location.exists() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn protect_untracked_files(path: &str, commit: &str) -> Result<(), String> {
    let root = run(path, &["rev-parse", "--show-toplevel"])?;
    let root = Path::new(root.trim());
    let tracked = run(path, &["ls-files", "--cached", "-z"])?;
    let tracked: HashSet<&str> = tracked.split('\0').collect();
    let target_files = run(path, &["ls-tree", "-r", "-z", "--name-only", commit])?;
    if target_files.contains('\u{fffd}') || tracked.iter().any(|name| name.contains('\u{fffd}')) {
        return Err("This repository has a filename that cannot be checked safely as UTF-8. Use Git in Terminal to restore it.".into());
    }
    for file in target_files.split('\0').filter(|file| !file.is_empty()) {
        let mut candidate = root.to_path_buf();
        let components: Vec<_> = Path::new(file).components().collect();
        if components
            .iter()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(
                "Git returned a path outside this repository. Restoration was stopped.".into(),
            );
        }
        for part in components.iter().take(components.len().saturating_sub(1)) {
            candidate.push(part.as_os_str());
            match fs::symlink_metadata(&candidate) {
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(format!(
                        "A file or symbolic link blocks restoring {file}. Move it first."
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!("Could not check the path for {file}: {error}"));
                }
            }
        }
        candidate.push(components.last().unwrap().as_os_str());
        if !tracked.contains(file) {
            match fs::symlink_metadata(&candidate) {
                Ok(_) => {
                    return Err(format!(
                        "An untracked or ignored file would be overwritten: {file}. Move it first."
                    ));
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!("Could not check the path for {file}: {error}"));
                }
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn git_reflog_restore(path: String, commit: String, confirmed: bool) -> Result<String, String> {
    if !confirmed {
        return Err("Confirm the branch restoration before applying it.".into());
    }
    let current_branch = run(&path, &["branch", "--show-current"])?;
    if current_branch.trim().is_empty() {
        return Err("Check out a branch before restoring a history state.".into());
    }
    if !run(&path, &["status", "--porcelain"])?.is_empty() {
        return Err("Commit or stash local changes before restoring branch history.".into());
    }
    if operation_in_progress(&path)? {
        return Err("Finish or abort the current Git operation before restoring history.".into());
    }
    if !git_reflog(path.clone())?
        .iter()
        .any(|entry| entry.commit == commit)
    {
        return Err("That history state is no longer in the recent reflog. Refresh and choose another entry.".into());
    }
    let current = run(&path, &["rev-parse", "HEAD"])?;
    if current.trim() == commit {
        return Err("The branch already points to this commit.".into());
    }
    protect_untracked_files(&path, &commit)?;
    let identifier = uuid::Uuid::new_v4().simple().to_string();
    let recovery = format!("weave/recovery-{}", &identifier[..12]);
    run(&path, &["branch", &recovery, "HEAD"])?;
    run(&path, &["reset", "--hard", &commit]).map_err(|error| {
        format!(
            "Could not restore the branch: {error}. Your previous tip is preserved in {recovery}."
        )
    })?;
    Ok(recovery)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn restores_history_while_preserving_the_previous_tip() {
        let root = std::env::temp_dir().join(format!("weave-recovery-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        assert!(git_reflog(path.clone()).unwrap().is_empty());
        fs::write(root.join("data.txt"), "first\n").unwrap();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("nested/data.txt"), "nested first\n").unwrap();
        run(&path, &["add", "."]).unwrap();
        run(&path, &["commit", "-m", "First"]).unwrap();
        let first = run(&path, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_owned();
        fs::write(root.join("data.txt"), "second\n").unwrap();
        run(&path, &["commit", "-am", "Second"]).unwrap();
        let second = run(&path, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_owned();
        assert!(git_reflog(path.clone())
            .unwrap()
            .iter()
            .any(|entry| entry.commit == first));
        assert!(git_reflog_restore(path.clone(), first.clone(), false).is_err());
        fs::write(root.join("pending.txt"), "keep").unwrap();
        assert!(git_reflog_restore(path.clone(), first.clone(), true).is_err());
        fs::remove_file(root.join("pending.txt")).unwrap();
        let recovery = git_reflog_restore(path.clone(), first.clone(), true).unwrap();
        assert_eq!(run(&path, &["rev-parse", "HEAD"]).unwrap().trim(), first);
        assert_eq!(
            run(&path, &["rev-parse", &recovery]).unwrap().trim(),
            second
        );
        assert_eq!(
            fs::read_to_string(root.join("data.txt")).unwrap(),
            "first\n"
        );
        run(&path, &["rm", "data.txt"]).unwrap();
        fs::write(root.join(".gitignore"), "data.txt\n").unwrap();
        run(&path, &["add", ".gitignore"]).unwrap();
        run(&path, &["commit", "-m", "Remove tracked file"]).unwrap();
        fs::write(root.join("data.txt"), "ignored private data\n").unwrap();
        assert!(git_reflog_restore(path.clone(), first.clone(), true)
            .unwrap_err()
            .contains("would be overwritten"));
        assert_eq!(
            fs::read_to_string(root.join("data.txt")).unwrap(),
            "ignored private data\n"
        );
        fs::remove_file(root.join("data.txt")).unwrap();
        run(&path, &["rm", "nested/data.txt"]).unwrap();
        if root.join("nested").exists() {
            fs::remove_dir(root.join("nested")).unwrap();
        }
        fs::write(root.join(".gitignore"), "data.txt\nnested\n").unwrap();
        run(&path, &["commit", "-am", "Remove nested file"]).unwrap();
        fs::write(root.join("nested"), "ignored private data\n").unwrap();
        assert!(git_reflog_restore(path.clone(), first, true)
            .unwrap_err()
            .contains("blocks restoring nested/data.txt"));
        assert_eq!(
            fs::read_to_string(root.join("nested")).unwrap(),
            "ignored private data\n"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
