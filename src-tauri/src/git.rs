use crate::forge;
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::{Component, Path},
    process::Stdio,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFile {
    pub path: String,
    pub staged: bool,
    pub changed: bool,
    pub status: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub short_id: String,
    pub id: String,
    pub subject: String,
    pub author: String,
    pub relative_date: String,
    pub refs: String,
    pub graph: String,
}

pub(crate) fn commit_log(path: &str, skip: usize) -> Result<Vec<Commit>, String> {
    commit_log_for(path, skip, &[])
}

/// One page of the commit graph; with `refs` empty it covers all branches.
pub(crate) fn commit_log_for(
    path: &str,
    skip: usize,
    refs: &[String],
) -> Result<Vec<Commit>, String> {
    let skip_arg = format!("--skip={skip}");
    let mut args = vec![
        "log",
        "--graph",
        "--no-color",
        "--max-count=300",
        &skip_arg,
        "--date=relative",
        "--format=%h%x1f%H%x1f%s%x1f%an%x1f%ar%x1f%D",
    ];
    // --graph orders topologically; some people prefer commits by date.
    if run(path, &["config", "--get", "weave.graphOrder"]).is_ok_and(|order| order.trim() == "date")
    {
        args.push("--date-order");
    }
    if refs.is_empty() {
        args.insert(1, "--all");
    } else {
        args.extend(refs.iter().map(String::as_str));
        args.push("--");
    }
    let log = run(path, &args)?;
    Ok(log
        .lines()
        .filter_map(|line| {
            let columns: Vec<_> = line.split('\u{1f}').collect();
            let (graph, short_id) = columns.first()?.rsplit_once(' ')?;
            (columns.len() == 6).then(|| Commit {
                short_id: short_id.into(),
                id: columns[1].into(),
                subject: columns[2].into(),
                author: columns[3].into(),
                relative_date: columns[4].into(),
                refs: columns[5].into(),
                graph: graph.into(),
            })
        })
        .collect())
}

#[tauri::command]
pub fn git_commits_page(path: String, skip: usize) -> Result<Vec<Commit>, String> {
    if skip > 1_000_000 {
        return Err("History offset is too large. Refresh the repository and try again.".into());
    }
    run(&path, &["rev-parse", "--show-toplevel"])?;
    if !has_commits(&path)? {
        return Ok(Vec::new());
    }
    commit_log(&path, skip)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameLine {
    pub number: usize,
    pub commit: String,
    pub author: String,
    pub summary: String,
    pub content: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlamePage {
    pub lines: Vec<BlameLine>,
    pub has_more: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineHistoryEntry {
    pub commit: Commit,
    pub patch: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitLfsInfo {
    pub version: String,
    pub tracked_patterns: String,
    pub tracked_files: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSnapshot {
    pub path: String,
    pub name: String,
    pub branch: String,
    pub branches: Vec<String>,
    pub files: Vec<GitFile>,
    pub commits: Vec<Commit>,
    pub ahead: u32,
    pub behind: u32,
    pub remote: Option<String>,
    pub tags: Vec<String>,
    pub stashes: Vec<String>,
    pub remotes: Vec<String>,
    pub remote_urls: Vec<GitRemote>,
    pub remote_branches: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRemote {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktree {
    pub path: String,
    pub branch: String,
    pub main: bool,
    pub locked: bool,
}

fn worktrees(path: &str) -> Result<Vec<GitWorktree>, String> {
    let output = run(path, &["worktree", "list", "--porcelain", "-z"])?;
    let mut result = Vec::new();
    let mut current: Option<GitWorktree> = None;
    for field in output.split('\0') {
        if field.is_empty() {
            if let Some(worktree) = current.take() {
                result.push(worktree);
            }
            continue;
        }
        if let Some(value) = field.strip_prefix("worktree ") {
            if let Some(worktree) = current.take() {
                result.push(worktree);
            }
            current = Some(GitWorktree {
                path: value.to_owned(),
                branch: "Detached HEAD".into(),
                main: result.is_empty(),
                locked: false,
            });
        } else if let Some(worktree) = current.as_mut() {
            if let Some(value) = field.strip_prefix("branch refs/heads/") {
                worktree.branch = value.to_owned();
            } else if field == "locked" || field.starts_with("locked ") {
                worktree.locked = true;
            }
        }
    }
    if let Some(worktree) = current {
        result.push(worktree);
    }
    Ok(result)
}

#[tauri::command]
pub fn git_worktrees(path: String) -> Result<Vec<GitWorktree>, String> {
    worktrees(&path)
}

#[tauri::command]
pub fn git_worktree_create(
    path: String,
    destination: String,
    branch: String,
    new_branch: bool,
) -> Result<String, String> {
    let destination = Path::new(destination.trim());
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Err("Choose an absolute destination folder for the worktree.".into());
    }
    if fs::symlink_metadata(destination).is_ok() {
        return Err("The worktree destination already exists. Choose a new folder name.".into());
    }
    let parent = destination
        .parent()
        .ok_or("Choose a folder inside an existing parent folder.")?;
    if !parent.is_dir() {
        return Err("The worktree parent folder does not exist.".into());
    }
    let branch = branch.trim();
    if branch.is_empty() || branch.starts_with('-') {
        return Err("Choose a valid branch for the worktree.".into());
    }
    run(&path, &["check-ref-format", "--branch", branch])?;
    let destination = fs::canonicalize(parent)
        .map_err(|error| format!("Could not read the worktree parent folder: {error}"))?
        .join(destination.file_name().unwrap());
    let destination = destination
        .to_str()
        .ok_or("The worktree folder name could not be read by Git.")?;
    if new_branch {
        run(&path, &["worktree", "add", "-b", branch, "--", destination])?;
    } else {
        run(&path, &["worktree", "add", "--", destination, branch])?;
    }
    fs::canonicalize(destination)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| format!("Worktree was created, but its folder could not be read: {error}"))
}

#[tauri::command]
pub fn git_worktree_remove(path: String, destination: String) -> Result<(), String> {
    let destination = fs::canonicalize(&destination)
        .map_err(|error| format!("Could not open the worktree folder: {error}"))?
        .to_string_lossy()
        .into_owned();
    let target = worktrees(&path)?
        .into_iter()
        .find(|item| item.path == destination)
        .ok_or("This folder is not a worktree of the current repository.")?;
    if target.main {
        return Err("The main repository cannot be removed as a worktree.".into());
    }
    if target.locked {
        return Err("Unlock this worktree in Git before removing it.".into());
    }
    run(&path, &["worktree", "remove", "--", &destination])?;
    Ok(())
}

#[tauri::command]
pub fn git_worktree_lock(
    path: String,
    destination: String,
    lock: bool,
) -> Result<Vec<GitWorktree>, String> {
    let destination = fs::canonicalize(&destination)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or(destination);
    let target = worktrees(&path)?
        .into_iter()
        .find(|item| item.path == destination)
        .ok_or("This folder is not a worktree of the current repository.")?;
    if target.main {
        return Err("The main repository cannot be locked or unlocked here.".into());
    }
    if lock && !target.locked {
        run(&path, &["worktree", "lock", "--", &destination])?;
    } else if !lock && target.locked {
        run(&path, &["worktree", "unlock", "--", &destination])?;
    }
    worktrees(&path)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitConflict {
    pub file: String,
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIdentity {
    pub local_name: Option<String>,
    pub local_email: Option<String>,
    pub global_name: Option<String>,
    pub global_email: Option<String>,
}

fn config_value(path: &str, scope: &str, key: &str) -> Result<Option<String>, String> {
    let output = crate::platform::command("git")
        .arg("-C")
        .arg(path)
        .args(["config", scope, "--get", key])
        .output()
        .map_err(|error| format!("Git could not read {key}: {error}"))?;
    match output.status.code() {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        )),
        Some(1) => Ok(None),
        _ => Err(format!(
            "Git could not read {key}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

pub(crate) fn has_commits(path: &str) -> Result<bool, String> {
    let count = run(path, &["rev-list", "--all", "--count"])?;
    count
        .trim()
        .parse::<u64>()
        .map(|count| count > 0)
        .map_err(|error| format!("Git returned an invalid history count: {error}"))
}

#[tauri::command]
pub fn git_identity(path: String) -> Result<GitIdentity, String> {
    run(&path, &["rev-parse", "--show-toplevel"])?;
    Ok(GitIdentity {
        local_name: config_value(&path, "--local", "user.name")?,
        local_email: config_value(&path, "--local", "user.email")?,
        global_name: config_value(&path, "--global", "user.name")?,
        global_email: config_value(&path, "--global", "user.email")?,
    })
}

#[tauri::command]
pub fn git_identity_save(
    path: String,
    scope: String,
    name: String,
    email: String,
) -> Result<GitIdentity, String> {
    run(&path, &["rev-parse", "--show-toplevel"])?;
    let scope_arg = match scope.as_str() {
        "local" => "--local",
        "global" => "--global",
        _ => return Err("Choose this repository or all repositories for the Git identity.".into()),
    };
    let name = name.trim();
    let email = email.trim();
    if name.is_empty() || email.is_empty() {
        return Err("Enter both a commit name and email address.".into());
    }
    if name.len() > 100
        || email.len() > 254
        || name.chars().any(char::is_control)
        || email.chars().any(char::is_whitespace)
        || !email.contains('@')
        || email.starts_with('@')
        || email.ends_with('@')
    {
        return Err(
            "Enter a name under 100 characters and an email address without spaces.".into(),
        );
    }
    run(&path, &["config", scope_arg, "user.name", name])?;
    run(&path, &["config", scope_arg, "user.email", email])
        .map_err(|error| format!("Commit name was saved, but email could not be saved: {error}"))?;
    git_identity(path)
}

#[tauri::command]
pub fn git_search(path: String, query: String, mode: String) -> Result<Vec<Commit>, String> {
    run(&path, &["rev-parse", "--show-toplevel"])?;
    let query = query.trim();
    if query.is_empty() {
        return Err("Enter a commit search term.".into());
    }
    if query.len() > 200 || query.chars().any(char::is_control) {
        return Err("Keep the commit search under 200 characters without line breaks.".into());
    }
    let search = match mode.as_str() {
        "message" => format!("--grep={query}"),
        "author" => format!("--author={query}"),
        "content" => format!("-S{query}"),
        _ => return Err("Choose message, author, or changed content for the search.".into()),
    };
    if !has_commits(&path)? {
        return Ok(Vec::new());
    }
    let output = run(
        &path,
        &[
            "log",
            "--all",
            "--no-color",
            "-n",
            "200",
            "--date=relative",
            "--format=%h%x1f%H%x1f%s%x1f%an%x1f%ar%x1f%D",
            &search,
            "--",
        ],
    )?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let columns: Vec<_> = line.split('\u{1f}').collect();
            (columns.len() == 6).then(|| Commit {
                short_id: columns[0].into(),
                id: columns[1].into(),
                subject: columns[2].into(),
                author: columns[3].into(),
                relative_date: columns[4].into(),
                refs: columns[5].into(),
                graph: String::new(),
            })
        })
        .collect())
}

#[tauri::command]
pub fn git_file_history(path: String, file: String) -> Result<Vec<Commit>, String> {
    checked_repo_file(&path, &file)?;
    if !has_commits(&path)? {
        return Ok(Vec::new());
    }
    let output = run(
        &path,
        &[
            "log",
            "--follow",
            "--no-color",
            "-n",
            "200",
            "--date=relative",
            "--format=%h%x1f%H%x1f%s%x1f%an%x1f%ar%x1f%D",
            "--",
            &file,
        ],
    )?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let columns: Vec<_> = line.split('\u{1f}').collect();
            (columns.len() == 6).then(|| Commit {
                short_id: columns[0].into(),
                id: columns[1].into(),
                subject: columns[2].into(),
                author: columns[3].into(),
                relative_date: columns[4].into(),
                refs: columns[5].into(),
                graph: String::new(),
            })
        })
        .collect())
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CommitFile {
    /// A, M, D, R (renamed), C (copied) or T (type changed).
    pub status: String,
    pub path: String,
    pub old_path: Option<String>,
}

/// Files a commit changed, compared with its first parent, with renames.
#[tauri::command]
pub fn git_commit_files(path: String, commit: String) -> Result<Vec<CommitFile>, String> {
    if !(4..=64).contains(&commit.len()) || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Choose a commit from the list.".into());
    }
    let output = run(
        &path,
        &[
            "show",
            "--no-color",
            "--diff-merges=first-parent",
            "--name-status",
            "-M",
            "-z",
            "--format=",
            &commit,
        ],
    )?;
    Ok(crate::git_compare::parse_name_status(&output))
}

#[tauri::command]
pub fn git_line_history(
    path: String,
    file: String,
    start: usize,
    end: usize,
) -> Result<Vec<LineHistoryEntry>, String> {
    if start == 0 || end < start || end - start > 499 {
        return Err("Choose a line range of 1 to 500 lines, starting at line 1 or later.".into());
    }
    if file.contains([':', '\n', '\r', '\0']) {
        return Err("This filename cannot be used for Git line-range history.".into());
    }
    let file_path = checked_repo_file(&path, &file)?;
    let metadata = fs::symlink_metadata(&file_path)
        .map_err(|error| format!("Could not inspect {file}: {error}"))?;
    if !metadata.is_file() || metadata.len() > 1_000_000 {
        return Err("Choose a regular text file up to 1 MB for line-range history.".into());
    }
    if !has_commits(&path)? {
        return Ok(Vec::new());
    }
    let range = format!("-L{start},{end}:{file}");
    let output = run(
        &path,
        &[
            "log",
            "--no-color",
            "-n",
            "100",
            "--date=relative",
            "--format=%x1e%h%x1f%H%x1f%s%x1f%an%x1f%ar%x1f%D",
            &range,
        ],
    )?;
    if output.len() > 8_000_000 {
        return Err(
            "This line history is too large to display. Choose a shorter line range.".into(),
        );
    }
    Ok(output
        .split('\u{1e}')
        .filter_map(|record| {
            let (header, patch) = record.trim_start_matches('\n').split_once('\n')?;
            let columns: Vec<_> = header.split('\u{1f}').collect();
            (columns.len() == 6).then(|| LineHistoryEntry {
                commit: Commit {
                    short_id: columns[0].into(),
                    id: columns[1].into(),
                    subject: columns[2].into(),
                    author: columns[3].into(),
                    relative_date: columns[4].into(),
                    refs: columns[5].into(),
                    graph: String::new(),
                },
                patch: patch.trim().to_owned(),
            })
        })
        .collect())
}

#[tauri::command]
pub fn git_blame_page(path: String, file: String, start: usize) -> Result<BlamePage, String> {
    const PAGE_SIZE: usize = 250;
    if start == 0 || start > 10_000_000 {
        return Err("Enter a line number between 1 and 10,000,000.".into());
    }
    let file_path = checked_repo_file(&path, &file)?;
    let metadata = fs::symlink_metadata(&file_path)
        .map_err(|error| format!("Could not inspect {file}: {error}"))?;
    if !metadata.is_file() {
        return Err("Choose a tracked text file for blame.".into());
    }
    let range = format!("{start},+{}", PAGE_SIZE + 1);
    let mut args = vec!["blame", "-L", &range, "--line-porcelain"];
    // Skip formatting-only commits listed the way GitHub and GitLab expect.
    let root = run(&path, &["rev-parse", "--show-toplevel"])?;
    let ignore_file = Path::new(root.trim()).join(".git-blame-ignore-revs");
    let ignore_arg = format!("--ignore-revs-file={}", ignore_file.display());
    if ignore_file.is_file() && run(&path, &["config", "--get", "blame.ignoreRevsFile"]).is_err() {
        args.push(&ignore_arg);
    }
    args.extend(["--", &file]);
    let output = run(&path, &args)?;
    if output.len() > 16_000_000 {
        return Err("These lines are too large to display. Choose another starting line.".into());
    }
    let mut lines = Vec::new();
    let mut commit = String::new();
    let mut number = 0;
    let mut author = String::new();
    let mut summary = String::new();
    for line in output.lines() {
        if let Some(content) = line.strip_prefix('\t') {
            lines.push(BlameLine {
                number,
                commit: commit.clone(),
                author: author.clone(),
                summary: summary.clone(),
                content: content.into(),
            });
        } else if let Some(value) = line.strip_prefix("author ") {
            author = value.into();
        } else if let Some(value) = line.strip_prefix("summary ") {
            summary = value.into();
        } else if line.split_whitespace().next().is_some_and(|word| {
            [40, 64].contains(&word.len()) && word.chars().all(|c| c.is_ascii_hexdigit())
        }) {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() >= 3 {
                commit = fields[0].into();
                number = fields[2].parse().unwrap_or(0);
                author.clear();
                summary.clear();
            }
        }
    }
    let has_more = lines.len() > PAGE_SIZE;
    lines.truncate(PAGE_SIZE);
    Ok(BlamePage { lines, has_more })
}

fn require_lfs(path: &str) -> Result<String, String> {
    run(path, &["lfs", "version"]).map_err(|_| {
        "Git LFS is not installed or not available to Git. Install Git LFS, then reopen this repository.".into()
    })
}

#[tauri::command]
pub fn git_lfs_info(path: String) -> Result<GitLfsInfo, String> {
    let version = require_lfs(&path)?.trim().to_string();
    let tracked_patterns = run(&path, &["lfs", "track"])?;
    let tracked_files = run(&path, &["lfs", "ls-files", "--name-only"])?
        .lines()
        .map(str::to_owned)
        .collect();
    Ok(GitLfsInfo {
        version,
        tracked_patterns,
        tracked_files,
    })
}

#[tauri::command]
pub fn git_lfs_track(path: String, pattern: String, track: bool) -> Result<GitLfsInfo, String> {
    require_lfs(&path)?;
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern.starts_with('-') || pattern.chars().any(char::is_control) {
        return Err("Enter a file pattern without line breaks or a leading dash.".into());
    }
    if track {
        run(&path, &["lfs", "track", "--", pattern])?;
    } else {
        run(&path, &["lfs", "untrack", "--", pattern])?;
    }
    git_lfs_info(path)
}

#[tauri::command]
pub fn git_lfs_pull(path: String) -> Result<GitLfsInfo, String> {
    require_lfs(&path)?;
    run(&path, &["lfs", "pull"])?;
    git_lfs_info(path)
}

pub(crate) fn checked_repo_file(path: &str, file: &str) -> Result<std::path::PathBuf, String> {
    if file.is_empty()
        || !Path::new(file)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err("Choose a file inside the repository.".into());
    }
    let root = run(path, &["rev-parse", "--show-toplevel"])?;
    Ok(Path::new(root.trim()).join(file))
}

#[tauri::command]
pub fn git_conflict(path: String, file: String) -> Result<GitConflict, String> {
    checked_repo_file(&path, &file)?;
    let index = run(&path, &["ls-files", "-u", "--", &file])?;
    if index.is_empty() {
        return Err("This file has no unresolved merge conflict.".into());
    }
    let read_stage = |stage: u8| -> Result<Option<String>, String> {
        if !index.lines().any(|line| {
            line.split('\t')
                .next()
                .is_some_and(|metadata| metadata.ends_with(&format!(" {stage}")))
        }) {
            return Ok(None);
        }
        let blob = format!(":{stage}:{file}");
        let output = crate::platform::command("git")
            .arg("-C")
            .arg(&path)
            .args(["show", &blob])
            .output()
            .map_err(|error| format!("Git could not read conflict version: {error}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        if output.stdout.len() > 1024 * 1024 {
            return Err("This conflict is larger than 1 MB. Open it in a dedicated editor.".into());
        }
        String::from_utf8(output.stdout)
            .map(Some)
            .map_err(|_| "This is a binary conflict. Use an external merge tool.".into())
    };
    let base = read_stage(1)?;
    let ours = read_stage(2)?;
    let theirs = read_stage(3)?;
    Ok(GitConflict {
        file,
        base,
        ours,
        theirs,
    })
}

#[tauri::command]
pub fn git_open_conflict_external(path: String, file: String) -> Result<(), String> {
    let destination = checked_repo_file(&path, &file)?;
    if run(&path, &["ls-files", "-u", "--", &file])?.is_empty() {
        return Err("This file has no unresolved merge conflict.".into());
    }
    let root = fs::canonicalize(run(&path, &["rev-parse", "--show-toplevel"])?.trim())
        .map_err(|error| format!("Could not inspect repository: {error}"))?;
    let destination = fs::canonicalize(&destination)
        .map_err(|error| format!("The conflicted file is not present in the worktree: {error}"))?;
    if !destination.starts_with(&root) || !destination.is_file() {
        return Err("The conflicted file must be a regular file inside the repository.".into());
    }
    crate::platform::open_path(&destination)
}

#[tauri::command]
pub fn git_conflict_resolve(path: String, file: String, result: String) -> Result<(), String> {
    let destination = checked_repo_file(&path, &file)?;
    if run(&path, &["ls-files", "-u", "--", &file])?.is_empty() {
        return Err("This file has no unresolved merge conflict.".into());
    }
    if result.len() > 4 * 1024 * 1024 {
        return Err("The resolved file is too large for the built-in editor.".into());
    }
    let root = fs::canonicalize(run(&path, &["rev-parse", "--show-toplevel"])?.trim())
        .map_err(|error| format!("Could not inspect repository: {error}"))?;
    let parent = destination
        .parent()
        .ok_or("Conflict file has no parent folder.")?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| format!("Could not open conflict folder: {error}"))?;
    if !canonical_parent.starts_with(&root) {
        return Err("Conflict file points outside the repository.".into());
    }
    if fs::symlink_metadata(&destination).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err("Resolve symbolic-link conflicts with Git or an external editor.".into());
    }
    fs::write(&destination, result)
        .map_err(|error| format!("Could not save resolved file: {error}"))?;
    run(&path, &["add", "--", &file])?;
    Ok(())
}

pub(crate) fn run(path: &str, args: &[&str]) -> Result<String, String> {
    if !Path::new(path).is_dir() {
        return Err(format!("Repository folder does not exist: {path}"));
    }
    let output = crate::platform::command("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|error| format!("Git could not start: {error}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if message.is_empty() {
            format!("Git command failed: git {}", args.join(" "))
        } else {
            message
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[tauri::command]
pub async fn git_snapshot(path: String) -> Result<GitSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || snapshot(path))
        .await
        .map_err(|error| format!("Repository refresh stopped unexpectedly: {error}"))?
}

fn snapshot(path: String) -> Result<GitSnapshot, String> {
    let root = run(&path, &["rev-parse", "--show-toplevel"])?
        .trim()
        .to_string();
    let branch = run(&root, &["branch", "--show-current"])?
        .trim()
        .to_string();
    let branches = run(&root, &["branch", "--format=%(refname:short)"])?
        .lines()
        .map(str::to_string)
        .collect();
    let status = run(&root, &["status", "--porcelain=v1", "-z"])?;
    let mut files = Vec::new();
    let mut parts = status.split('\0');
    while let Some(item) = parts.next() {
        if item.is_empty() {
            break;
        }
        if item.len() < 4 {
            continue;
        }
        let code = &item[..2];
        let file_path = item[3..].to_string();
        if code.contains('R') || code.contains('C') {
            parts.next();
        }
        files.push(GitFile {
            path: file_path,
            staged: code.as_bytes()[0] != b' ' && code != "??",
            changed: code.as_bytes()[1] != b' ' || code == "??",
            status: code.to_string(),
        });
    }
    let commits = if has_commits(&root)? {
        commit_log(&root, 0)?
    } else {
        Vec::new()
    };
    let tracking = run(
        &root,
        &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
    )
    .ok();
    let counts: Vec<u32> = tracking
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|part| part.parse().ok())
        .collect();
    let remote = run(&root, &["remote", "get-url", "origin"])
        .ok()
        .map(|value| redact_remote(value.trim()));
    let tags = run(&root, &["tag", "--list"])?
        .lines()
        .map(str::to_string)
        .collect();
    let stashes = run(&root, &["stash", "list", "--format=%gd%x1f%s"])?
        .lines()
        .map(str::to_string)
        .collect();
    let remotes = run(&root, &["remote"])?
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let remote_urls = remotes
        .iter()
        .map(|name| {
            run(&root, &["remote", "get-url", name]).map(|url| GitRemote {
                name: name.clone(),
                url: redact_remote(url.trim()),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let remote_branches = run(
        &root,
        &["for-each-ref", "refs/remotes", "--format=%(refname:short)"],
    )?
    .lines()
    .filter(|branch| !branch.ends_with("/HEAD"))
    .map(str::to_string)
    .collect();
    Ok(GitSnapshot {
        path: root.clone(),
        name: Path::new(&root)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        branch: if branch.is_empty() {
            "Detached HEAD".into()
        } else {
            branch
        },
        branches,
        files,
        commits,
        ahead: *counts.first().unwrap_or(&0),
        behind: *counts.get(1).unwrap_or(&0),
        remote,
        tags,
        stashes,
        remotes,
        remote_urls,
        remote_branches,
    })
}

fn redact_remote(remote: &str) -> String {
    if let Ok(mut url) = url::Url::parse(remote) {
        if matches!(url.scheme(), "http" | "https")
            && (!url.username().is_empty() || url.password().is_some())
        {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            return url.to_string();
        }
    }
    remote.to_owned()
}

#[tauri::command]
pub fn git_diff(path: String, file: String, staged: bool) -> Result<String, String> {
    let target = checked_repo_file(&path, &file)?;
    if !staged && run(&path, &["ls-files", "--error-unmatch", "--", &file]).is_err() {
        let root = fs::canonicalize(run(&path, &["rev-parse", "--show-toplevel"])?.trim())
            .map_err(|error| format!("Could not inspect repository: {error}"))?;
        let target = fs::canonicalize(&target)
            .map_err(|error| format!("Could not open untracked file: {error}"))?;
        if !target.starts_with(root) {
            return Err("The untracked file points outside the repository.".into());
        }
        let metadata = fs::symlink_metadata(&target)
            .map_err(|error| format!("Could not inspect untracked file: {error}"))?;
        if !metadata.is_file() {
            return Err("Choose an untracked regular file to inspect.".into());
        }
        if metadata.len() > 2_000_000 {
            return Err(
                "This untracked file is larger than 2 MB. Open it in a dedicated editor.".into(),
            );
        }
        let output = crate::platform::command("git")
            .arg("-C")
            .arg(&path)
            .args(["diff", "--no-index", "--", "/dev/null"])
            .arg(&target)
            .output()
            .map_err(|error| format!("Git could not start: {error}"))?;
        if output.status.code() == Some(1) {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
    }
    if staged {
        run(&path, &["diff", "--cached", "--", &file])
    } else {
        run(&path, &["diff", "--", &file])
    }
}

fn diff_hunks(path: &str, file: &str, staged: bool) -> Result<(String, Vec<String>), String> {
    checked_repo_file(path, file)?;
    let diff = if staged {
        run(path, &["diff", "--cached", "--no-ext-diff", "--", file])?
    } else {
        run(path, &["diff", "--no-ext-diff", "--", file])?
    };
    let mut header = String::new();
    let mut hunks = Vec::new();
    for line in diff.split_inclusive('\n') {
        if line.starts_with("@@ ") {
            hunks.push(String::new());
        }
        if let Some(hunk) = hunks.last_mut() {
            hunk.push_str(line);
        } else {
            header.push_str(line);
        }
    }
    Ok((header, hunks))
}

#[tauri::command]
pub fn git_stage_hunk(
    path: String,
    file: String,
    index: usize,
    staged: bool,
) -> Result<(), String> {
    let (header, hunks) = diff_hunks(&path, &file, staged)?;
    let hunk = hunks
        .get(index)
        .ok_or("That diff section no longer exists. Refresh the repository and try again.")?;
    let patch = format!("{header}{hunk}");
    apply_index_patch(&path, &patch, staged)
}

#[tauri::command]
pub fn git_stage_line(
    path: String,
    file: String,
    hunk_index: usize,
    line_index: usize,
    staged: bool,
) -> Result<(), String> {
    let (header, hunks) = diff_hunks(&path, &file, staged)?;
    let hunk = hunks
        .get(hunk_index)
        .ok_or("That diff section no longer exists. Refresh the repository and try again.")?;
    if hunk.contains("\\ No newline at end of file") {
        return Err(
            "Stage this section as a whole because it contains a line without a final newline."
                .into(),
        );
    }
    let mut lines = hunk.split_inclusive('\n');
    let heading = lines.next().ok_or("The diff section is empty.")?;
    let mut selected = false;
    let mut partial = String::from(heading);
    for (index, line) in lines.enumerate() {
        let prefix = line.as_bytes().first().copied();
        if index == line_index {
            if !matches!(prefix, Some(b'+') | Some(b'-')) {
                return Err("Choose an added or removed line to stage.".into());
            }
            selected = true;
            partial.push_str(line);
        } else if prefix == Some(b' ') {
            partial.push_str(line);
        } else if (staged && prefix == Some(b'+')) || (!staged && prefix == Some(b'-')) {
            partial.push(' ');
            partial.push_str(&line[1..]);
        }
    }
    if !selected {
        return Err("That changed line no longer exists. Refresh the diff and try again.".into());
    }
    apply_index_patch(&path, &format!("{header}{partial}"), staged)
}

fn apply_index_patch(path: &str, patch: &str, staged: bool) -> Result<(), String> {
    let mut command = crate::platform::command("git");
    command
        .arg("-C")
        .arg(path)
        .args(["apply", "--cached", "--recount"]);
    if staged {
        command.arg("--reverse");
    }
    let mut child = command
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Git could not start: {error}"))?;
    child
        .stdin
        .take()
        .ok_or("Git patch input is unavailable.")?
        .write_all(patch.as_bytes())
        .map_err(|error| format!("Could not send diff to Git: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("Git patch failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not {} diff section: {}",
            if staged { "unstage" } else { "stage" },
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[tauri::command]
pub fn git_stage(path: String, file: String) -> Result<(), String> {
    checked_repo_file(&path, &file)?;
    run(&path, &["add", "--", &file]).map(|_| ())
}

#[tauri::command]
pub fn git_unstage(path: String, file: String) -> Result<(), String> {
    checked_repo_file(&path, &file)?;
    run(&path, &["restore", "--staged", "--", &file]).map(|_| ())
}

#[tauri::command]
pub fn git_commit(path: String, message: String) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("Write a commit message before committing.".into());
    }
    run(&path, &["commit", "-m", message.trim()])
}

#[tauri::command]
pub fn git_operation(
    path: String,
    operation: String,
    value: Option<String>,
    confirmed: bool,
) -> Result<String, String> {
    let value = value.as_deref().unwrap_or("").trim();
    if value.starts_with('-') {
        return Err(
            "Git targets cannot begin with a dash. Enter a branch, tag, commit, or remote name."
                .into(),
        );
    }
    let required = || {
        if value.is_empty() {
            Err(format!("Enter a value for {operation}."))
        } else {
            Ok(value)
        }
    };
    match operation.as_str() {
        "stage_all" => run(&path, &["add", "-A"]),
        "unstage_all" => run(&path, &["restore", "--staged", "."]),
        "amend" => run(&path, &["commit", "--amend", "--no-edit"]),
        "stash" => {
            if value.is_empty() {
                run(&path, &["stash", "push"])
            } else {
                run(&path, &["stash", "push", "-m", value])
            }
        }
        "stash_apply" => run(&path, &["stash", "apply", required()?]),
        "stash_pop" => run(&path, &["stash", "pop", required()?]),
        "stash_drop" => run(&path, &["stash", "drop", required()?]),
        "branch_rename" => {
            run(&path, &["check-ref-format", "--branch", required()?])?;
            run(&path, &["branch", "-m", value])
        }
        "branch_delete" => {
            let name = required()?;
            crate::git_extra::remember_deleted_branch(&path, name)?;
            run(&path, &["branch", "-d", name])
        }
        "tag_create" => {
            let reference = format!("refs/tags/{}", required()?);
            run(&path, &["check-ref-format", &reference])?;
            run(&path, &["tag", value])
        }
        "tag_delete" => run(&path, &["tag", "-d", required()?]),
        "branch_at" => {
            let (name, commit) = required()?
                .split_once(' ')
                .ok_or("Enter the new branch name followed by the commit.")?;
            let commit = commit.trim();
            if name.starts_with('-') || commit.is_empty() || commit.starts_with('-') {
                return Err("Enter the new branch name followed by the commit.".into());
            }
            run(&path, &["check-ref-format", "--branch", name])?;
            run(&path, &["branch", name, commit])
        }
        "checkout_detached" => run(&path, &["switch", "--detach", required()?]),
        "merge" => run(&path, &["merge", "--no-edit", required()?]),
        "merge_no_ff" => run(&path, &["merge", "--no-edit", "--no-ff", required()?]),
        "merge_squash" => run(&path, &["merge", "--squash", required()?]),
        "merge_abort" => run(&path, &["merge", "--abort"]),
        "rebase" => run(&path, &["rebase", required()?]),
        "rebase_continue" => run(&path, &["rebase", "--continue"]),
        "rebase_abort" => run(&path, &["rebase", "--abort"]),
        "cherry_pick" => run(&path, &["cherry-pick", required()?]),
        "cherry_pick_abort" => run(&path, &["cherry-pick", "--abort"]),
        "revert" => run(&path, &["revert", "--no-edit", required()?]),
        "reset_soft" => run(&path, &["reset", "--soft", required()?]),
        "reset_mixed" => run(&path, &["reset", "--mixed", required()?]),
        "reset_hard" => {
            if !confirmed {
                return Err(
                    "Confirm that you want to discard local changes before a hard reset.".into(),
                );
            }
            run(&path, &["reset", "--hard", required()?])
        }
        "discard" => {
            if !confirmed {
                return Err("Confirm that you want to discard this file's changes.".into());
            }
            if run(&path, &["ls-files", "--cached", "-z", "--", required()?])?.is_empty() {
                let target = checked_repo_file(&path, value.trim_end_matches('/'))?;
                let root = fs::canonicalize(run(&path, &["rev-parse", "--show-toplevel"])?.trim())
                    .map_err(|error| format!("Could not inspect repository: {error}"))?;
                let parent = target
                    .parent()
                    .ok_or("Untracked file has no parent folder.")?;
                let parent = fs::canonicalize(parent)
                    .map_err(|error| format!("Could not inspect untracked file folder: {error}"))?;
                if !parent.starts_with(root) {
                    return Err("The untracked file points outside the repository.".into());
                }
                let metadata = fs::symlink_metadata(&target)
                    .map_err(|error| format!("Could not inspect untracked file: {error}"))?;
                if metadata.is_dir() {
                    fs::remove_dir_all(&target)
                        .map_err(|error| format!("Could not delete untracked folder: {error}"))?;
                    return Ok("Untracked folder deleted.".into());
                }
                fs::remove_file(&target)
                    .map_err(|error| format!("Could not delete untracked file: {error}"))?;
                return Ok("Untracked file deleted.".into());
            }
            run(&path, &["restore", "--", required()?])
        }
        "conflict_ours" | "conflict_theirs" => {
            let side = if operation == "conflict_ours" {
                "--ours"
            } else {
                "--theirs"
            };
            run(&path, &["checkout", side, "--", required()?])?;
            run(&path, &["add", "--", value])
        }
        "remote_add" => {
            let (name, url) = value
                .split_once(' ')
                .ok_or("Enter the remote name followed by its URL.")?;
            if name.is_empty() || url.trim().is_empty() || url.trim().starts_with('-') {
                return Err("Enter the remote name followed by its URL.".into());
            }
            reject_embedded_credentials(url.trim())?;
            run(&path, &["remote", "add", name, url.trim()])
        }
        "remote_remove" => run(&path, &["remote", "remove", required()?]),
        "remote_set_url" => {
            let value = required()?;
            let (name, url) = value
                .split_once(' ')
                .ok_or("Enter the remote name followed by its new URL.")?;
            if name.is_empty() || url.trim().is_empty() {
                return Err("Enter the remote name followed by its new URL.".into());
            }
            reject_embedded_credentials(url.trim())?;
            run(&path, &["remote", "set-url", name, url.trim()])
        }
        "remote_rename" => {
            let value = required()?;
            let (old, new) = value
                .split_once(' ')
                .ok_or("Enter the current remote name followed by its new name.")?;
            if old.is_empty() || new.trim().is_empty() || new.trim().starts_with('-') {
                return Err("Enter the current remote name followed by its new name.".into());
            }
            run(&path, &["remote", "rename", old, new.trim()])
        }
        _ => Err(format!("Unknown Git action: {operation}")),
    }
}

#[tauri::command]
pub fn git_branch_create(path: String, name: String) -> Result<(), String> {
    if name.trim().is_empty() || name.starts_with('-') {
        return Err("Enter a branch name.".into());
    }
    run(&path, &["check-ref-format", "--branch", &name])?;
    run(&path, &["switch", "-c", &name]).map(|_| ())
}

#[tauri::command]
pub fn git_checkout(path: String, name: String) -> Result<(), String> {
    if name.trim().is_empty() || name.starts_with('-') {
        return Err("Choose a valid branch to check out.".into());
    }
    run(&path, &["switch", &name]).map(|_| ())
}

#[tauri::command]
pub fn git_checkout_remote(path: String, branch: String) -> Result<(), String> {
    if branch.starts_with('-') || !branch.contains('/') || branch.ends_with("/HEAD") {
        return Err("Choose a valid remote branch to track.".into());
    }
    let available = run(
        &path,
        &["for-each-ref", "refs/remotes", "--format=%(refname:short)"],
    )?;
    if !available.lines().any(|name| name == branch) {
        return Err("That remote branch is no longer available. Fetch and try again.".into());
    }
    run(&path, &["switch", "--track", &branch]).map(|_| ())
}

async fn network_git(path: String, args: &'static [&'static str]) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if let Ok(url) = run(&path, &["config", "--get", "remote.origin.url"]) {
            reject_embedded_credentials(url.trim())?;
        }
        let auth = forge::git_auth_for_repo(&path)?;
        run_authenticated(&path, args, auth.as_ref())
    })
    .await
    .map_err(|error| format!("Git operation stopped unexpectedly: {error}"))?
}

fn reject_embedded_credentials(remote: &str) -> Result<(), String> {
    if let Ok(url) = url::Url::parse(remote) {
        if matches!(url.scheme(), "http" | "https")
            && (!url.username().is_empty() || url.password().is_some())
        {
            return Err("Remove embedded credentials from the remote URL. Connect your account in Weave instead.".into());
        }
    }
    Ok(())
}

pub(crate) fn run_authenticated(
    path: &str,
    args: &[&str],
    auth: Option<&forge::GitAuth>,
) -> Result<String, String> {
    let Some(auth) = auth else {
        return run(path, args);
    };
    if !Path::new(path).is_dir() {
        return Err(format!("Repository folder does not exist: {path}"));
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate Weave's Git credential helper: {error}"))?;
    let output = crate::platform::command("git")
        .arg("-c")
        .arg("credential.helper=")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_ASKPASS", executable)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("WEAVE_GIT_ASKPASS", "1")
        .env("WEAVE_GIT_USERNAME", &auth.username)
        .env("WEAVE_GIT_HOST", &auth.host)
        .env("WEAVE_GIT_TOKEN", auth.token.as_str())
        .output()
        .map_err(|error| format!("Git could not start: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if message.is_empty() {
            format!("Git operation failed: git {}", args.join(" "))
        } else {
            message
        })
    }
}

#[tauri::command]
pub async fn git_fetch(path: String) -> Result<String, String> {
    network_git(path, &["fetch", "--prune"]).await
}

#[tauri::command]
pub async fn git_pull(path: String, mode: Option<String>) -> Result<String, String> {
    let args: &'static [&'static str] = match mode.as_deref().unwrap_or("ff-only") {
        "ff-only" => &["pull", "--ff-only"],
        "rebase" => &["pull", "--rebase"],
        "merge" => &["pull", "--no-rebase"],
        _ => return Err("Choose fast-forward only, rebase or merge for pulling.".into()),
    };
    network_git(path, args).await
}

#[tauri::command]
pub async fn git_push(path: String) -> Result<String, String> {
    network_git(path, &["push"]).await
}

#[tauri::command]
pub async fn git_publish(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let branch = run(&path, &["branch", "--show-current"])?;
        let branch = branch.trim();
        if branch.is_empty() {
            return Err("Check out a branch before publishing it.".into());
        }
        let remote = run(&path, &["remote", "get-url", "origin"])
            .map_err(|_| "Add an origin remote before publishing this branch.")?;
        reject_embedded_credentials(remote.trim())?;
        let auth = forge::git_auth_for_repo(&path)?;
        run_authenticated(&path, &["push", "-u", "origin", branch], auth.as_ref())
    })
    .await
    .map_err(|error| format!("Publish stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn git_flow_publish(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let info = crate::git_flow::git_flow_info(path.clone())?;
        if !info.initialized {
            return Err("Set up Gitflow before publishing its branches.".into());
        }
        let remote = run(&path, &["remote", "get-url", "origin"])
            .map_err(|_| "Add an origin remote before publishing Gitflow.")?;
        reject_embedded_credentials(remote.trim())?;
        let mut refs = vec![
            format!(
                "refs/heads/{}:refs/heads/{}",
                info.main_branch, info.main_branch
            ),
            format!(
                "refs/heads/{}:refs/heads/{}",
                info.develop_branch, info.develop_branch
            ),
        ];
        refs.extend(
            info.publish_tags
                .iter()
                .map(|tag| format!("refs/tags/{tag}:refs/tags/{tag}")),
        );
        let mut args = vec!["push", "--atomic", "origin"];
        args.extend(refs.iter().map(String::as_str));
        let auth = forge::git_auth_for_repo(&path)?;
        run_authenticated(&path, &args, auth.as_ref())
    })
    .await
    .map_err(|error| format!("Gitflow publish stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub fn git_init(path: String) -> Result<(), String> {
    run(&path, &["init"]).map(|_| ())
}

#[tauri::command]
pub async fn git_clone(url: String, destination: String, depth: Option<u32>) -> Result<(), String> {
    if url.trim().is_empty() {
        return Err("Enter a repository URL to clone.".into());
    }
    if Path::new(&destination).exists() {
        return Err("The destination folder already exists. Choose another name.".into());
    }
    if let Ok(parsed) = url::Url::parse(&url) {
        if matches!(parsed.scheme(), "http" | "https")
            && (!parsed.username().is_empty() || parsed.password().is_some())
        {
            return Err("Remove embedded credentials from the clone URL. Connect your account in Weave instead.".into());
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        let parent = Path::new(&destination)
            .parent()
            .ok_or("Choose a destination inside an existing folder.")?;
        if !parent.is_dir() {
            return Err("The destination's parent folder does not exist.".into());
        }
        let auth = forge::git_auth_for_url(&url)?;
        let mut command = crate::platform::command("git");
        if let Some(auth) = auth.as_ref() {
            let executable = std::env::current_exe().map_err(|error| {
                format!("Could not locate Weave's Git credential helper: {error}")
            })?;
            command
                .arg("-c")
                .arg("credential.helper=")
                .env("GIT_ASKPASS", executable)
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("WEAVE_GIT_ASKPASS", "1")
                .env("WEAVE_GIT_USERNAME", &auth.username)
                .env("WEAVE_GIT_HOST", &auth.host)
                .env("WEAVE_GIT_TOKEN", auth.token.as_str());
        }
        command.arg("clone");
        if let Some(depth) = depth.filter(|depth| *depth > 0) {
            command.arg(format!("--depth={depth}"));
        }
        let output = command
            .arg("--")
            .arg(&url)
            .arg(&destination)
            .output()
            .map_err(|error| format!("Git could not start: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    })
    .await
    .map_err(|error| format!("Clone stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_files_a_commit_changed_with_renames() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_string_lossy().to_string();
        run(&path, &["init", "-b", "main"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        std::fs::write(
            root.path().join("old name.txt"),
            "same content for rename detection\n".repeat(5),
        )
        .unwrap();
        std::fs::write(root.path().join("gone.txt"), "x").unwrap();
        run(&path, &["add", "."]).unwrap();
        run(&path, &["commit", "-m", "First"]).unwrap();
        run(&path, &["mv", "old name.txt", "new ñame.txt"]).unwrap();
        run(&path, &["rm", "-q", "gone.txt"]).unwrap();
        std::fs::write(root.path().join("added.txt"), "y").unwrap();
        run(&path, &["add", "."]).unwrap();
        run(&path, &["commit", "-m", "Second"]).unwrap();
        let commit = run(&path, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let mut files = git_commit_files(path.clone(), commit).unwrap();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(
            files,
            [
                CommitFile {
                    status: "A".into(),
                    path: "added.txt".into(),
                    old_path: None
                },
                CommitFile {
                    status: "D".into(),
                    path: "gone.txt".into(),
                    old_path: None
                },
                CommitFile {
                    status: "R".into(),
                    path: "new ñame.txt".into(),
                    old_path: Some("old name.txt".into())
                },
            ]
        );
        assert!(git_commit_files(path, "not-a-commit".into()).is_err());
    }
    use std::fs;

    #[test]
    fn creates_lists_and_safely_removes_worktrees() {
        let root = std::env::temp_dir().join(format!("weave-worktrees-{}", uuid::Uuid::new_v4()));
        let repository = root.join("repository");
        let linked = root.join("feature copy");
        fs::create_dir_all(&repository).unwrap();
        let path = repository.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        fs::write(repository.join("readme.txt"), "first").unwrap();
        run(&path, &["add", "readme.txt"]).unwrap();
        run(&path, &["commit", "-m", "First commit"]).unwrap();

        let destination = linked.to_string_lossy().to_string();
        assert!(git_worktree_create(path.clone(), path.clone(), "feature".into(), true).is_err());
        let created =
            git_worktree_create(path.clone(), destination.clone(), "feature".into(), true).unwrap();
        assert_eq!(
            created,
            fs::canonicalize(&linked).unwrap().to_string_lossy()
        );
        let listed = git_worktrees(path.clone()).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed[0].main);
        assert_eq!(listed[1].branch, "feature");
        assert_eq!(listed[1].path, created);
        assert!(git_worktree_remove(path.clone(), path.clone()).is_err());
        let locked = git_worktree_lock(path.clone(), destination.clone(), true).unwrap();
        assert!(locked[1].locked);
        assert!(git_worktree_remove(path.clone(), destination.clone()).is_err());
        let unlocked = git_worktree_lock(path.clone(), destination.clone(), false).unwrap();
        assert!(!unlocked[1].locked);

        fs::write(linked.join("untracked.txt"), "keep").unwrap();
        assert!(git_worktree_remove(path.clone(), destination.clone()).is_err());
        assert!(linked.join("untracked.txt").exists());
        fs::remove_file(linked.join("untracked.txt")).unwrap();
        git_worktree_remove(path.clone(), destination.clone()).unwrap();
        assert!(!linked.exists());
        git_worktree_create(path.clone(), destination.clone(), "feature".into(), false).unwrap();
        assert_eq!(git_worktrees(path.clone()).unwrap()[1].branch, "feature");
        git_worktree_remove(path.clone(), destination).unwrap();
        assert_eq!(git_worktrees(path).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remote_management_lists_urls_and_hides_embedded_credentials() {
        let root = std::env::temp_dir().join(format!("weave-remotes-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        assert!(git_operation(
            path.clone(),
            "remote_add".into(),
            Some("origin https://secret@github.com/a/b.git".into()),
            false
        )
        .is_err());
        git_operation(
            path.clone(),
            "remote_add".into(),
            Some("origin https://github.com/a/b.git".into()),
            false,
        )
        .unwrap();
        git_operation(
            path.clone(),
            "remote_set_url".into(),
            Some("origin git@gitlab.com:a/b.git".into()),
            false,
        )
        .unwrap();
        let state = snapshot(path.clone()).unwrap();
        assert_eq!(state.remote_urls[0].name, "origin");
        assert_eq!(state.remote_urls[0].url, "git@gitlab.com:a/b.git");
        git_operation(
            path.clone(),
            "remote_rename".into(),
            Some("origin upstream".into()),
            false,
        )
        .unwrap();
        assert_eq!(
            snapshot(path.clone()).unwrap().remote_urls[0].name,
            "upstream"
        );
        assert_eq!(
            redact_remote("https://user:secret@github.com/a/b.git"),
            "https://github.com/a/b.git"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saves_a_repository_commit_identity_and_uses_it_for_commits() {
        let root = std::env::temp_dir().join(format!("weave-git-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        assert!(
            git_identity_save(path.clone(), "local".into(), "".into(), "".into())
                .unwrap_err()
                .contains("name and email")
        );
        let saved = git_identity_save(
            path.clone(),
            "local".into(),
            "Weave Person".into(),
            "weave@example.invalid".into(),
        )
        .unwrap();
        assert_eq!(saved.local_name.as_deref(), Some("Weave Person"));
        assert_eq!(saved.local_email.as_deref(), Some("weave@example.invalid"));
        fs::write(root.join("hello.txt"), "hello\n").unwrap();
        git_stage(path.clone(), "hello.txt".into()).unwrap();
        git_commit(path.clone(), "Identity works".into()).unwrap();
        assert_eq!(
            run(&path, &["log", "-1", "--format=%an <%ae>"])
                .unwrap()
                .trim(),
            "Weave Person <weave@example.invalid>"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn searches_commit_messages_authors_and_changed_content() {
        let root = std::env::temp_dir().join(format!("weave-git-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        assert!(snapshot(path.clone()).unwrap().commits.is_empty());
        assert!(git_commits_page(path.clone(), 0).unwrap().is_empty());
        assert!(
            git_search(path.clone(), "anything".into(), "message".into())
                .unwrap()
                .is_empty()
        );
        run(&path, &["config", "user.name", "Weave Author"]).unwrap();
        run(&path, &["config", "user.email", "test@example.invalid"]).unwrap();
        fs::write(root.join("notes.txt"), "first line\n").unwrap();
        git_stage(path.clone(), "notes.txt".into()).unwrap();
        git_commit(path.clone(), "Create notes".into()).unwrap();
        fs::write(root.join("notes.txt"), "first line\nunique needle\n").unwrap();
        git_stage(path.clone(), "notes.txt".into()).unwrap();
        git_commit(path.clone(), "Add research detail".into()).unwrap();

        for (mode, query) in [
            ("message", "research"),
            ("author", "Weave Author"),
            ("content", "unique needle"),
        ] {
            let results = git_search(path.clone(), query.into(), mode.into()).unwrap();
            assert!(results
                .iter()
                .any(|item| item.subject == "Add research detail"));
        }
        assert!(
            git_search(path.clone(), "no such commit".into(), "message".into())
                .unwrap()
                .is_empty()
        );
        assert!(git_search(path, "needle".into(), "invalid".into()).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn follows_file_renames_and_attributes_lines_to_commits() {
        let root =
            std::env::temp_dir().join(format!("weave-file-history-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "test@example.invalid"]).unwrap();
        fs::write(
            root.join("old.txt"),
            "\tfirst line\nsecond line\nthird line\nfourth line\nfifth line\n",
        )
        .unwrap();
        run(&path, &["add", "old.txt"]).unwrap();
        run(&path, &["commit", "-m", "Create file"]).unwrap();
        run(&path, &["mv", "old.txt", "new.txt"]).unwrap();
        fs::write(
            root.join("new.txt"),
            "\tfirst line\nupdated line\nthird line\nfourth line\nfifth line\n",
        )
        .unwrap();
        run(&path, &["add", "new.txt"]).unwrap();
        run(&path, &["commit", "-m", "Rename and edit file"]).unwrap();

        let history = git_file_history(path.clone(), "new.txt".into()).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].subject, "Rename and edit file");
        assert_eq!(history[1].subject, "Create file");
        let blame = git_blame_page(path.clone(), "new.txt".into(), 1).unwrap();
        assert_eq!(blame.lines.len(), 5);
        assert!(!blame.has_more);
        assert_eq!(blame.lines[0].number, 1);
        assert_eq!(blame.lines[0].content, "\tfirst line");
        assert_eq!(blame.lines[1].summary, "Rename and edit file");
        let lines = git_line_history(path.clone(), "new.txt".into(), 2, 2).unwrap();
        assert!(!lines.is_empty());
        assert_eq!(lines[0].commit.subject, "Rename and edit file");
        assert!(lines[0].patch.contains("updated line"));
        assert!(git_line_history(path.clone(), "new.txt".into(), 0, 2).is_err());
        assert!(git_line_history(path.clone(), "new.txt".into(), 2, 502).is_err());
        assert!(git_file_history(path.clone(), "../outside".into()).is_err());
        assert!(git_blame_page(path, "../outside".into(), 1).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pages_blame_beyond_previous_file_and_line_limits() {
        let root = std::env::temp_dir().join(format!("weave-large-blame-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "test@example.invalid"]).unwrap();
        let contents = (1..=6000)
            .map(|line| format!("{line:04} {}\n", "x".repeat(190)))
            .collect::<String>();
        fs::write(root.join("large.txt"), contents).unwrap();
        run(&path, &["add", "large.txt"]).unwrap();
        run(&path, &["commit", "-m", "Add large file"]).unwrap();

        let first = git_blame_page(path.clone(), "large.txt".into(), 1).unwrap();
        assert_eq!(first.lines.len(), 250);
        assert!(first.has_more);
        let middle = git_blame_page(path.clone(), "large.txt".into(), 5500).unwrap();
        assert_eq!(middle.lines.first().unwrap().number, 5500);
        assert_eq!(middle.lines.last().unwrap().number, 5749);
        assert!(middle.has_more);
        let last = git_blame_page(path.clone(), "large.txt".into(), 5751).unwrap();
        assert_eq!(last.lines.len(), 250);
        assert!(!last.has_more);
        assert_eq!(last.lines.last().unwrap().number, 6000);
        assert!(git_blame_page(path.clone(), "large.txt".into(), 0).is_err());
        assert!(git_blame_page(path, "large.txt".into(), 6001).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manages_lfs_tracking_in_a_local_repository() {
        let root = std::env::temp_dir().join(format!("weave-lfs-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        if require_lfs(&path).is_err() {
            assert!(git_lfs_info(path.clone())
                .unwrap_err()
                .contains("not installed"));
            fs::remove_dir_all(root).unwrap();
            return;
        }
        run(&path, &["lfs", "install", "--local"]).unwrap();
        assert!(git_lfs_track(path.clone(), "--bad".into(), true).is_err());
        let info = git_lfs_track(path.clone(), "*.bin".into(), true).unwrap();
        assert!(info.tracked_patterns.contains("*.bin"));
        assert!(fs::read_to_string(root.join(".gitattributes"))
            .unwrap()
            .contains("filter=lfs"));
        let info = git_lfs_track(path.clone(), "*.bin".into(), false).unwrap();
        assert!(!info.tracked_patterns.contains("*.bin"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn discards_tracked_changes_and_safely_deletes_untracked_files() {
        let root = std::env::temp_dir().join(format!("weave-git-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "test@example.invalid"]).unwrap();
        fs::write(root.join("tracked.txt"), "original\n").unwrap();
        git_stage(path.clone(), "tracked.txt".into()).unwrap();
        git_commit(path.clone(), "Initial".into()).unwrap();
        fs::write(root.join("tracked.txt"), "changed\n").unwrap();
        git_operation(
            path.clone(),
            "discard".into(),
            Some("tracked.txt".into()),
            true,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("tracked.txt")).unwrap(),
            "original\n"
        );

        fs::write(root.join("new.txt"), "untracked\n").unwrap();
        assert!(git_operation(
            path.clone(),
            "discard".into(),
            Some("new.txt".into()),
            false,
        )
        .is_err());
        assert!(root.join("new.txt").exists());
        git_operation(path.clone(), "discard".into(), Some("new.txt".into()), true).unwrap();
        assert!(!root.join("new.txt").exists());

        fs::create_dir(root.join("folder")).unwrap();
        fs::write(root.join("folder/keep.txt"), "keep").unwrap();
        assert!(git_operation(
            path.clone(),
            "discard".into(),
            Some("folder/".into()),
            false,
        )
        .is_err());
        assert!(root.join("folder/keep.txt").exists());
        git_operation(path.clone(), "discard".into(), Some("folder/".into()), true).unwrap();
        assert!(!root.join("folder").exists());

        #[cfg(unix)]
        {
            let outside =
                std::env::temp_dir().join(format!("weave-outside-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&outside).unwrap();
            fs::write(outside.join("keep.txt"), "keep").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
            assert!(git_operation(
                path.clone(),
                "discard".into(),
                Some("escape/keep.txt".into()),
                true,
            )
            .unwrap_err()
            .contains("outside the repository"));
            assert!(outside.join("keep.txt").exists());
            fs::remove_dir_all(outside).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stages_and_commits_real_git_repository() {
        let root = std::env::temp_dir().join(format!("weave-git-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "test@example.invalid"]).unwrap();
        fs::write(root.join("hello.txt"), "hello").unwrap();
        assert_eq!(snapshot(path.clone()).unwrap().files.len(), 1);
        git_stage(path.clone(), "hello.txt".into()).unwrap();
        assert!(snapshot(path.clone()).unwrap().files[0].staged);
        git_commit(path.clone(), "First commit".into()).unwrap();
        let snapshot = snapshot(path.clone()).unwrap();
        assert!(snapshot.files.is_empty());
        assert_eq!(snapshot.commits.len(), 1);
        assert!(snapshot.commits[0].graph.contains('*'));
        fs::write(root.join("hello.txt"), "hello again").unwrap();
        git_stage(path.clone(), "hello.txt".into()).unwrap();
        git_commit(path.clone(), "Second commit".into()).unwrap();
        let older = git_commits_page(path.clone(), 1).unwrap();
        assert_eq!(older.len(), 1);
        assert_eq!(older[0].subject, "First commit");
        assert!(git_commits_page(path.clone(), 2).unwrap().is_empty());
        assert!(git_commits_page(path, 1_000_001).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_history_beyond_the_first_graph_page() {
        let root = std::env::temp_dir().join(format!("weave-history-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        run(&path, &["config", "commit.gpgsign", "false"]).unwrap();
        run(&path, &["config", "gc.auto", "0"]).unwrap();
        for number in 0..302 {
            run(
                &path,
                &["commit", "--allow-empty", "-m", &format!("Commit {number}")],
            )
            .unwrap();
        }
        let first = git_commits_page(path.clone(), 0).unwrap();
        let next = git_commits_page(path.clone(), 300).unwrap();
        assert_eq!(first.len(), 300);
        assert_eq!(next.len(), 2);
        assert_ne!(first[299].id, next[0].id);
        assert_eq!(next[1].subject, "Commit 0");
        assert!(git_commits_page(path, 302).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires the local SSH Git test server"]
    fn clones_pushes_fetches_and_pulls_over_ssh() {
        let port = std::env::var("WEAVE_SSH_TEST_PORT").unwrap();
        let user = std::env::var("WEAVE_SSH_TEST_USER").unwrap();
        let key = std::env::var("WEAVE_SSH_TEST_KEY").unwrap();
        let known_hosts = std::env::var("WEAVE_SSH_TEST_KNOWN_HOSTS").unwrap();
        let root = std::env::temp_dir().join(format!("weave-ssh-git-{}", uuid::Uuid::new_v4()));
        let remote = root.join("remote.git");
        let source = root.join("source");
        let clone = root.join("clone");
        fs::create_dir_all(&remote).unwrap();
        fs::create_dir_all(&source).unwrap();
        run(remote.to_str().unwrap(), &["init", "--bare"]).unwrap();
        let source_path = source.to_string_lossy().to_string();
        let clone_path = clone.to_string_lossy().to_string();
        run(&source_path, &["init"]).unwrap();
        run(&source_path, &["config", "user.name", "Weave Test"]).unwrap();
        run(
            &source_path,
            &["config", "user.email", "weave@example.test"],
        )
        .unwrap();
        run(&source_path, &["config", "commit.gpgsign", "false"]).unwrap();
        fs::write(source.join("readme.txt"), "first\n").unwrap();
        run(&source_path, &["add", "readme.txt"]).unwrap();
        run(&source_path, &["commit", "-m", "Initial"]).unwrap();
        run(&source_path, &["branch", "-M", "main"]).unwrap();
        run(
            &source_path,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        run(&source_path, &["push", "-u", "origin", "main"]).unwrap();
        run(
            remote.to_str().unwrap(),
            &["symbolic-ref", "HEAD", "refs/heads/main"],
        )
        .unwrap();

        let ssh = format!(
            "ssh -i {key} -o UserKnownHostsFile={known_hosts} -o StrictHostKeyChecking=yes -o BatchMode=yes"
        );
        std::env::set_var("GIT_SSH_COMMAND", ssh);
        let ssh_url = format!("ssh://{user}@127.0.0.1:{port}{}", remote.display());
        tauri::async_runtime::block_on(git_clone(ssh_url, clone_path.clone(), None)).unwrap();
        assert_eq!(
            fs::read_to_string(clone.join("readme.txt")).unwrap(),
            "first\n"
        );
        run(&clone_path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&clone_path, &["config", "user.email", "weave@example.test"]).unwrap();
        run(&clone_path, &["config", "commit.gpgsign", "false"]).unwrap();
        fs::write(clone.join("clone.txt"), "from clone\n").unwrap();
        git_stage(clone_path.clone(), "clone.txt".into()).unwrap();
        git_commit(clone_path.clone(), "Push over SSH".into()).unwrap();
        tauri::async_runtime::block_on(git_push(clone_path.clone())).unwrap();
        let remote_head = run(remote.to_str().unwrap(), &["rev-parse", "main"]).unwrap();
        assert_eq!(
            remote_head.trim(),
            run(&clone_path, &["rev-parse", "HEAD"]).unwrap().trim()
        );

        run(&source_path, &["pull", "--ff-only"]).unwrap();
        fs::write(source.join("source.txt"), "from source\n").unwrap();
        run(&source_path, &["add", "source.txt"]).unwrap();
        run(&source_path, &["commit", "-m", "Update source"]).unwrap();
        run(&source_path, &["push"]).unwrap();
        tauri::async_runtime::block_on(git_fetch(clone_path.clone())).unwrap();
        tauri::async_runtime::block_on(git_pull(clone_path.clone(), Some("rebase".into())))
            .unwrap();
        assert_eq!(
            fs::read_to_string(clone.join("source.txt")).unwrap(),
            "from source\n"
        );
        std::env::remove_var("GIT_SSH_COMMAND");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clones_local_repository() {
        let root = std::env::temp_dir().join(format!("weave-clone-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(&source).unwrap();
        let source_path = source.to_string_lossy().to_string();
        run(&source_path, &["init"]).unwrap();
        run(&source_path, &["config", "user.name", "Weave Test"]).unwrap();
        run(
            &source_path,
            &["config", "user.email", "test@example.invalid"],
        )
        .unwrap();
        fs::write(source.join("readme.txt"), "weave").unwrap();
        run(&source_path, &["add", "readme.txt"]).unwrap();
        run(&source_path, &["commit", "-m", "Initial commit"]).unwrap();
        fs::write(source.join("readme.txt"), "weave 2").unwrap();
        run(&source_path, &["commit", "-am", "Second commit"]).unwrap();
        tauri::async_runtime::block_on(git_clone(
            format!("file://{source_path}"),
            target.to_string_lossy().to_string(),
            Some(1),
        ))
        .unwrap();
        assert_eq!(
            fs::read_to_string(target.join("readme.txt")).unwrap(),
            "weave 2"
        );
        let target_path = target.to_string_lossy().to_string();
        assert_eq!(
            run(&target_path, &["rev-list", "--count", "HEAD"])
                .unwrap()
                .trim(),
            "1"
        );
        assert!(
            tauri::async_runtime::block_on(git_pull(target_path, Some("sideways".into())))
                .unwrap_err()
                .contains("Choose")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fetches_pulls_and_pushes_a_local_remote() {
        let root = std::env::temp_dir().join(format!("weave-git-remote-{}", uuid::Uuid::new_v4()));
        let remote = root.join("remote.git");
        let source = root.join("source");
        let clone = root.join("clone");
        fs::create_dir_all(&source).unwrap();
        let remote_path = remote.to_string_lossy().to_string();
        let source_path = source.to_string_lossy().to_string();
        let clone_path = clone.to_string_lossy().to_string();
        let result = crate::platform::command("git")
            .args(["init", "--bare", &remote_path])
            .output()
            .unwrap();
        assert!(result.status.success());
        run(&source_path, &["init"]).unwrap();
        run(&source_path, &["config", "user.name", "Weave Test"]).unwrap();
        run(
            &source_path,
            &["config", "user.email", "test@example.invalid"],
        )
        .unwrap();
        fs::write(source.join("data.txt"), "first\n").unwrap();
        run(&source_path, &["add", "data.txt"]).unwrap();
        run(&source_path, &["commit", "-m", "First"]).unwrap();
        run(&source_path, &["branch", "-M", "main"]).unwrap();
        run(&source_path, &["remote", "add", "origin", &remote_path]).unwrap();
        run(&source_path, &["push", "-u", "origin", "main"]).unwrap();
        tauri::async_runtime::block_on(git_clone(remote_path.clone(), clone_path.clone(), None))
            .unwrap();
        run(&clone_path, &["switch", "main"]).unwrap();
        run(&clone_path, &["config", "user.name", "Weave Test"]).unwrap();
        run(
            &clone_path,
            &["config", "user.email", "test@example.invalid"],
        )
        .unwrap();

        run(&source_path, &["switch", "-c", "feature"]).unwrap();
        fs::write(source.join("feature.txt"), "published\n").unwrap();
        run(&source_path, &["add", "feature.txt"]).unwrap();
        run(&source_path, &["commit", "-m", "Feature"]).unwrap();
        tauri::async_runtime::block_on(git_publish(source_path.clone())).unwrap();
        run(&source_path, &["switch", "main"]).unwrap();
        tauri::async_runtime::block_on(git_fetch(clone_path.clone())).unwrap();
        assert!(snapshot(clone_path.clone())
            .unwrap()
            .remote_branches
            .contains(&"origin/feature".to_string()));
        git_checkout_remote(clone_path.clone(), "origin/feature".into()).unwrap();
        assert_eq!(snapshot(clone_path.clone()).unwrap().branch, "feature");
        run(&clone_path, &["switch", "main"]).unwrap();

        fs::write(source.join("data.txt"), "second\n").unwrap();
        run(&source_path, &["commit", "-am", "Second"]).unwrap();
        tauri::async_runtime::block_on(git_push(source_path)).unwrap();
        tauri::async_runtime::block_on(git_fetch(clone_path.clone())).unwrap();
        assert_eq!(
            fs::read_to_string(clone.join("data.txt")).unwrap(),
            "first\n"
        );
        tauri::async_runtime::block_on(git_pull(clone_path.clone(), None)).unwrap();
        assert_eq!(
            fs::read_to_string(clone.join("data.txt")).unwrap(),
            "second\n"
        );

        fs::write(clone.join("data.txt"), "third\n").unwrap();
        run(&clone_path, &["commit", "-am", "Third"]).unwrap();
        tauri::async_runtime::block_on(git_push(clone_path)).unwrap();
        assert_eq!(
            run(&remote_path, &["show", "main:data.txt"]).unwrap(),
            "third\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manages_branches_tags_stashes_and_rejects_unconfirmed_reset() {
        let root = std::env::temp_dir().join(format!("weave-git-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "test@example.invalid"]).unwrap();
        fs::write(root.join("data.txt"), "first").unwrap();
        git_stage(path.clone(), "data.txt".into()).unwrap();
        git_commit(path.clone(), "First commit".into()).unwrap();
        git_branch_create(path.clone(), "feature".into()).unwrap();
        git_operation(
            path.clone(),
            "branch_rename".into(),
            Some("renamed".into()),
            false,
        )
        .unwrap();
        git_operation(path.clone(), "tag_create".into(), Some("v1".into()), false).unwrap();
        fs::write(root.join("data.txt"), "changed").unwrap();
        git_operation(
            path.clone(),
            "stash".into(),
            Some("pending work".into()),
            false,
        )
        .unwrap();
        let snapshot = snapshot(path.clone()).unwrap();
        assert_eq!(snapshot.branch, "renamed");
        assert!(snapshot.tags.contains(&"v1".into()));
        assert_eq!(snapshot.stashes.len(), 1);
        assert!(git_operation(
            path.clone(),
            "reset_hard".into(),
            Some("HEAD".into()),
            false
        )
        .is_err());
        git_operation(
            path.clone(),
            "stash_pop".into(),
            Some("stash@{0}".into()),
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("data.txt")).unwrap(),
            "changed"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn shows_untracked_file_diff() {
        let root = std::env::temp_dir().join(format!("weave-git-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        fs::write(root.join("new.txt"), "new file\n").unwrap();
        assert!(git_diff(path, "new.txt".into(), false)
            .unwrap()
            .contains("+new file"));
        let path = root.to_string_lossy().to_string();
        assert!(git_diff(path.clone(), "../outside.txt".into(), false).is_err());
        let outside = std::env::temp_dir().join(format!("weave-outside-{}", uuid::Uuid::new_v4()));
        fs::write(&outside, "private\n").unwrap();
        crate::platform::symlink(&outside, &root.join("link.txt")).unwrap();
        assert!(git_diff(path, "link.txt".into(), false)
            .unwrap_err()
            .contains("outside the repository"));
        fs::remove_file(outside).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stages_and_unstages_one_diff_hunk() {
        let root = std::env::temp_dir().join(format!("weave-git-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "test@example.invalid"]).unwrap();
        let original = (1..=25)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        fs::write(root.join("data.txt"), &original).unwrap();
        git_stage(path.clone(), "data.txt".into()).unwrap();
        git_commit(path.clone(), "Initial".into()).unwrap();
        let changed = original
            .replace("line 2\n", "first change\n")
            .replace("line 22\n", "second change\n");
        fs::write(root.join("data.txt"), changed).unwrap();
        assert_eq!(diff_hunks(&path, "data.txt", false).unwrap().1.len(), 2);
        git_stage_hunk(path.clone(), "data.txt".into(), 0, false).unwrap();
        let staged = git_diff(path.clone(), "data.txt".into(), true).unwrap();
        assert!(staged.contains("first change"));
        assert!(!staged.contains("second change"));
        git_stage_hunk(path.clone(), "data.txt".into(), 0, true).unwrap();
        assert!(git_diff(path.clone(), "data.txt".into(), true)
            .unwrap()
            .is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stages_and_unstages_only_the_selected_line() {
        let root = std::env::temp_dir().join(format!("weave-git-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "test@example.invalid"]).unwrap();
        fs::write(root.join("data.txt"), "one\ntwo\nthree\n").unwrap();
        run(&path, &["add", "data.txt"]).unwrap();
        run(&path, &["commit", "-m", "Initial"]).unwrap();
        fs::write(root.join("data.txt"), "one\nTWO\nthree\nextra\n").unwrap();

        let (_, hunks) = diff_hunks(&path, "data.txt", false).unwrap();
        let line = hunks[0]
            .lines()
            .skip(1)
            .position(|line| line == "+extra")
            .unwrap();
        git_stage_line(path.clone(), "data.txt".into(), 0, line, false).unwrap();
        let staged = git_diff(path.clone(), "data.txt".into(), true).unwrap();
        assert!(staged.contains("+extra"));
        assert!(!staged.contains("+TWO"));
        assert!(!staged.contains("-two"));

        let (_, staged_hunks) = diff_hunks(&path, "data.txt", true).unwrap();
        let line = staged_hunks[0]
            .lines()
            .skip(1)
            .position(|line| line == "+extra")
            .unwrap();
        git_stage_line(path.clone(), "data.txt".into(), 0, line, true).unwrap();
        assert!(git_diff(path.clone(), "data.txt".into(), true)
            .unwrap()
            .is_empty());

        let (_, hunks) = diff_hunks(&path, "data.txt", false).unwrap();
        let line = hunks[0]
            .lines()
            .skip(1)
            .position(|line| line == "-two")
            .unwrap();
        git_stage_line(path.clone(), "data.txt".into(), 0, line, false).unwrap();
        let staged = git_diff(path.clone(), "data.txt".into(), true).unwrap();
        assert!(staged.contains("-two"));
        assert!(!staged.contains("+TWO"));
        assert!(!staged.contains("+extra"));

        let (_, staged_hunks) = diff_hunks(&path, "data.txt", true).unwrap();
        let line = staged_hunks[0]
            .lines()
            .skip(1)
            .position(|line| line == "-two")
            .unwrap();
        git_stage_line(path.clone(), "data.txt".into(), 0, line, true).unwrap();
        assert!(git_diff(path.clone(), "data.txt".into(), true)
            .unwrap()
            .is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_and_resolves_real_merge_conflict() {
        let root = std::env::temp_dir().join(format!("weave-git-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "test@example.invalid"]).unwrap();
        fs::write(root.join("data.txt"), "base\n").unwrap();
        run(&path, &["add", "data.txt"]).unwrap();
        run(&path, &["commit", "-m", "Base"]).unwrap();
        let main = run(&path, &["branch", "--show-current"])
            .unwrap()
            .trim()
            .to_string();
        run(&path, &["switch", "-c", "feature"]).unwrap();
        fs::write(root.join("data.txt"), "theirs\n").unwrap();
        run(&path, &["commit", "-am", "Their change"]).unwrap();
        run(&path, &["switch", &main]).unwrap();
        fs::write(root.join("data.txt"), "ours\n").unwrap();
        run(&path, &["commit", "-am", "Our change"]).unwrap();
        assert!(run(&path, &["merge", "feature"]).is_err());
        let conflict = git_conflict(path.clone(), "data.txt".into()).unwrap();
        assert_eq!(conflict.base.as_deref(), Some("base\n"));
        assert_eq!(conflict.ours.as_deref(), Some("ours\n"));
        assert_eq!(conflict.theirs.as_deref(), Some("theirs\n"));
        assert!(git_open_conflict_external(path.clone(), "../outside.txt".into()).is_err());
        assert!(git_conflict_resolve(path.clone(), "../outside.txt".into(), "bad".into()).is_err());
        git_conflict_resolve(path.clone(), "data.txt".into(), "resolved\n".into()).unwrap();
        assert!(git_open_conflict_external(path.clone(), "data.txt".into()).is_err());
        assert!(run(&path, &["ls-files", "-u"]).unwrap().is_empty());
        assert_eq!(
            fs::read_to_string(root.join("data.txt")).unwrap(),
            "resolved\n"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
