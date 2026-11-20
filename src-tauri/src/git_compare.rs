//! Comparing things: a commit's own changes, two branches or commits, two
//! folders or files outside Git, and the before and after of an image.

use crate::git::{checked_repo_file, run, CommitFile};
use base64::Engine;
use serde::Serialize;
use std::path::Path;

/// Diffs bigger than this are refused rather than freezing the view.
const MAX_DIFF_BYTES: usize = 8_000_000;
const MAX_IMAGE_BYTES: usize = 20_000_000;

fn check_rev(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.starts_with('-')
        || value.contains(['\n', '\r', '\0', ' ', '\t'])
        || value.len() > 256
    {
        return Err("Choose a branch, tag or commit.".into());
    }
    Ok(())
}

fn limited(diff: String) -> Result<String, String> {
    if diff.len() > MAX_DIFF_BYTES {
        return Err("This diff is too large to show. Open it in your diff tool instead.".into());
    }
    Ok(diff)
}

/// Parses `--name-status -z` output (status, path, and a second path for renames and copies).
pub(crate) fn parse_name_status(output: &str) -> Vec<CommitFile> {
    let mut fields = output
        .trim_start_matches('\n')
        .split('\0')
        .filter(|field| !field.is_empty());
    let mut files = Vec::new();
    while let Some(code) = fields.next() {
        let status = code.trim().chars().next().unwrap_or('M').to_string();
        let first = fields.next().unwrap_or_default().to_string();
        files.push(if status == "R" || status == "C" {
            CommitFile {
                status,
                path: fields.next().unwrap_or_default().to_string(),
                old_path: Some(first),
            }
        } else {
            CommitFile {
                status,
                path: first,
                old_path: None,
            }
        });
    }
    files
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfo {
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_email: String,
    pub author_date: String,
    pub committer: String,
    pub committer_date: String,
    pub subject: String,
    pub body: String,
}

#[tauri::command]
pub fn git_commit_info(path: String, commit: String) -> Result<CommitInfo, String> {
    check_rev(&commit)?;
    let output = run(
        &path,
        &[
            "show",
            "--no-patch",
            "--no-color",
            "--date=format:%Y-%m-%d %H:%M",
            "--format=%H%x00%P%x00%an%x00%ae%x00%ad%x00%cn%x00%cd%x00%s%x00%b",
            &commit,
        ],
    )?;
    let fields: Vec<_> = output.splitn(9, '\0').collect();
    if fields.len() != 9 {
        return Err("Git returned commit details Weave could not read.".into());
    }
    Ok(CommitInfo {
        id: fields[0].into(),
        parents: fields[1].split_whitespace().map(String::from).collect(),
        author: fields[2].into(),
        author_email: fields[3].into(),
        author_date: fields[4].into(),
        committer: fields[5].into(),
        committer_date: fields[6].into(),
        subject: fields[7].into(),
        body: fields[8].trim_end().into(),
    })
}

/// One file's changes in a commit, against its first parent.
#[tauri::command]
pub fn git_commit_file_diff(path: String, commit: String, file: String) -> Result<String, String> {
    check_rev(&commit)?;
    checked_repo_file(&path, &file)?;
    limited(run(
        &path,
        &[
            "show",
            "--no-color",
            "--no-ext-diff",
            "--diff-merges=first-parent",
            "-M",
            "--format=",
            &commit,
            "--",
            &file,
        ],
    )?)
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub files: Vec<CommitFile>,
    /// Commits in `target` that are not in `base`.
    pub ahead: usize,
    /// Commits in `base` that are not in `target`.
    pub behind: usize,
    /// The commit the left side of the diff is taken from.
    pub left: String,
}

fn range(base: &str, target: &str, since_fork: bool) -> String {
    if since_fork {
        format!("{base}...{target}")
    } else {
        format!("{base}..{target}")
    }
}

/// Files that differ between two branches, tags or commits. With
/// `since_fork` only the changes made on `target` since it split from `base`
/// are listed, as a pull request would show them.
#[tauri::command]
pub fn git_compare_refs(
    path: String,
    base: String,
    target: String,
    since_fork: bool,
) -> Result<Comparison, String> {
    check_rev(&base)?;
    check_rev(&target)?;
    for rev in [&base, &target] {
        run(
            &path,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{rev}^{{commit}}"),
            ],
        )
        .map_err(|_| format!("Git does not know “{rev}”. Fetch, or check the name."))?;
    }
    let names = run(
        &path,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--name-status",
            "-M",
            "-z",
            &range(&base, &target, since_fork),
        ],
    )?;
    let counts = run(
        &path,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{base}...{target}"),
        ],
    )?;
    let mut numbers = counts.split_whitespace().map(|n| n.parse().unwrap_or(0));
    let behind = numbers.next().unwrap_or(0);
    let ahead = numbers.next().unwrap_or(0);
    let left = if since_fork {
        run(&path, &["merge-base", &base, &target]).map_err(|_| {
            format!("{base} and {target} share no history. Turn off “since they split”.")
        })?
    } else {
        run(&path, &["rev-parse", &base])?
    };
    Ok(Comparison {
        files: parse_name_status(&names),
        ahead,
        behind,
        left: left.trim().to_owned(),
    })
}

#[tauri::command]
pub fn git_compare_file_diff(
    path: String,
    base: String,
    target: String,
    since_fork: bool,
    file: String,
    old_path: Option<String>,
) -> Result<String, String> {
    check_rev(&base)?;
    check_rev(&target)?;
    checked_repo_file(&path, &file)?;
    let range = range(&base, &target, since_fork);
    let mut args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "-M",
        &range,
        "--",
        &file,
    ];
    if let Some(old) = old_path.as_deref() {
        checked_repo_file(&path, old)?;
        args.push(old);
    }
    limited(run(&path, &args)?)
}

/// Compares two files or two folders anywhere on this Mac, like `diff -r`.
#[tauri::command]
pub async fn git_compare_paths(left: String, right: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || compare_paths(&left, &right))
        .await
        .map_err(|error| format!("Comparing stopped unexpectedly: {error}"))?
}

fn compare_paths(left: &str, right: &str) -> Result<String, String> {
    let (left_path, right_path) = (Path::new(left), Path::new(right));
    for path in [left_path, right_path] {
        if !path.is_absolute() || !path.exists() {
            return Err(format!("{} does not exist.", path.display()));
        }
    }
    if left_path.is_dir() != right_path.is_dir() {
        return Err("Compare two files or two folders, not one of each.".into());
    }
    let output = crate::platform::command("git")
        .args([
            "diff",
            "--no-index",
            "--no-color",
            "--no-ext-diff",
            "-M",
            "--",
        ])
        .arg(left_path)
        .arg(right_path)
        .output()
        .map_err(|error| format!("Git could not start: {error}"))?;
    match output.status.code() {
        Some(0) => Ok(String::new()),
        Some(1) => limited(String::from_utf8_lossy(&output.stdout).into_owned()),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageVersion {
    /// A data: URL, or none when the file does not exist on that side.
    pub data_url: Option<String>,
    pub size: usize,
}

fn mime(file: &str) -> Option<&'static str> {
    let extension = Path::new(file).extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        _ => return None,
    })
}

/// An image as it is at `revision`: a commit or branch, `:` for the staged
/// copy, or empty for the file on disk.
#[tauri::command]
pub fn git_image_version(
    path: String,
    revision: String,
    file: String,
) -> Result<ImageVersion, String> {
    let kind = mime(&file).ok_or("This file is not an image Weave can preview.")?;
    let target = checked_repo_file(&path, &file)?;
    let bytes = if revision.is_empty() {
        match std::fs::read(&target) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("Could not read {file}: {error}")),
        }
    } else {
        if revision != ":" {
            check_rev(&revision)?;
        }
        let spec = if revision == ":" {
            format!(":{file}")
        } else {
            format!("{revision}:{file}")
        };
        let output = crate::platform::command("git")
            .arg("-C")
            .arg(&path)
            .args(["cat-file", "blob", &spec])
            .output()
            .map_err(|error| format!("Git could not start: {error}"))?;
        output.status.success().then_some(output.stdout)
    };
    let Some(bytes) = bytes else {
        return Ok(ImageVersion {
            data_url: None,
            size: 0,
        });
    };
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("This image is larger than 20 MB. Open it in Preview instead.".into());
    }
    Ok(ImageVersion {
        size: bytes.len(),
        data_url: Some(format!(
            "data:{kind};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        )),
    })
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BranchDeleteReport {
    pub deleted: Vec<String>,
    /// Branch name and why it was kept.
    pub kept: Vec<(String, String)>,
}

/// Deletes several local branches. Without `force`, branches with commits
/// that are not merged anywhere are kept and reported.
#[tauri::command]
pub fn git_branches_delete(
    path: String,
    names: Vec<String>,
    force: bool,
) -> Result<BranchDeleteReport, String> {
    if names.is_empty() {
        return Err("Choose the branches to delete.".into());
    }
    let current = run(&path, &["branch", "--show-current"])?;
    let mut report = BranchDeleteReport {
        deleted: Vec::new(),
        kept: Vec::new(),
    };
    for name in names {
        if check_rev(&name).is_err() {
            report
                .kept
                .push((name, "This is not a valid branch name.".into()));
            continue;
        }
        if name == current.trim() {
            report
                .kept
                .push((name, "This branch is checked out.".into()));
            continue;
        }
        crate::git_extra::remember_deleted_branch(&path, &name)?;
        match run(&path, &["branch", if force { "-D" } else { "-d" }, &name]) {
            Ok(_) => report.deleted.push(name),
            Err(error) if error.contains("not fully merged") => report
                .kept
                .push((name, "It has commits that are not merged.".into())),
            Err(error) => report.kept.push((name, error)),
        }
    }
    Ok(report)
}

/// "date" lists history by commit date; anything else keeps branches together.
#[tauri::command]
pub fn git_graph_order(path: String) -> Result<String, String> {
    Ok(run(&path, &["config", "--get", "weave.graphOrder"])
        .map(|order| order.trim().to_owned())
        .unwrap_or_else(|_| "topo".into()))
}

#[tauri::command]
pub fn git_graph_order_set(path: String, order: String) -> Result<(), String> {
    match order.as_str() {
        "date" => run(&path, &["config", "weave.graphOrder", "date"]).map(|_| ()),
        "topo" => match run(&path, &["config", "--unset", "weave.graphOrder"]) {
            Ok(_) => Ok(()),
            // Already unset.
            Err(_) if git_graph_order(path.clone())? == "topo" => Ok(()),
            Err(error) => Err(error),
        },
        _ => Err("Choose date or branch order for the history.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        run(path, &["init", "-q", "-b", "main"]).unwrap();
        run(path, &["config", "user.name", "Test"]).unwrap();
        run(path, &["config", "user.email", "test@example.com"]).unwrap();
        run(path, &["config", "commit.gpgsign", "false"]).unwrap();
        dir
    }

    fn commit(path: &str, file: &str, text: &[u8], message: &str) {
        std::fs::write(Path::new(path).join(file), text).unwrap();
        run(path, &["add", "-A"]).unwrap();
        run(path, &["commit", "-q", "-m", message]).unwrap();
    }

    #[test]
    fn shows_commit_details_and_a_files_change() {
        let dir = repo();
        let path = dir.path().to_str().unwrap();
        commit(path, "a.txt", b"one\n", "First");
        commit(path, "a.txt", b"two\n", "Second\n\nWith a body.");
        let info = git_commit_info(path.into(), "HEAD".into()).unwrap();
        assert_eq!(info.subject, "Second");
        assert_eq!(info.body, "With a body.");
        assert_eq!(info.parents.len(), 1);
        assert_eq!(info.author_email, "test@example.com");
        let diff = git_commit_file_diff(path.into(), info.id, "a.txt".into()).unwrap();
        assert!(diff.contains("-one") && diff.contains("+two"));
        assert!(git_commit_info(path.into(), "--all".into()).is_err());
    }

    #[test]
    fn compares_branches_with_counts_and_renames() {
        let dir = repo();
        let path = dir.path().to_str().unwrap();
        commit(
            path,
            "keep.txt",
            b"a long enough line\nanother line\n",
            "Base",
        );
        run(path, &["switch", "-q", "-c", "feature"]).unwrap();
        run(path, &["mv", "keep.txt", "moved.txt"]).unwrap();
        commit(path, "new.txt", b"new\n", "Feature");
        run(path, &["switch", "-q", "main"]).unwrap();
        commit(path, "main.txt", b"main\n", "Main moves on");

        let since_fork =
            git_compare_refs(path.into(), "main".into(), "feature".into(), true).unwrap();
        assert_eq!((since_fork.ahead, since_fork.behind), (1, 1));
        assert_eq!(
            since_fork.left,
            run(path, &["rev-parse", "main~1"]).unwrap().trim()
        );
        assert!(since_fork.files.iter().all(|file| file.path != "main.txt"));
        assert!(since_fork.files.contains(&CommitFile {
            status: "R".into(),
            path: "moved.txt".into(),
            old_path: Some("keep.txt".into()),
        }));
        let direct = git_compare_refs(path.into(), "main".into(), "feature".into(), false).unwrap();
        assert!(direct.files.iter().any(|file| file.path == "main.txt"));
        let diff = git_compare_file_diff(
            path.into(),
            "main".into(),
            "feature".into(),
            true,
            "new.txt".into(),
            None,
        )
        .unwrap();
        assert!(diff.contains("+new"));
        assert!(
            git_compare_refs(path.into(), "main".into(), "missing".into(), true)
                .unwrap_err()
                .contains("does not know")
        );
    }

    #[test]
    fn compares_folders_outside_git() {
        let dir = tempfile::tempdir().unwrap();
        let (left, right) = (dir.path().join("left"), dir.path().join("right"));
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir_all(&right).unwrap();
        std::fs::write(left.join("same.txt"), "same\n").unwrap();
        std::fs::write(right.join("same.txt"), "same\n").unwrap();
        assert_eq!(
            compare_paths(left.to_str().unwrap(), right.to_str().unwrap()).unwrap(),
            ""
        );
        std::fs::write(right.join("same.txt"), "changed\n").unwrap();
        std::fs::write(right.join("extra.txt"), "extra\n").unwrap();
        let diff = compare_paths(left.to_str().unwrap(), right.to_str().unwrap()).unwrap();
        assert!(diff.contains("+changed") && diff.contains("extra.txt"));
        assert!(compare_paths(
            left.to_str().unwrap(),
            right.join("extra.txt").to_str().unwrap()
        )
        .unwrap_err()
        .contains("two folders"));
    }

    #[test]
    fn reads_image_versions_from_history_index_and_disk() {
        let dir = repo();
        let path = dir.path().to_str().unwrap();
        commit(path, "logo.png", b"\x89PNG old", "Logo");
        std::fs::write(dir.path().join("logo.png"), b"\x89PNG new!").unwrap();
        let old = git_image_version(path.into(), "HEAD".into(), "logo.png".into()).unwrap();
        let new = git_image_version(path.into(), String::new(), "logo.png".into()).unwrap();
        assert_eq!((old.size, new.size), (8, 9));
        assert!(old.data_url.unwrap().starts_with("data:image/png;base64,"));
        let staged = git_image_version(path.into(), ":".into(), "missing.png".into()).unwrap();
        assert_eq!(staged.data_url, None);
        assert!(git_image_version(path.into(), "HEAD".into(), "a.txt".into()).is_err());
    }

    #[test]
    fn switches_history_order_between_branches_and_dates() {
        let dir = repo();
        let path = dir.path().to_str().unwrap();
        commit(path, "a.txt", b"a\n", "Base");
        assert_eq!(git_graph_order(path.into()).unwrap(), "topo");
        git_graph_order_set(path.into(), "topo".into()).unwrap();
        git_graph_order_set(path.into(), "date".into()).unwrap();
        assert_eq!(git_graph_order(path.into()).unwrap(), "date");
        assert_eq!(crate::git::commit_log(path, 0).unwrap().len(), 1);
        git_graph_order_set(path.into(), "topo".into()).unwrap();
        assert_eq!(git_graph_order(path.into()).unwrap(), "topo");
        assert!(git_graph_order_set(path.into(), "random".into()).is_err());
    }

    #[test]
    fn branches_from_a_commit_checks_it_out_and_merges_with_a_commit() {
        use crate::git::git_operation;
        let dir = repo();
        let path = dir.path().to_str().unwrap();
        commit(path, "a.txt", b"a\n", "First");
        let first = run(path, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        commit(path, "a.txt", b"b\n", "Second");
        git_operation(
            path.into(),
            "branch_at".into(),
            Some(format!("old {first}")),
            false,
        )
        .unwrap();
        assert_eq!(run(path, &["rev-parse", "old"]).unwrap().trim(), first);
        assert!(git_operation(
            path.into(),
            "branch_at".into(),
            Some("lonely".into()),
            false
        )
        .is_err());

        run(path, &["switch", "-q", "-c", "topic"]).unwrap();
        commit(path, "t.txt", b"t\n", "Topic");
        run(path, &["switch", "-q", "main"]).unwrap();
        git_operation(
            path.into(),
            "merge_no_ff".into(),
            Some("topic".into()),
            false,
        )
        .unwrap();
        assert_eq!(
            git_commit_info(path.into(), "HEAD".into())
                .unwrap()
                .parents
                .len(),
            2
        );

        git_operation(
            path.into(),
            "checkout_detached".into(),
            Some(first.clone()),
            false,
        )
        .unwrap();
        assert_eq!(run(path, &["branch", "--show-current"]).unwrap().trim(), "");
    }

    #[test]
    fn deletes_several_branches_and_explains_the_ones_kept() {
        let dir = repo();
        let path = dir.path().to_str().unwrap();
        commit(path, "a.txt", b"a\n", "Base");
        run(path, &["branch", "merged"]).unwrap();
        run(path, &["switch", "-q", "-c", "unmerged"]).unwrap();
        commit(path, "b.txt", b"b\n", "Work");
        run(path, &["switch", "-q", "main"]).unwrap();
        let report = git_branches_delete(
            path.into(),
            vec!["merged".into(), "unmerged".into(), "main".into()],
            false,
        )
        .unwrap();
        assert_eq!(report.deleted, vec!["merged".to_string()]);
        assert_eq!(report.kept.len(), 2);
        assert!(report.kept[0].1.contains("not merged"));
        assert!(report.kept[1].1.contains("checked out"));
        let forced = git_branches_delete(path.into(), vec!["unmerged".into()], true).unwrap();
        assert_eq!(forced.deleted, vec!["unmerged".to_string()]);
    }
}
