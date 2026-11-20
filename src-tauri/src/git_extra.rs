//! Further Git workflows: subtrees, external diff and merge tools, bisect,
//! patches, cleaning, stash options, annotated tags, notes, branch-filtered
//! history, custom commands and restoring deleted branches.

use crate::{
    forge,
    git::{commit_log_for, has_commits, run, run_authenticated, Commit},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

fn root(path: &str) -> Result<String, String> {
    Ok(run(path, &["rev-parse", "--show-toplevel"])?
        .trim()
        .to_owned())
}

fn check_ref(value: &str, what: &str) -> Result<(), String> {
    if value.is_empty() || value.starts_with('-') || value.contains(['\n', '\r', '\0', ' ']) {
        return Err(format!("Enter a valid {what}."));
    }
    Ok(())
}

fn check_paths(paths: &[String]) -> Result<(), String> {
    if paths
        .iter()
        .any(|path| path.is_empty() || path.starts_with('-') || path.contains('\0'))
    {
        return Err("One of the chosen paths is not valid.".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// History filtered to chosen branches

#[tauri::command]
pub fn git_commits_for(
    path: String,
    refs: Vec<String>,
    skip: usize,
) -> Result<Vec<Commit>, String> {
    root(&path)?;
    for reference in &refs {
        check_ref(reference, "branch")?;
    }
    if !has_commits(&path)? {
        return Ok(Vec::new());
    }
    commit_log_for(&path, skip, &refs)
}

// ---------------------------------------------------------------------------
// Subtrees

#[tauri::command]
pub async fn git_subtree(
    path: String,
    action: String,
    prefix: String,
    repository: String,
    branch: String,
    squash: bool,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = root(&path)?;
        let prefix = prefix.trim().trim_matches('/');
        if prefix.is_empty()
            || prefix.starts_with('-')
            || prefix
                .split('/')
                .any(|part| part == ".." || part.is_empty())
        {
            return Err(
                "Enter the subtree folder inside the repository, such as vendor/library.".into(),
            );
        }
        check_ref(repository.trim(), "repository URL or remote name")?;
        check_ref(branch.trim(), "branch")?;
        let verb = match action.as_str() {
            "add" | "pull" | "push" => action.as_str(),
            _ => return Err("Choose add, pull or push for the subtree.".into()),
        };
        if verb == "add" && Path::new(&root).join(prefix).exists() {
            return Err(format!(
                "{prefix} already exists. Choose an empty folder for the subtree."
            ));
        }
        if verb != "push"
            && !run(&root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty()
        {
            return Err("Commit or stash your changes before updating a subtree.".into());
        }
        let prefix_arg = format!("--prefix={prefix}");
        let mut args = vec!["subtree", verb, prefix_arg.as_str()];
        if squash && verb != "push" {
            args.push("--squash");
        }
        args.push(repository.trim());
        args.push(branch.trim());
        let auth = forge::git_auth_for_url(repository.trim()).ok().flatten();
        run_authenticated(&root, &args, auth.as_ref())?;
        Ok(match verb {
            "add" => format!("Added {} {} as {prefix}.", repository.trim(), branch.trim()),
            "pull" => format!("Updated {prefix} from {}.", branch.trim()),
            _ => format!(
                "Pushed {prefix} to {} {}.",
                repository.trim(),
                branch.trim()
            ),
        })
    })
    .await
    .map_err(|error| format!("The subtree command stopped unexpectedly: {error}"))?
}

// ---------------------------------------------------------------------------
// External diff and merge tools

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOption {
    id: String,
    name: String,
    installed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSettings {
    diff_tool: String,
    merge_tool: String,
    tools: Vec<ToolOption>,
}

/// (git tool name, label)
const TOOLS: [(&str, &str); 11] = [
    ("opendiff", "FileMerge (Xcode)"),
    ("vscode", "Visual Studio Code"),
    ("Kaleidoscope", "Kaleidoscope"),
    ("bc", "Beyond Compare"),
    ("p4merge", "P4Merge"),
    ("smerge", "Sublime Merge"),
    ("kdiff3", "KDiff3"),
    ("meld", "Meld"),
    ("winmerge", "WinMerge"),
    ("deltawalker", "DeltaWalker"),
    ("araxis", "Araxis Merge"),
];

/// Whether a tool exists for this system at all.
fn offered_here(tool: &str) -> bool {
    match tool {
        "opendiff" | "Kaleidoscope" | "deltawalker" => cfg!(target_os = "macos"),
        "winmerge" => cfg!(windows),
        _ => true,
    }
}

/// Where each tool's launcher usually lives, so Git finds it even when Weave
/// was started from the Dock, Start menu or a desktop launcher.
fn candidates(tool: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let paths: &[&str] = match tool {
            "opendiff" => &["/usr/bin/opendiff"],
            "vscode" => &["/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"],
            "Kaleidoscope" => &[
                "/usr/local/bin/ksdiff",
                "/opt/homebrew/bin/ksdiff",
                "/Applications/Kaleidoscope.app/Contents/MacOS/ksdiff",
            ],
            "bc" => &["/Applications/Beyond Compare.app/Contents/MacOS/bcomp"],
            "p4merge" => &["/Applications/p4merge.app/Contents/Resources/launchp4merge"],
            "smerge" => &["/Applications/Sublime Merge.app/Contents/SharedSupport/bin/smerge"],
            "kdiff3" => &["/Applications/kdiff3.app/Contents/MacOS/kdiff3"],
            "meld" => &["/Applications/Meld.app/Contents/MacOS/Meld"],
            "deltawalker" => &["/Applications/DeltaWalker.app/Contents/MacOS/DeltaWalker"],
            "araxis" => &["/Applications/Araxis Merge.app/Contents/Utilities/compare"],
            _ => &[],
        };
        found.extend(paths.iter().map(PathBuf::from));
    }
    #[cfg(windows)]
    {
        let relative: &[&str] = match tool {
            "vscode" => &[
                r"Microsoft VS Code\bin\code.cmd",
                r"Programs\Microsoft VS Code\bin\code.cmd",
            ],
            "bc" => &[r"Beyond Compare 5\BComp.exe", r"Beyond Compare 4\BComp.exe"],
            "p4merge" => &[r"Perforce\p4merge.exe"],
            "smerge" => &[r"Sublime Merge\smerge.exe"],
            "kdiff3" => &[r"KDiff3\kdiff3.exe"],
            "meld" => &[r"Meld\Meld.exe", r"Programs\Meld\Meld.exe"],
            "winmerge" => &[
                r"WinMerge\WinMergeU.exe",
                r"Programs\WinMerge\WinMergeU.exe",
            ],
            "araxis" => &[r"Araxis\Araxis Merge\Compare.exe"],
            _ => &[],
        };
        for base in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(base) {
                found.extend(relative.iter().map(|path| PathBuf::from(&base).join(path)));
            }
        }
    }
    let command = match tool {
        "vscode" => "code",
        "bc" => "bcompare",
        "Kaleidoscope" => "ksdiff",
        "araxis" => "compare",
        "winmerge" => "WinMergeU",
        other => other,
    };
    if tool != "araxis" {
        found.extend(crate::platform::find_tool(command));
    }
    found
}

fn tool_path(tool: &str) -> Option<String> {
    if !offered_here(tool) {
        return None;
    }
    candidates(tool)
        .into_iter()
        .find(|path| path.exists())
        .map(|path| path.to_string_lossy().into_owned())
}

fn global_config(args: &[&str]) -> Result<String, String> {
    let output = crate::platform::command("git")
        .args(["config", "--global"])
        .args(args)
        .output()
        .map_err(|error| format!("Git could not start: {error}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[tauri::command]
pub fn git_tool_settings() -> Result<ToolSettings, String> {
    Ok(ToolSettings {
        diff_tool: global_config(&["--get", "diff.tool"])?,
        merge_tool: global_config(&["--get", "merge.tool"])?,
        tools: TOOLS
            .iter()
            .filter(|(id, _)| offered_here(id))
            .map(|(id, name)| ToolOption {
                id: (*id).into(),
                name: (*name).into(),
                installed: tool_path(id).is_some(),
            })
            .collect(),
    })
}

fn configure_tool(kind: &str, tool: &str) -> Result<(), String> {
    let key = format!("{kind}.tool");
    if tool.is_empty() {
        global_config(&["--unset", &key])?;
        return Ok(());
    }
    if !TOOLS.iter().any(|(id, _)| *id == tool && offered_here(id)) {
        return Err("Choose one of the listed tools.".into());
    }
    global_config(&[&key, tool])?;
    let section = if kind == "diff" {
        "difftool"
    } else {
        "mergetool"
    };
    if tool == "Kaleidoscope" {
        let ksdiff = tool_path(tool).ok_or("Install Kaleidoscope's command-line tool (ksdiff) from its Integration settings first.")?;
        let command = if kind == "diff" {
            format!("\"{ksdiff}\" --partial-changeset --relative-path \"$MERGED\" -- \"$LOCAL\" \"$REMOTE\"")
        } else {
            format!("\"{ksdiff}\" --merge --output \"$MERGED\" --base \"$BASE\" -- \"$LOCAL\" --snapshot \"$REMOTE\" --snapshot")
        };
        global_config(&[&format!("{section}.Kaleidoscope.cmd"), &command])?;
        global_config(&[&format!("{section}.Kaleidoscope.trustExitCode"), "true"])?;
    } else if let Some(path) = tool_path(tool) {
        global_config(&[&format!("{section}.{tool}.path"), &path])?;
    }
    Ok(())
}

#[tauri::command]
pub fn git_tool_settings_save(
    diff_tool: String,
    merge_tool: String,
) -> Result<ToolSettings, String> {
    configure_tool("diff", diff_tool.trim())?;
    configure_tool("merge", merge_tool.trim())?;
    git_tool_settings()
}

fn spawn_tool(root: &str, args: &[&str]) -> Result<(), String> {
    let mut dirs = crate::platform::extra_tool_dirs();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    let path = std::env::join_paths(dirs).unwrap_or_default();
    crate::platform::command("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not start the external tool: {error}"))
}

#[tauri::command]
pub fn git_open_difftool(path: String, file: String, staged: bool) -> Result<(), String> {
    let root = root(&path)?;
    check_paths(std::slice::from_ref(&file))?;
    if global_config(&["--get", "diff.tool"])?.is_empty() {
        return Err("Choose an external diff tool in Git tools first.".into());
    }
    let mut args = vec!["difftool", "--no-prompt"];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", file.as_str()]);
    spawn_tool(&root, &args)
}

#[tauri::command]
pub fn git_open_mergetool(path: String, file: String) -> Result<(), String> {
    let root = root(&path)?;
    check_paths(std::slice::from_ref(&file))?;
    if global_config(&["--get", "merge.tool"])?.is_empty() {
        return Err("Choose an external merge tool in Git tools first.".into());
    }
    spawn_tool(&root, &["mergetool", "--no-prompt", "--", &file])
}

// ---------------------------------------------------------------------------
// Bisect

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BisectState {
    active: bool,
    current: String,
    log: String,
    found: String,
}

fn bisect_state(root: &str, last_output: &str) -> Result<BisectState, String> {
    let active = run(root, &["rev-parse", "--verify", "-q", "refs/bisect/bad"]).is_ok();
    let current = if active {
        run(root, &["log", "-1", "--format=%h %s"])?
            .trim()
            .to_owned()
    } else {
        String::new()
    };
    let found = last_output
        .lines()
        // Older Git says "first bad commit"; newer Git quotes the term: "first 'bad' commit".
        .find(|line| {
            line.contains("is the first bad commit") || line.contains("is the first 'bad' commit")
        })
        .map(|line| line.trim().to_owned())
        .unwrap_or_default();
    Ok(BisectState {
        active,
        current,
        log: if active {
            run(root, &["bisect", "log"]).unwrap_or_default()
        } else {
            String::new()
        },
        found,
    })
}

#[tauri::command]
pub fn git_bisect(
    path: String,
    action: String,
    good: Option<String>,
    bad: Option<String>,
) -> Result<BisectState, String> {
    let root = root(&path)?;
    let output = match action.as_str() {
        "status" => String::new(),
        "start" => {
            let good = good.unwrap_or_default();
            let bad = bad
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "HEAD".into());
            check_ref(good.trim(), "known good commit")?;
            check_ref(bad.trim(), "known bad commit")?;
            if !run(&root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
                return Err("Commit or stash your changes before bisecting.".into());
            }
            run(&root, &["bisect", "start", bad.trim(), good.trim()])?
        }
        "good" | "bad" | "skip" => {
            if run(&root, &["rev-parse", "--verify", "-q", "refs/bisect/bad"]).is_err() {
                return Err("Start a bisect first.".into());
            }
            run(&root, &["bisect", &action])?
        }
        "reset" => run(&root, &["bisect", "reset"])?,
        _ => return Err("Choose start, good, bad, skip or reset.".into()),
    };
    bisect_state(&root, &output)
}

// ---------------------------------------------------------------------------
// Patches

#[tauri::command]
pub fn git_patch_create(
    path: String,
    revisions: String,
    folder: String,
) -> Result<Vec<String>, String> {
    let root = root(&path)?;
    let revisions = revisions.trim();
    check_ref(revisions, "commit or range, such as main..feature")?;
    let folder_path = Path::new(&folder);
    if !folder_path.is_absolute() || !folder_path.is_dir() {
        return Err("Choose an existing folder for the patch files.".into());
    }
    // A single commit becomes one patch; a range becomes a series.
    let selector = if revisions.contains("..") {
        revisions.to_owned()
    } else {
        format!("{revisions}^!")
    };
    let output = run(&root, &["format-patch", "-o", &folder, &selector])?;
    let files: Vec<String> = output
        .lines()
        .map(str::to_owned)
        .filter(|line| !line.is_empty())
        .collect();
    if files.is_empty() {
        return Err("No commits matched, so no patch was written.".into());
    }
    Ok(files)
}

#[tauri::command]
pub fn git_patch_apply(path: String, file: String, as_commits: bool) -> Result<String, String> {
    let root = root(&path)?;
    let patch = Path::new(&file);
    if !patch.is_absolute() || !patch.is_file() {
        return Err("Choose a patch file.".into());
    }
    if as_commits {
        run(&root, &["am", "--3way", &file]).map_err(|error| {
            let _ = run(&root, &["am", "--abort"]);
            format!("The patch did not apply cleanly, so nothing was changed. {error}")
        })?;
        Ok("Applied the patch as commits.".into())
    } else {
        run(&root, &["apply", "--check", &file])
            .map_err(|error| format!("The patch does not apply to these files: {error}"))?;
        run(&root, &["apply", &file])?;
        Ok("Applied the patch to the working files.".into())
    }
}

// ---------------------------------------------------------------------------
// Cleaning untracked files

fn clean_args(ignored: bool, directories: bool, dry_run: bool) -> Vec<&'static str> {
    let mut args = vec!["clean", if dry_run { "-n" } else { "-f" }];
    if directories {
        args.push("-d");
    }
    if ignored {
        args.push("-x");
    }
    args
}

#[tauri::command]
pub fn git_clean_preview(
    path: String,
    ignored: bool,
    directories: bool,
) -> Result<Vec<String>, String> {
    let root = root(&path)?;
    Ok(run(&root, &clean_args(ignored, directories, true))?
        .lines()
        .filter_map(|line| line.strip_prefix("Would remove "))
        .map(str::to_owned)
        .collect())
}

#[tauri::command]
pub fn git_clean(
    path: String,
    ignored: bool,
    directories: bool,
    expected: usize,
) -> Result<usize, String> {
    let root = root(&path)?;
    let preview = git_clean_preview(root.clone(), ignored, directories)?;
    if preview.len() != expected {
        return Err(
            "The list of files changed since the preview. Review it again before deleting.".into(),
        );
    }
    run(&root, &clean_args(ignored, directories, false))?;
    Ok(preview.len())
}

// ---------------------------------------------------------------------------
// Stash, tags and notes

#[tauri::command]
pub fn git_stash_push(
    path: String,
    message: String,
    include_untracked: bool,
    keep_index: bool,
    paths: Vec<String>,
) -> Result<String, String> {
    let root = root(&path)?;
    check_paths(&paths)?;
    let mut args = vec!["stash", "push"];
    if include_untracked {
        args.push("--include-untracked");
    }
    if keep_index {
        args.push("--keep-index");
    }
    let message = message.trim();
    if !message.is_empty() {
        args.extend(["-m", message]);
    }
    if !paths.is_empty() {
        args.push("--");
        args.extend(paths.iter().map(String::as_str));
    }
    let output = run(&root, &args)?;
    if output.contains("No local changes to save") {
        return Err("There are no matching changes to stash.".into());
    }
    Ok("Changes stashed.".into())
}

#[tauri::command]
pub fn git_tag_annotated(
    path: String,
    name: String,
    message: String,
    target: String,
    sign: bool,
) -> Result<(), String> {
    let root = root(&path)?;
    let name = name.trim();
    run(&root, &["check-ref-format", &format!("refs/tags/{name}")])
        .map_err(|_| format!("“{name}” is not a valid tag name."))?;
    let target = if target.trim().is_empty() {
        "HEAD"
    } else {
        target.trim()
    };
    check_ref(target, "commit")?;
    if message.trim().is_empty() {
        return Err("Write a tag message.".into());
    }
    run(
        &root,
        &[
            "tag",
            if sign { "-s" } else { "-a" },
            name,
            "-m",
            message.trim(),
            target,
        ],
    )?;
    Ok(())
}

#[tauri::command]
pub async fn git_push_tags(
    path: String,
    tag: Option<String>,
    delete: bool,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = root(&path)?;
        let auth = forge::git_auth_for_repo(&root)?;
        match tag.filter(|tag| !tag.trim().is_empty()) {
            Some(tag) => {
                check_ref(tag.trim(), "tag")?;
                let refspec = if delete {
                    format!(":refs/tags/{}", tag.trim())
                } else {
                    format!("refs/tags/{}", tag.trim())
                };
                run_authenticated(&root, &["push", "origin", &refspec], auth.as_ref())?;
                Ok(if delete {
                    format!("Deleted tag {} from origin.", tag.trim())
                } else {
                    format!("Pushed tag {}.", tag.trim())
                })
            }
            None => {
                run_authenticated(&root, &["push", "origin", "--tags"], auth.as_ref())?;
                Ok("Pushed all tags.".into())
            }
        }
    })
    .await
    .map_err(|error| format!("Pushing tags stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub fn git_notes(path: String, commit: String) -> Result<String, String> {
    let root = root(&path)?;
    check_ref(commit.trim(), "commit")?;
    Ok(run(&root, &["notes", "show", commit.trim()])
        .unwrap_or_default()
        .trim_end()
        .to_owned())
}

#[tauri::command]
pub fn git_notes_set(path: String, commit: String, text: String) -> Result<(), String> {
    let root = root(&path)?;
    check_ref(commit.trim(), "commit")?;
    if text.trim().is_empty() {
        let _ = run(
            &root,
            &["notes", "remove", "--ignore-missing", commit.trim()],
        );
        return Ok(());
    }
    run(
        &root,
        &["notes", "add", "-f", "-m", text.trim(), commit.trim()],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Custom commands

#[tauri::command]
pub async fn git_custom_command(path: String, args: Vec<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = root(&path)?;
        if args.is_empty() || args.len() > 64 || args.iter().any(|arg| arg.contains('\0')) {
            return Err("Enter a Git command, such as log --oneline -5.".into());
        }
        if args[0] == "git" {
            return Err("Leave out the word git; Weave adds it.".into());
        }
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let auth = forge::git_auth_for_repo(&root)?;
        let output = run_authenticated(&root, &borrowed, auth.as_ref())?;
        Ok(if output.len() > 200_000 {
            format!("{}…\n(output shortened)", &output[..200_000])
        } else {
            output
        })
    })
    .await
    .map_err(|error| format!("The command stopped unexpectedly: {error}"))?
}

// ---------------------------------------------------------------------------
// Deleted branches

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeletedBranch {
    pub name: String,
    pub commit: String,
    pub deleted_at: i64,
}

fn deleted_file(root: &str) -> Result<PathBuf, String> {
    let location = run(
        root,
        &["rev-parse", "--git-path", "weave-deleted-branches.json"],
    )?;
    let location = PathBuf::from(location.trim());
    Ok(if location.is_absolute() {
        location
    } else {
        Path::new(root).join(location)
    })
}

pub(crate) fn deleted_branches(root: &str) -> Vec<DeletedBranch> {
    deleted_file(root)
        .ok()
        .and_then(|file| fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_deleted(root: &str, list: &[DeletedBranch]) -> Result<(), String> {
    let text = serde_json::to_string(list).map_err(|error| error.to_string())?;
    fs::write(deleted_file(root)?, text)
        .map_err(|error| format!("Could not remember the deleted branch: {error}"))
}

/// Records a branch's tip just before it is deleted so it can be restored.
pub(crate) fn remember_deleted_branch(path: &str, name: &str) -> Result<(), String> {
    let root = root(path)?;
    let commit = run(
        &root,
        &["rev-parse", "--verify", &format!("refs/heads/{name}")],
    )?
    .trim()
    .to_owned();
    let mut list = deleted_branches(&root);
    list.retain(|item| item.name != name);
    list.push(DeletedBranch {
        name: name.into(),
        commit,
        deleted_at: chrono::Utc::now().timestamp(),
    });
    let excess = list.len().saturating_sub(50);
    list.drain(..excess);
    save_deleted(&root, &list)
}

#[tauri::command]
pub fn git_deleted_branches(path: String) -> Result<Vec<DeletedBranch>, String> {
    let root = root(&path)?;
    let mut list = deleted_branches(&root);
    list.retain(|item| {
        run(
            &root,
            &["cat-file", "-e", &format!("{}^{{commit}}", item.commit)],
        )
        .is_ok()
    });
    list.reverse();
    Ok(list)
}

#[tauri::command]
pub fn git_branch_restore(path: String, name: String) -> Result<String, String> {
    let root = root(&path)?;
    let mut list = deleted_branches(&root);
    let item = list
        .iter()
        .rev()
        .find(|item| item.name == name)
        .cloned()
        .ok_or("That branch is not in the deleted list.")?;
    if run(
        &root,
        &["rev-parse", "--verify", "-q", &format!("refs/heads/{name}")],
    )
    .is_ok()
    {
        return Err(format!("A branch named {name} already exists."));
    }
    run(&root, &["branch", &name, &item.commit])?;
    list.retain(|entry| entry.name != name);
    save_deleted(&root, &list)?;
    Ok(format!("Restored branch {name}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        run(&path, &["init", "-b", "main"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        run(&path, &["config", "commit.gpgsign", "false"]).unwrap();
        run(&path, &["config", "tag.gpgsign", "false"]).unwrap();
        (dir, path)
    }

    fn commit(dir: &tempfile::TempDir, path: &str, file: &str, text: &str) {
        fs::write(dir.path().join(file), text).unwrap();
        run(path, &["add", file]).unwrap();
        run(path, &["commit", "-m", text]).unwrap();
    }

    #[test]
    fn filters_history_to_chosen_branches() {
        let (dir, path) = repo();
        commit(&dir, &path, "a", "base");
        run(&path, &["checkout", "-b", "side"]).unwrap();
        commit(&dir, &path, "b", "side work");
        run(&path, &["checkout", "main"]).unwrap();
        let main_only = git_commits_for(path.clone(), vec!["main".into()], 0).unwrap();
        assert_eq!(
            main_only
                .iter()
                .map(|c| c.subject.as_str())
                .collect::<Vec<_>>(),
            ["base"]
        );
        assert_eq!(git_commits_for(path.clone(), vec![], 0).unwrap().len(), 2);
        assert!(git_commits_for(path, vec!["--all".into()], 0).is_err());
    }

    #[test]
    fn bisects_to_the_first_bad_commit() {
        let (dir, path) = repo();
        for n in 1..=8 {
            commit(
                &dir,
                &path,
                "value",
                &format!("{} {n}", if n >= 5 { "broken" } else { "ok" }),
            );
            run(&path, &["tag", &format!("v{n}")]).unwrap();
        }
        let mut state = git_bisect(path.clone(), "start".into(), Some("v1".into()), None).unwrap();
        for _ in 0..10 {
            if !state.found.is_empty() {
                break;
            }
            let broken = fs::read_to_string(dir.path().join("value"))
                .unwrap()
                .starts_with("broken");
            state = git_bisect(
                path.clone(),
                if broken { "bad" } else { "good" }.into(),
                None,
                None,
            )
            .unwrap();
        }
        let first_bad = run(&path, &["rev-parse", "v5"]).unwrap();
        assert!(state.found.starts_with(first_bad.trim()));
        let reset = git_bisect(path, "reset".into(), None, None).unwrap();
        assert!(!reset.active);
    }

    #[test]
    fn creates_and_applies_patches() {
        let (dir, path) = repo();
        commit(&dir, &path, "a", "one");
        commit(&dir, &path, "a", "two");
        let out = tempfile::tempdir().unwrap();
        let files = git_patch_create(
            path.clone(),
            "HEAD".into(),
            out.path().to_string_lossy().into(),
        )
        .unwrap();
        assert_eq!(files.len(), 1);
        run(&path, &["reset", "--hard", "HEAD~1"]).unwrap();
        git_patch_apply(path.clone(), files[0].clone(), true).unwrap();
        assert_eq!(
            run(&path, &["log", "-1", "--format=%s"]).unwrap().trim(),
            "two"
        );
        run(&path, &["reset", "--hard", "HEAD~1"]).unwrap();
        git_patch_apply(path.clone(), files[0].clone(), false).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("a")).unwrap(), "two");
        fs::write(dir.path().join("a"), "conflicting").unwrap();
        run(&path, &["commit", "-am", "conflict"]).unwrap();
        assert!(git_patch_apply(path, files[0].clone(), false)
            .unwrap_err()
            .contains("does not apply"));
    }

    #[test]
    fn previews_then_cleans_untracked_files() {
        let (dir, path) = repo();
        commit(&dir, &path, "a", "one");
        fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
        fs::write(dir.path().join("junk.txt"), "x").unwrap();
        fs::write(dir.path().join("debug.log"), "x").unwrap();
        fs::create_dir(dir.path().join("build")).unwrap();
        fs::write(dir.path().join("build/out"), "x").unwrap();
        let preview = git_clean_preview(path.clone(), false, true).unwrap();
        assert_eq!(preview, [".gitignore", "build/", "junk.txt"]);
        assert!(git_clean(path.clone(), false, true, 1).is_err());
        assert_eq!(git_clean(path.clone(), false, true, 3).unwrap(), 3);
        assert!(dir.path().join("debug.log").exists());
        assert_eq!(git_clean(path, true, true, 1).unwrap(), 1);
        assert!(!dir.path().join("debug.log").exists());
    }

    #[test]
    fn stashes_selected_and_untracked_files() {
        let (dir, path) = repo();
        commit(&dir, &path, "a", "one");
        commit(&dir, &path, "b", "two");
        fs::write(dir.path().join("a"), "changed a").unwrap();
        fs::write(dir.path().join("b"), "changed b").unwrap();
        fs::write(dir.path().join("new"), "new").unwrap();
        git_stash_push(
            path.clone(),
            "only a".into(),
            false,
            false,
            vec!["a".into()],
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("a")).unwrap(), "one");
        assert_eq!(
            fs::read_to_string(dir.path().join("b")).unwrap(),
            "changed b"
        );
        git_stash_push(path.clone(), "rest".into(), true, false, vec![]).unwrap();
        assert!(!dir.path().join("new").exists());
        assert!(run(&path, &["stash", "list"]).unwrap().contains("rest"));
        assert!(git_stash_push(path, String::new(), false, false, vec![]).is_err());
    }

    #[test]
    fn creates_annotated_tags_and_notes() {
        let (dir, path) = repo();
        commit(&dir, &path, "a", "one");
        git_tag_annotated(
            path.clone(),
            "v1.0".into(),
            "First release".into(),
            String::new(),
            false,
        )
        .unwrap();
        assert_eq!(
            run(&path, &["cat-file", "-t", "v1.0"]).unwrap().trim(),
            "tag"
        );
        assert!(git_tag_annotated(
            path.clone(),
            "bad name".into(),
            "m".into(),
            String::new(),
            false
        )
        .is_err());
        git_notes_set(path.clone(), "HEAD".into(), "Reviewed by Ada".into()).unwrap();
        assert_eq!(
            git_notes(path.clone(), "HEAD".into()).unwrap(),
            "Reviewed by Ada"
        );
        git_notes_set(path.clone(), "HEAD".into(), String::new()).unwrap();
        assert_eq!(git_notes(path, "HEAD".into()).unwrap(), "");
    }

    #[test]
    fn restores_deleted_branches() {
        let (dir, path) = repo();
        commit(&dir, &path, "a", "one");
        run(&path, &["checkout", "-b", "topic"]).unwrap();
        commit(&dir, &path, "b", "topic work");
        let tip = run(&path, &["rev-parse", "HEAD"]).unwrap();
        run(&path, &["checkout", "main"]).unwrap();
        remember_deleted_branch(&path, "topic").unwrap();
        run(&path, &["branch", "-D", "topic"]).unwrap();
        assert_eq!(git_deleted_branches(path.clone()).unwrap()[0].name, "topic");
        git_branch_restore(path.clone(), "topic".into()).unwrap();
        assert_eq!(run(&path, &["rev-parse", "topic"]).unwrap(), tip);
        assert!(git_deleted_branches(path).unwrap().is_empty());
    }

    #[test]
    fn adds_and_pulls_a_subtree() {
        let (library_dir, library) = repo();
        commit(&library_dir, &library, "lib.txt", "v1");
        let (dir, path) = repo();
        commit(&dir, &path, "app.txt", "app");
        let message = tauri::async_runtime::block_on(git_subtree(
            path.clone(),
            "add".into(),
            "vendor/lib".into(),
            library.clone(),
            "main".into(),
            true,
        ))
        .unwrap();
        assert!(message.contains("vendor/lib"));
        assert_eq!(
            fs::read_to_string(dir.path().join("vendor/lib/lib.txt")).unwrap(),
            "v1"
        );
        commit(&library_dir, &library, "lib.txt", "v2");
        tauri::async_runtime::block_on(git_subtree(
            path.clone(),
            "pull".into(),
            "vendor/lib".into(),
            library,
            "main".into(),
            true,
        ))
        .unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("vendor/lib/lib.txt")).unwrap(),
            "v2"
        );
        assert!(tauri::async_runtime::block_on(git_subtree(
            path,
            "add".into(),
            "../x".into(),
            "r".into(),
            "main".into(),
            false
        ))
        .is_err());
    }

    #[test]
    fn runs_custom_commands_without_a_shell() {
        let (dir, path) = repo();
        commit(&dir, &path, "a", "one");
        let output = tauri::async_runtime::block_on(git_custom_command(
            path.clone(),
            vec!["log".into(), "--format=%s".into()],
        ))
        .unwrap();
        assert_eq!(output.trim(), "one");
        assert!(tauri::async_runtime::block_on(git_custom_command(
            path,
            vec!["git".into(), "log".into()]
        ))
        .is_err());
    }
}
