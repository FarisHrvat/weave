use crate::git::run;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebaseStep {
    pub id: String,
    pub subject: String,
    pub action: String,
    pub message: String,
}

#[tauri::command]
pub fn git_rebase_plan(path: String, base: String) -> Result<Vec<RebaseStep>, String> {
    let base = base.trim();
    if base.is_empty() || base.starts_with('-') || base.contains(['\n', '\r', '\0']) {
        return Err("Enter a valid base commit or branch for interactive rebase.".into());
    }
    run(&path, &["rev-parse", "--show-toplevel"])?;
    let base_commit = format!("{base}^{{commit}}");
    run(&path, &["rev-parse", "--verify", &base_commit])?;
    run(&path, &["merge-base", "--is-ancestor", base, "HEAD"]).map_err(|_| {
        "Choose a base commit that is an ancestor of the current branch.".to_owned()
    })?;
    let range = format!("{base}..HEAD");
    if !run(&path, &["rev-list", "--merges", &range])?.is_empty() {
        return Err("This branch includes merge commits. Use Rebase keeping merges to move it onto the new base without flattening them.".into());
    }
    let output = run(
        &path,
        &["log", "--reverse", "--format=%H%x00%s%x00%B%x00", &range],
    )?;
    let mut parts = output.split('\0');
    let mut steps = Vec::new();
    while let Some(id) = parts.next() {
        let id = id.trim();
        if id.is_empty() {
            break;
        }
        let subject = parts.next().unwrap_or("").trim().to_owned();
        let message = parts.next().unwrap_or("").trim_end_matches('\n').to_owned();
        steps.push(RebaseStep {
            id: id.to_owned(),
            subject,
            action: "pick".into(),
            message,
        });
    }
    if steps.is_empty() {
        return Err("There are no commits after this base to rebase.".into());
    }
    Ok(steps)
}

#[tauri::command]
pub async fn git_rebase_apply(
    path: String,
    base: String,
    steps: Vec<RebaseStep>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || apply_rebase(path, base, steps))
        .await
        .map_err(|error| format!("Interactive rebase stopped unexpectedly: {error}"))?
}

fn apply_rebase(path: String, base: String, steps: Vec<RebaseStep>) -> Result<String, String> {
    if !run(&path, &["status", "--porcelain"])?.is_empty() {
        return Err("Commit or stash local changes before starting an interactive rebase.".into());
    }
    let original = git_rebase_plan(path.clone(), base.clone())?;
    if steps.len() != original.len() {
        return Err("The rebase plan changed. Reload it before applying.".into());
    }
    let mut expected: Vec<&str> = original.iter().map(|step| step.id.as_str()).collect();
    let mut received: Vec<&str> = steps.iter().map(|step| step.id.as_str()).collect();
    expected.sort_unstable();
    received.sort_unstable();
    if expected != received {
        return Err("The rebase plan must contain each original commit exactly once.".into());
    }
    if steps
        .first()
        .is_some_and(|step| step.action == "squash" || step.action == "fixup")
    {
        return Err("The first commit cannot be squashed or fixed up.".into());
    }
    if steps.iter().any(|step| {
        !matches!(
            step.action.as_str(),
            "pick" | "reword" | "squash" | "fixup" | "drop"
        )
    }) {
        return Err("Choose pick, reword, squash, fixup, or drop for each commit.".into());
    }
    if steps.iter().any(|step| {
        step.action == "reword"
            && (step.message.trim().is_empty()
                || step.message.len() > 20_000
                || step.message.contains('\0'))
    }) {
        return Err(
            "Reworded messages must contain text, be under 20 KB, and contain no null bytes."
                .into(),
        );
    }
    if steps.iter().all(|step| step.action == "drop") {
        return Err("Keep at least one commit in the rebase plan.".into());
    }
    let mut has_previous = false;
    for step in &steps {
        if matches!(step.action.as_str(), "squash" | "fixup") && !has_previous {
            return Err("A squash or fixup needs a preceding commit to combine with.".into());
        }
        if step.action != "drop" {
            has_previous = true;
        }
    }
    let temporary = std::env::temp_dir().join(format!("weave-rebase-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&temporary)
        .map_err(|error| format!("Could not prepare rebase plan: {error}"))?;
    let todo = temporary.join("todo");
    let editor = temporary.join("editor.sh");
    let contents = steps
        .iter()
        .map(|step| {
            if step.action == "reword" {
                let encoded = STANDARD.encode(step.message.as_bytes());
                format!(
                    "pick {}\nexec printf '%s' '{}' | base64 --decode | git commit --amend -F -\n",
                    step.id, encoded
                )
            } else {
                format!("{} {}\n", step.action, step.id)
            }
        })
        .collect::<String>();
    let result = (|| {
        fs::write(&todo, contents)
            .map_err(|error| format!("Could not write rebase plan: {error}"))?;
        fs::write(&editor, "#!/bin/sh\ncat \"$WEAVE_REBASE_TODO\" > \"$1\"\n")
            .map_err(|error| format!("Could not prepare Git's sequence editor: {error}"))?;
        crate::platform::make_executable(&editor)
            .map_err(|error| format!("Could not start Git's sequence editor: {error}"))?;
        let output = crate::platform::command("git")
            .arg("-C")
            .arg(&path)
            .args(["rebase", "-i", base.trim()])
            .env("GIT_SEQUENCE_EDITOR", crate::platform::shell_path(&editor))
            .env("WEAVE_REBASE_TODO", &todo)
            .env("GIT_EDITOR", "true")
            .output()
            .map_err(|error| format!("Git could not start interactive rebase: {error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            let detail = String::from_utf8_lossy(&output.stderr);
            Err(format!("Interactive rebase stopped: {}. Resolve conflicts, then use Continue rebase or Abort rebase.", detail.trim()))
        }
    })();
    let _ = fs::remove_dir_all(&temporary);
    result
}

/// Rebases the current branch onto `onto` while recreating its merge
/// commits (`git rebase --rebase-merges`). Conflicts stop the rebase so they
/// can be resolved with the existing continue and abort controls.
#[tauri::command]
pub async fn git_rebase_merges(path: String, onto: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || rebase_keeping_merges(&path, &onto))
        .await
        .map_err(|error| format!("Rebase stopped unexpectedly: {error}"))?
}

fn rebase_keeping_merges(path: &str, onto: &str) -> Result<String, String> {
    let onto = onto.trim();
    if onto.is_empty() || onto.starts_with('-') || onto.contains(['\n', '\r', '\0']) {
        return Err("Enter a valid branch or commit to rebase onto.".into());
    }
    run(path, &["rev-parse", "--show-toplevel"])?;
    run(
        path,
        &["rev-parse", "--verify", &format!("{onto}^{{commit}}")],
    )?;
    if run(path, &["symbolic-ref", "-q", "HEAD"]).is_err() {
        return Err("Check out a branch before rebasing.".into());
    }
    if !run(path, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err("Commit or stash local changes before rebasing.".into());
    }
    let output = crate::platform::command("git")
        .arg("-C")
        .arg(path)
        .args(["rebase", "--rebase-merges", onto])
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .output()
        .map_err(|error| format!("Git could not start the rebase: {error}"))?;
    if output.status.success() {
        let merges = run(
            path,
            &["rev-list", "--count", "--merges", &format!("{onto}..HEAD")],
        )?;
        Ok(format!(
            "Rebased onto {onto}, keeping {} merge commit{}.",
            merges.trim(),
            if merges.trim() == "1" { "" } else { "s" }
        ))
    } else {
        Err(format!(
            "Rebase stopped: {}. Resolve conflicts, then use Continue rebase or Abort rebase.",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Runs `git rebase -i --rebase-merges` with a sequence editor script.
/// `todo` replaces Git's plan; with `None` the plan is captured and the
/// rebase is cancelled before anything changes.
fn rebase_with_todo(path: &str, onto: &str, todo: Option<&str>) -> Result<String, String> {
    let onto = onto.trim();
    if onto.is_empty() || onto.starts_with('-') || onto.contains(['\n', '\r', '\0']) {
        return Err("Enter a valid branch or commit to rebase onto.".into());
    }
    run(
        path,
        &["rev-parse", "--verify", &format!("{onto}^{{commit}}")],
    )?;
    if run(path, &["symbolic-ref", "-q", "HEAD"]).is_err() {
        return Err("Check out a branch before rebasing.".into());
    }
    if !run(path, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err("Commit or stash local changes before rebasing.".into());
    }
    let temporary =
        tempfile::tempdir().map_err(|error| format!("Could not prepare the rebase: {error}"))?;
    let plan = temporary.path().join("plan");
    let editor = temporary.path().join("editor.sh");
    let script = match todo {
        Some(text) => {
            fs::write(&plan, text).map_err(|error| format!("Could not write the plan: {error}"))?;
            "#!/bin/sh\ncat \"$WEAVE_REBASE_PLAN\" > \"$1\"\n"
        }
        None => "#!/bin/sh\ncat \"$1\" > \"$WEAVE_REBASE_PLAN\"\nexit 1\n",
    };
    fs::write(&editor, script)
        .and_then(|()| crate::platform::make_executable(&editor))
        .map_err(|error| format!("Could not prepare Git's sequence editor: {error}"))?;
    let output = crate::platform::command("git")
        .arg("-C")
        .arg(path)
        .args(["rebase", "-i", "--rebase-merges", onto])
        .env("GIT_SEQUENCE_EDITOR", crate::platform::shell_path(&editor))
        .env("WEAVE_REBASE_PLAN", &plan)
        .env("GIT_EDITOR", "true")
        .output()
        .map_err(|error| format!("Git could not start the rebase: {error}"))?;
    match todo {
        None => {
            let text =
                fs::read_to_string(&plan).map_err(|_| "Git did not produce a rebase plan.")?;
            Ok(text
                .lines()
                .filter(|line| !line.starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_owned())
        }
        Some(_) if output.status.success() => {
            Ok(format!("Rebased onto {onto} with your edited plan."))
        }
        Some(_) => Err(format!(
            "Rebase stopped: {}. Resolve conflicts, then use Continue rebase or Abort rebase.",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// Git's full plan for a merge-preserving rebase, including label, reset and
/// merge steps, for editing before it runs.
#[tauri::command]
pub fn git_rebase_merges_plan(path: String, onto: String) -> Result<String, String> {
    let plan = rebase_with_todo(&path, &onto, None)?;
    if plan.is_empty() || plan == "noop" {
        return Err("There are no commits to rebase onto that base.".into());
    }
    Ok(plan)
}

#[tauri::command]
pub async fn git_rebase_merges_apply(
    path: String,
    onto: String,
    plan: String,
) -> Result<String, String> {
    if plan.trim().is_empty() || plan.len() > 200_000 || plan.contains('\0') {
        return Err("The plan is empty or too long.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        rebase_with_todo(&path, &onto, Some(&format!("{}\n", plan.trim())))
    })
    .await
    .map_err(|error| format!("Rebase stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_and_applies_a_merge_preserving_plan() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_string_lossy().to_string();
        let commit = |file: &str, subject: &str| {
            fs::write(root.path().join(file), subject).unwrap();
            run(&path, &["add", file]).unwrap();
            run(&path, &["commit", "-m", subject]).unwrap();
        };
        run(&path, &["init", "-b", "main"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        commit("base.txt", "Base");
        run(&path, &["checkout", "-b", "feature"]).unwrap();
        commit("f1.txt", "Feature one");
        run(&path, &["checkout", "-b", "topic"]).unwrap();
        commit("t.txt", "Topic");
        run(&path, &["checkout", "feature"]).unwrap();
        commit("f2.txt", "Feature two");
        run(&path, &["merge", "--no-ff", "-m", "Merge topic", "topic"]).unwrap();
        run(&path, &["checkout", "main"]).unwrap();
        commit("main.txt", "Main moves on");
        run(&path, &["checkout", "feature"]).unwrap();
        let before = run(&path, &["rev-parse", "HEAD"]).unwrap();

        let plan = git_rebase_merges_plan(path.clone(), "main".into()).unwrap();
        assert!(plan.contains("label onto"));
        assert!(plan.contains("merge -C"));
        assert_eq!(run(&path, &["rev-parse", "HEAD"]).unwrap(), before);
        assert!(run(&path, &["status", "--porcelain"]).unwrap().is_empty());

        // Drop "Feature two" from the plan, keeping the merge.
        let edited: String = plan
            .lines()
            .filter(|line| !line.ends_with("Feature two"))
            .collect::<Vec<_>>()
            .join("\n");
        tauri::async_runtime::block_on(git_rebase_merges_apply(
            path.clone(),
            "main".into(),
            edited,
        ))
        .unwrap();
        assert!(!root.path().join("f2.txt").exists());
        assert!(root.path().join("t.txt").exists());
        assert_eq!(
            run(&path, &["log", "-1", "--format=%s"]).unwrap().trim(),
            "Merge topic"
        );
    }

    #[test]
    fn rebases_a_branch_and_keeps_its_merge_commits() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_string_lossy().to_string();
        let commit = |file: &str, subject: &str| {
            fs::write(root.path().join(file), subject).unwrap();
            run(&path, &["add", file]).unwrap();
            run(&path, &["commit", "-m", subject]).unwrap();
        };
        run(&path, &["init", "-b", "main"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        commit("base.txt", "Base");
        run(&path, &["checkout", "-b", "feature"]).unwrap();
        commit("f1.txt", "Feature one");
        run(&path, &["checkout", "-b", "topic"]).unwrap();
        commit("t.txt", "Topic");
        run(&path, &["checkout", "feature"]).unwrap();
        commit("f2.txt", "Feature two");
        run(&path, &["merge", "--no-ff", "-m", "Merge topic", "topic"]).unwrap();
        run(&path, &["checkout", "main"]).unwrap();
        commit("main.txt", "Main moves on");
        run(&path, &["checkout", "feature"]).unwrap();

        let fork = run(&path, &["merge-base", "main", "HEAD"]).unwrap();
        assert!(git_rebase_plan(path.clone(), fork.trim().into())
            .unwrap_err()
            .contains("Rebase keeping merges"));
        assert!(rebase_keeping_merges(&path, "-x").is_err());
        let message = rebase_keeping_merges(&path, "main").unwrap();
        assert!(message.contains("1 merge commit"));
        run(&path, &["merge-base", "--is-ancestor", "main", "HEAD"]).unwrap();
        assert_eq!(
            run(&path, &["log", "-1", "--format=%s"]).unwrap().trim(),
            "Merge topic"
        );
        assert!(root.path().join("main.txt").exists());
        assert!(root.path().join("t.txt").exists());
    }

    #[test]
    fn reorders_and_squashes_commits_in_a_real_repository() {
        let root = std::env::temp_dir().join(format!("weave-rebase-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        for (file, subject) in [
            ("base.txt", "Base"),
            ("a.txt", "A"),
            ("b.txt", "B"),
            ("c.txt", "C"),
        ] {
            fs::write(root.join(file), subject).unwrap();
            run(&path, &["add", file]).unwrap();
            run(&path, &["commit", "-m", subject]).unwrap();
        }
        let base = "HEAD~3".to_owned();
        let mut steps = git_rebase_plan(path.clone(), base.clone()).unwrap();
        assert_eq!(
            steps
                .iter()
                .map(|step| step.subject.as_str())
                .collect::<Vec<_>>(),
            ["A", "B", "C"]
        );
        steps.swap(0, 1);
        steps[2].action = "fixup".into();
        apply_rebase(path.clone(), base, steps).unwrap();
        assert_eq!(
            run(&path, &["log", "-2", "--format=%s"])
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        assert!(root.join("c.txt").exists());

        let base = "HEAD~2".to_owned();
        let mut steps = git_rebase_plan(path.clone(), base.clone()).unwrap();
        steps[1].action = "squash".into();
        apply_rebase(path.clone(), base, steps).unwrap();
        let message = run(&path, &["log", "-1", "--format=%B"]).unwrap();
        assert!(message.contains('A'));
        assert!(message.contains('B'));

        for (file, subject) in [("d.txt", "D"), ("e.txt", "E")] {
            fs::write(root.join(file), subject).unwrap();
            run(&path, &["add", file]).unwrap();
            run(&path, &["commit", "-m", subject]).unwrap();
        }
        let base = "HEAD~2".to_owned();
        let mut steps = git_rebase_plan(path.clone(), base.clone()).unwrap();
        steps[1].action = "drop".into();
        apply_rebase(path.clone(), base, steps).unwrap();
        assert!(!root.join("e.txt").exists());

        fs::write(root.join("reword.txt"), "content").unwrap();
        run(&path, &["add", "reword.txt"]).unwrap();
        run(&path, &["commit", "-m", "Old message"]).unwrap();
        let base = "HEAD~1".to_owned();
        let mut steps = git_rebase_plan(path.clone(), base.clone()).unwrap();
        assert_eq!(steps[0].message, "Old message");
        steps[0].action = "reword".into();
        steps[0].message = "".into();
        assert!(apply_rebase(path.clone(), base.clone(), steps.clone()).is_err());
        let marker = root.join("shell-injection");
        steps[0].message = format!(
            "Better title\n\nLiteral $(touch {}) and café",
            marker.display()
        );
        apply_rebase(path.clone(), base, steps.clone()).unwrap();
        assert_eq!(
            run(&path, &["log", "-1", "--format=%B"])
                .unwrap()
                .trim_end(),
            steps[0].message
        );
        assert!(!marker.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
