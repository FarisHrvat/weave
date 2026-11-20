//! Undo and redo for Git actions, read from the HEAD reflog, several steps
//! deep. Undoing a commit keeps its changes staged; other actions move the
//! branch back with `reset --keep`, which refuses to discard local edits.

use crate::{git::run, git_recovery::operation_in_progress};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Kind {
    Checkout,
    Commit,
    Move,
    BranchDeleted,
}

#[derive(Debug, Serialize, Deserialize)]
struct RedoState {
    kind: Kind,
    label: String,
    /// HEAD commit to return to, or the branch name for a checkout.
    target: String,
}

/// Where undo stands. Each undo adds its own reflog entry and steps past the
/// undone one, so the next action to undo is two entries further back. A
/// redo adds an entry that shifts the redone action back into that slot.
#[derive(Debug, Default, Serialize, Deserialize)]
struct History {
    /// HEAD when this history was saved; any other HEAD means a new action.
    expected_head: String,
    /// Reflog index of the next action to undo.
    next: usize,
    redo: Vec<RedoState>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UndoInfo {
    pub undo: Option<String>,
    pub redo: Option<String>,
}

struct Undoable {
    kind: Kind,
    label: String,
    previous: String,
}

fn head(path: &str) -> Option<String> {
    run(path, &["rev-parse", "--verify", "-q", "HEAD"])
        .ok()
        .map(|value| value.trim().to_owned())
}

fn history_file(path: &str) -> Result<PathBuf, String> {
    let location = run(path, &["rev-parse", "--git-path", "weave-undo.json"])?;
    let location = PathBuf::from(location.trim());
    Ok(if location.is_absolute() {
        location
    } else {
        PathBuf::from(path).join(location)
    })
}

/// A branch deleted after the latest HEAD change is the most recent action.
fn deleted_branch_action(path: &str) -> Option<Undoable> {
    let latest = crate::git_extra::deleted_branches(path).pop()?;
    let head_time: i64 = run(path, &["reflog", "show", "-n", "1", "--format=%ct", "HEAD"])
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0);
    let exists = run(
        path,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{}", latest.name),
        ],
    )
    .is_ok();
    (latest.deleted_at >= head_time && !exists).then(|| Undoable {
        kind: Kind::BranchDeleted,
        label: format!("delete branch {}", latest.name),
        previous: latest.name,
    })
}

fn action_at(path: &str, index: usize) -> Result<Option<Undoable>, String> {
    if index == 0 {
        if let Some(action) = deleted_branch_action(path) {
            return Ok(Some(action));
        }
    }
    let Ok(output) = run(
        path,
        &[
            "reflog",
            "show",
            "-n",
            &(index + 2).to_string(),
            "--format=%H%x1f%gs",
            "HEAD",
        ],
    ) else {
        return Ok(None);
    };
    let mut lines = output
        .lines()
        .skip(index)
        .map(|line| line.split_once('\u{1f}'));
    let Some(Some((_, message))) = lines.next() else {
        return Ok(None);
    };
    let previous = match lines.next() {
        Some(Some((commit, _))) => commit.to_owned(),
        _ => return Ok(None),
    };
    let message = message.trim();
    let action = if let Some(rest) = message.strip_prefix("checkout: moving from ") {
        let (from, _) = rest.split_once(" to ").unwrap_or((rest, ""));
        Undoable {
            kind: Kind::Checkout,
            label: format!("switch back to {from}"),
            previous: from.to_owned(),
        }
    } else if message.starts_with("commit (initial)") {
        return Ok(None);
    } else if let Some(subject) = message
        .strip_prefix("commit: ")
        .or_else(|| message.strip_prefix("commit (amend): "))
    {
        Undoable {
            kind: Kind::Commit,
            label: format!("commit “{subject}”"),
            previous,
        }
    } else {
        let label = match message.split_once(':') {
            Some((verb, _)) => verb.trim().to_owned(),
            None => message.to_owned(),
        };
        Undoable {
            kind: Kind::Move,
            label,
            previous,
        }
    };
    Ok(Some(action))
}

/// The saved history, or a fresh one if something changed HEAD since.
fn read_history(path: &str) -> History {
    let saved: Option<History> = history_file(path)
        .ok()
        .and_then(|file| fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str(&text).ok());
    match saved {
        Some(history) if head(path).as_deref() == Some(history.expected_head.as_str()) => history,
        _ => History::default(),
    }
}

fn write_history(path: &str, mut history: History) -> Result<(), String> {
    history.expected_head = head(path).unwrap_or_default();
    let json = serde_json::to_string(&history).map_err(|error| error.to_string())?;
    fs::write(history_file(path)?, json)
        .map_err(|error| format!("Could not save the undo history: {error}"))
}

fn ensure_ready(path: &str) -> Result<(), String> {
    // Files rewritten with identical content can look modified until the
    // index is refreshed, which would make reset --keep refuse needlessly.
    let _ = run(path, &["update-index", "-q", "--refresh"]);
    if operation_in_progress(path)? {
        return Err("Finish or abort the merge, rebase or cherry-pick in progress first.".into());
    }
    Ok(())
}

#[tauri::command]
pub fn git_undo_info(path: String) -> Result<UndoInfo, String> {
    run(&path, &["rev-parse", "--show-toplevel"])?;
    let history = read_history(&path);
    Ok(UndoInfo {
        undo: action_at(&path, history.next)?.map(|action| action.label),
        redo: history.redo.last().map(|state| state.label.clone()),
    })
}

#[tauri::command]
pub fn git_undo(path: String) -> Result<String, String> {
    ensure_ready(&path)?;
    let mut history = read_history(&path);
    let action = action_at(&path, history.next)?.ok_or(if history.next == 0 {
        "There is no Git action to undo."
    } else {
        "There is nothing older to undo."
    })?;
    let before = head(&path).ok_or("This repository has no commits yet.")?;
    let branch = run(&path, &["symbolic-ref", "-q", "--short", "HEAD"])
        .map(|name| name.trim().to_owned())
        .ok();
    let redo_target = match action.kind {
        Kind::Checkout => {
            run(&path, &["checkout", &action.previous]).map_err(|error| {
                format!("Could not switch back to {}: {error}", action.previous)
            })?;
            branch.unwrap_or(before)
        }
        Kind::Commit => {
            run(&path, &["reset", "--soft", &action.previous])?;
            before
        }
        Kind::Move => {
            run(&path, &["reset", "--keep", &action.previous]).map_err(|_| {
                "Undo would overwrite files you have changed. Commit or stash them first."
                    .to_owned()
            })?;
            before
        }
        Kind::BranchDeleted => {
            crate::git_extra::git_branch_restore(path.clone(), action.previous.clone())?;
            action.previous.clone()
        }
    };
    if action.kind != Kind::BranchDeleted {
        history.next += 2;
    }
    history.redo.push(RedoState {
        kind: action.kind,
        label: action.label.clone(),
        target: redo_target,
    });
    write_history(&path, history).map_err(|error| format!("Undo worked, but {error}"))?;
    Ok(format!("Undid {}.", action.label))
}

#[tauri::command]
pub fn git_redo(path: String) -> Result<String, String> {
    ensure_ready(&path)?;
    let mut history = read_history(&path);
    let state = history.redo.pop().ok_or("There is nothing to redo.")?;
    match state.kind {
        Kind::Checkout => run(&path, &["checkout", &state.target])?,
        Kind::Commit => run(&path, &["reset", "--soft", &state.target])?,
        Kind::Move => run(&path, &["reset", "--keep", &state.target]).map_err(|_| {
            "Redo would overwrite files you have changed. Commit or stash them first.".to_owned()
        })?,
        Kind::BranchDeleted => {
            crate::git_extra::remember_deleted_branch(&path, &state.target)?;
            run(&path, &["branch", "-D", &state.target])?
        }
    };
    write_history(&path, history)?;
    Ok(format!("Redid {}.", state.label))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, String) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_string_lossy().to_string();
        run(&path, &["init", "-b", "main"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        (root, path)
    }

    fn commit(root: &tempfile::TempDir, path: &str, file: &str, subject: &str) {
        fs::write(root.path().join(file), subject).unwrap();
        run(path, &["add", file]).unwrap();
        run(path, &["commit", "-m", subject]).unwrap();
    }

    #[test]
    fn undoes_and_redoes_a_commit_keeping_its_changes() {
        let (root, path) = repo();
        commit(&root, &path, "a.txt", "First");
        assert_eq!(git_undo_info(path.clone()).unwrap().undo, None);
        commit(&root, &path, "b.txt", "Second");
        let second = head(&path).unwrap();
        assert_eq!(
            git_undo_info(path.clone()).unwrap(),
            UndoInfo {
                undo: Some("commit “Second”".into()),
                redo: None
            }
        );
        assert_eq!(git_undo(path.clone()).unwrap(), "Undid commit “Second”.");
        assert_eq!(
            run(&path, &["diff", "--cached", "--name-only"])
                .unwrap()
                .trim(),
            "b.txt"
        );
        assert_eq!(
            git_undo_info(path.clone()).unwrap().redo.as_deref(),
            Some("commit “Second”")
        );
        git_redo(path.clone()).unwrap();
        assert_eq!(head(&path).unwrap(), second);
        assert!(run(&path, &["status", "--porcelain"]).unwrap().is_empty());
        assert_eq!(git_undo_info(path).unwrap().redo, None);
    }

    #[test]
    fn undoes_checkout_and_merge() {
        let (root, path) = repo();
        commit(&root, &path, "a.txt", "First");
        run(&path, &["checkout", "-b", "feature"]).unwrap();
        commit(&root, &path, "f.txt", "Feature");
        run(&path, &["checkout", "main"]).unwrap();
        assert_eq!(
            git_undo_info(path.clone()).unwrap().undo.as_deref(),
            Some("switch back to feature")
        );
        git_undo(path.clone()).unwrap();
        assert_eq!(
            run(&path, &["branch", "--show-current"]).unwrap().trim(),
            "feature"
        );
        git_redo(path.clone()).unwrap();
        assert_eq!(
            run(&path, &["branch", "--show-current"]).unwrap().trim(),
            "main"
        );

        let before = head(&path).unwrap();
        run(&path, &["merge", "--ff-only", "feature"]).unwrap();
        git_undo(path.clone()).unwrap();
        assert_eq!(head(&path).unwrap(), before);
        assert!(!root.path().join("f.txt").exists());
    }

    #[test]
    fn refuses_to_discard_local_edits_and_drops_stale_redo() {
        let (root, path) = repo();
        commit(&root, &path, "a.txt", "First");
        commit(&root, &path, "a.txt", "Second");
        run(&path, &["reset", "--hard", "HEAD~1"]).unwrap();
        fs::write(root.path().join("a.txt"), "local edit").unwrap();
        assert!(git_undo(path.clone())
            .unwrap_err()
            .contains("Commit or stash"));
        fs::write(root.path().join("a.txt"), "First").unwrap();
        git_undo(path.clone()).unwrap();
        commit(&root, &path, "c.txt", "Third");
        assert_eq!(git_undo_info(path).unwrap().redo, None);
    }

    #[test]
    fn undoes_and_redoes_a_branch_deletion() {
        let (root, path) = repo();
        commit(&root, &path, "a.txt", "First");
        run(&path, &["branch", "topic"]).unwrap();
        crate::git::git_operation(
            path.clone(),
            "branch_delete".into(),
            Some("topic".into()),
            true,
        )
        .unwrap();
        assert_eq!(
            git_undo_info(path.clone()).unwrap().undo.as_deref(),
            Some("delete branch topic")
        );
        git_undo(path.clone()).unwrap();
        assert!(run(&path, &["rev-parse", "--verify", "topic"]).is_ok());
        git_redo(path.clone()).unwrap();
        assert!(run(&path, &["rev-parse", "--verify", "-q", "refs/heads/topic"]).is_err());
    }

    #[test]
    fn undoes_several_steps_and_redoes_them_in_order() {
        let (root, path) = repo();
        commit(&root, &path, "a.txt", "First");
        let first = head(&path).unwrap();
        commit(&root, &path, "b.txt", "Second");
        let second = head(&path).unwrap();
        commit(&root, &path, "c.txt", "Third");
        let third = head(&path).unwrap();
        assert_eq!(git_undo(path.clone()).unwrap(), "Undid commit “Third”.");
        assert_eq!(
            git_undo_info(path.clone()).unwrap().undo.as_deref(),
            Some("commit “Second”")
        );
        assert_eq!(git_undo(path.clone()).unwrap(), "Undid commit “Second”.");
        assert_eq!(head(&path).unwrap(), first);
        assert!(git_undo(path.clone())
            .unwrap_err()
            .contains("nothing older"));
        assert_eq!(
            git_undo_info(path.clone()).unwrap().redo.as_deref(),
            Some("commit “Second”")
        );
        git_redo(path.clone()).unwrap();
        assert_eq!(head(&path).unwrap(), second);
        assert_eq!(
            git_undo_info(path.clone()).unwrap().undo.as_deref(),
            Some("commit “Second”")
        );
        git_redo(path.clone()).unwrap();
        assert_eq!(head(&path).unwrap(), third);
        assert_eq!(git_undo_info(path.clone()).unwrap().redo, None);
        // Undo again after redoing walks back through the same commits.
        git_undo(path.clone()).unwrap();
        git_undo(path.clone()).unwrap();
        assert_eq!(head(&path).unwrap(), first);
        // A new commit starts a fresh history.
        run(&path, &["commit", "-m", "Fresh"]).unwrap();
        let info = git_undo_info(path).unwrap();
        assert_eq!(
            (info.undo.as_deref(), info.redo),
            (Some("commit “Fresh”"), None)
        );
    }
}
