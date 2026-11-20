use crate::{git::run, git_recovery::operation_in_progress};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFlowSettings {
    main_branch: String,
    develop_branch: String,
    feature_prefix: String,
    release_prefix: String,
    hotfix_prefix: String,
    tag_prefix: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFlowInfo {
    pub initialized: bool,
    pub main_branch: String,
    pub develop_branch: String,
    pub feature_prefix: String,
    pub release_prefix: String,
    pub hotfix_prefix: String,
    pub tag_prefix: String,
    pub current_branch: String,
    pub publish_tags: Vec<String>,
}

fn repository_root(path: &str) -> Result<String, String> {
    let root = run(path, &["rev-parse", "--show-toplevel"])?;
    Ok(root.trim().to_owned())
}

fn config(path: &str, key: &str) -> Option<String> {
    run(path, &["config", "--local", "--get", key])
        .ok()
        .map(|value| value.trim().to_owned())
}

fn branch_exists(path: &str, name: &str) -> bool {
    run(
        path,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .is_ok()
}

fn check_branch_name(path: &str, name: &str) -> Result<(), String> {
    if name.is_empty() || name.starts_with('-') || name.contains(['\n', '\r', '\0']) {
        return Err("Enter a valid branch name without a leading dash or line break.".into());
    }
    run(path, &["check-ref-format", "--branch", name])
        .map(|_| ())
        .map_err(|_| format!("{name} is not a valid Git branch name."))
}

fn check_prefix(path: &str, prefix: &str) -> Result<(), String> {
    if !prefix.ends_with('/') {
        return Err("Each Gitflow branch prefix must end with a slash.".into());
    }
    check_branch_name(path, &format!("{prefix}example"))
}

fn check_ready(path: &str) -> Result<(), String> {
    if !run(path, &["status", "--porcelain"])?.is_empty() {
        return Err("Commit or stash local changes before a Gitflow action.".into());
    }
    if operation_in_progress(path)? {
        return Err("Finish or abort the current Git operation before a Gitflow action.".into());
    }
    Ok(())
}

#[tauri::command]
pub fn git_flow_info(path: String) -> Result<GitFlowInfo, String> {
    let path = repository_root(&path)?;
    let main_branch = config(&path, "gitflow.branch.master");
    let develop_branch = config(&path, "gitflow.branch.develop");
    let main_name = main_branch.clone().unwrap_or_else(|| "main".into());
    let tag_prefix = config(&path, "gitflow.prefix.versiontag").unwrap_or_default();
    let publish_tags = if branch_exists(&path, &main_name) {
        run(&path, &["tag", "--merged", &main_name])?
            .lines()
            .filter(|tag| tag.starts_with(&tag_prefix))
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    Ok(GitFlowInfo {
        initialized: main_branch.is_some() && develop_branch.is_some(),
        main_branch: main_name,
        develop_branch: develop_branch.unwrap_or_else(|| "develop".into()),
        feature_prefix: config(&path, "gitflow.prefix.feature")
            .unwrap_or_else(|| "feature/".into()),
        release_prefix: config(&path, "gitflow.prefix.release")
            .unwrap_or_else(|| "release/".into()),
        hotfix_prefix: config(&path, "gitflow.prefix.hotfix").unwrap_or_else(|| "hotfix/".into()),
        tag_prefix,
        current_branch: run(&path, &["branch", "--show-current"])?.trim().to_owned(),
        publish_tags,
    })
}

#[tauri::command]
pub async fn git_flow_setup(
    path: String,
    settings: GitFlowSettings,
    update: bool,
) -> Result<GitFlowInfo, String> {
    tauri::async_runtime::spawn_blocking(move || setup(&path, &settings, update))
        .await
        .map_err(|error| format!("Gitflow setup stopped unexpectedly: {error}"))?
}

fn setup(path: &str, settings: &GitFlowSettings, update: bool) -> Result<GitFlowInfo, String> {
    let path = repository_root(path)?;
    let GitFlowSettings {
        main_branch,
        develop_branch,
        feature_prefix,
        release_prefix,
        hotfix_prefix,
        tag_prefix,
    } = settings;
    check_ready(&path)?;
    if run(&path, &["rev-parse", "--verify", "HEAD"]).is_err() {
        return Err("Create the first commit before setting up Gitflow.".into());
    }
    check_branch_name(&path, main_branch)?;
    check_branch_name(&path, develop_branch)?;
    if main_branch == develop_branch {
        return Err("Production and development branches must have different names.".into());
    }
    for prefix in [feature_prefix, release_prefix, hotfix_prefix] {
        check_prefix(&path, prefix)?;
    }
    if feature_prefix.starts_with(release_prefix)
        || release_prefix.starts_with(feature_prefix)
        || feature_prefix.starts_with(hotfix_prefix)
        || hotfix_prefix.starts_with(feature_prefix)
        || release_prefix.starts_with(hotfix_prefix)
        || hotfix_prefix.starts_with(release_prefix)
    {
        return Err("Feature, release, and hotfix prefixes must not overlap.".into());
    }
    if tag_prefix.starts_with('-')
        || tag_prefix.contains(['\n', '\r', '\0'])
        || run(
            &path,
            &[
                "check-ref-format",
                &format!("refs/tags/{tag_prefix}example"),
            ],
        )
        .is_err()
    {
        return Err("Enter a valid release tag prefix.".into());
    }
    let initialized = git_flow_info(path.clone())?.initialized;
    if initialized && !update {
        return Err("Gitflow is already configured for this repository.".into());
    }
    if !initialized && update {
        return Err("Set up Gitflow before changing its settings.".into());
    }
    if update
        && [main_branch, develop_branch]
            .iter()
            .any(|branch| !branch_exists(&path, branch))
    {
        return Err("Create both production and development branches before selecting them in Gitflow settings.".into());
    }
    if !branch_exists(&path, main_branch) {
        run(&path, &["branch", main_branch, "HEAD"])?;
    }
    if !branch_exists(&path, develop_branch) {
        run(&path, &["branch", develop_branch, main_branch])?;
    }
    for (key, value) in [
        ("gitflow.branch.master", main_branch),
        ("gitflow.branch.develop", develop_branch),
        ("gitflow.prefix.feature", feature_prefix),
        ("gitflow.prefix.release", release_prefix),
        ("gitflow.prefix.hotfix", hotfix_prefix),
        ("gitflow.prefix.versiontag", tag_prefix),
    ] {
        run(&path, &["config", "--local", key, value])?;
    }
    git_flow_info(path)
}

#[tauri::command]
pub async fn git_flow_start(
    path: String,
    kind: String,
    name: String,
) -> Result<GitFlowInfo, String> {
    tauri::async_runtime::spawn_blocking(move || start(&path, &kind, &name))
        .await
        .map_err(|error| format!("Gitflow branch creation stopped unexpectedly: {error}"))?
}

fn start(path: &str, kind: &str, name: &str) -> Result<GitFlowInfo, String> {
    let path = repository_root(path)?;
    check_ready(&path)?;
    let info = git_flow_info(path.clone())?;
    if !info.initialized {
        return Err("Set up Gitflow for this repository first.".into());
    }
    let (prefix, base) = match kind {
        "feature" => (&info.feature_prefix, &info.develop_branch),
        "release" => (&info.release_prefix, &info.develop_branch),
        "hotfix" => (&info.hotfix_prefix, &info.main_branch),
        _ => return Err("Choose a feature, release, or hotfix branch.".into()),
    };
    let name = name.trim();
    if name.is_empty() || name.starts_with('/') {
        return Err("Enter a name for the new Gitflow branch.".into());
    }
    let branch = format!("{prefix}{name}");
    check_branch_name(&path, &branch)?;
    if branch_exists(&path, &branch) {
        return Err(format!("The branch {branch} already exists."));
    }
    run(&path, &["switch", "-c", &branch, base])?;
    git_flow_info(path)
}

#[tauri::command]
pub async fn git_flow_finish(path: String) -> Result<GitFlowInfo, String> {
    tauri::async_runtime::spawn_blocking(move || finish(&path))
        .await
        .map_err(|error| format!("Gitflow finish stopped unexpectedly: {error}"))?
}

fn finish(path: &str) -> Result<GitFlowInfo, String> {
    let path = repository_root(path)?;
    check_ready(&path)?;
    let info = git_flow_info(path.clone())?;
    if !info.initialized {
        return Err("Set up Gitflow for this repository first.".into());
    }
    let branch = info.current_branch.as_str();
    let (kind, name) = if let Some(name) = branch.strip_prefix(&info.feature_prefix) {
        ("feature", name)
    } else if let Some(name) = branch.strip_prefix(&info.release_prefix) {
        ("release", name)
    } else if let Some(name) = branch.strip_prefix(&info.hotfix_prefix) {
        ("hotfix", name)
    } else {
        return Err("Check out a feature, release, or hotfix branch to finish it.".into());
    };
    if name.is_empty() {
        return Err("The current Gitflow branch has no name after its prefix.".into());
    }
    let tag = format!("{}{name}", info.tag_prefix);
    if kind != "feature"
        && run(
            &path,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/tags/{tag}"),
            ],
        )
        .is_ok()
    {
        return Err(format!(
            "The tag {tag} already exists. Rename or remove it first."
        ));
    }
    let merge = |target: &str| -> Result<(), String> {
        run(&path, &["switch", target])?;
        run(&path, &["merge", "--no-ff", "--no-edit", branch])
            .map(|_| ())
            .map_err(|error| format!("Could not merge {branch} into {target}: {error}. Resolve the conflict or abort the merge in Git."))
    };
    if kind == "feature" {
        merge(&info.develop_branch)?;
    } else {
        merge(&info.main_branch)?;
        let main_tip = run(&path, &["rev-parse", "HEAD"])?.trim().to_owned();
        merge(&info.develop_branch).map_err(|error| {
            format!("{error} The production branch was already updated; the source branch and tag were preserved.")
        })?;
        run(&path, &["tag", &tag, &main_tip]).map_err(|error| {
            format!("Both target branches were merged, but the tag {tag} could not be created: {error}. The source branch remains available.")
        })?;
    }
    run(&path, &["branch", "-d", branch])?;
    git_flow_info(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn settings() -> GitFlowSettings {
        GitFlowSettings {
            main_branch: "main".into(),
            develop_branch: "develop".into(),
            feature_prefix: "feature/".into(),
            release_prefix: "release/".into(),
            hotfix_prefix: "hotfix/".into(),
            tag_prefix: "v".into(),
        }
    }

    #[test]
    fn completes_feature_release_and_hotfix_in_a_real_repository() {
        let root = std::env::temp_dir().join(format!("weave-git-flow-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init", "-b", "main"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        fs::write(root.join("base.txt"), "base\n").unwrap();
        run(&path, &["add", "."]).unwrap();
        run(&path, &["commit", "-m", "Base"]).unwrap();
        let mut invalid = settings();
        invalid.develop_branch = "main".into();
        assert!(setup(&path, &invalid, false).is_err());
        let info = setup(&path, &settings(), false).unwrap();
        assert!(info.initialized);
        assert!(setup(&path, &settings(), false).is_err());
        let mut missing = settings();
        missing.develop_branch = "missing".into();
        assert!(setup(&path, &missing, true).is_err());
        setup(&path, &settings(), true).unwrap();

        start(&path, "feature", "search").unwrap();
        fs::write(root.join("feature.txt"), "feature\n").unwrap();
        run(&path, &["add", "."]).unwrap();
        run(&path, &["commit", "-m", "Feature"]).unwrap();
        assert!(finish(&path).unwrap().current_branch == "develop");
        assert!(!branch_exists(&path, "feature/search"));
        assert!(root.join("feature.txt").exists());

        start(&path, "release", "1.0").unwrap();
        fs::write(root.join("release.txt"), "release\n").unwrap();
        run(&path, &["add", "."]).unwrap();
        run(&path, &["commit", "-m", "Release"]).unwrap();
        finish(&path).unwrap();
        let tag = run(&path, &["rev-parse", "v1.0"]).unwrap();
        let main = run(&path, &["rev-parse", "main"]).unwrap();
        assert_eq!(tag, main);
        assert!(root.join("release.txt").exists());

        start(&path, "hotfix", "urgent").unwrap();
        fs::write(root.join("hotfix.txt"), "hotfix\n").unwrap();
        run(&path, &["add", "."]).unwrap();
        run(&path, &["commit", "-m", "Hotfix"]).unwrap();
        finish(&path).unwrap();
        assert_eq!(
            run(&path, &["rev-parse", "vurgent"]).unwrap(),
            run(&path, &["rev-parse", "main"]).unwrap()
        );
        assert!(root.join("hotfix.txt").exists());

        let remote =
            std::env::temp_dir().join(format!("weave-flow-remote-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&remote).unwrap();
        let remote_path = remote.to_string_lossy().to_string();
        run(&remote_path, &["init", "--bare"]).unwrap();
        run(&path, &["remote", "add", "origin", &remote_path]).unwrap();
        let publish = git_flow_info(path.clone()).unwrap();
        assert!(publish.publish_tags.contains(&"vurgent".into()));
        tauri::async_runtime::block_on(crate::git::git_flow_publish(path.clone())).unwrap();
        for reference in [
            "refs/heads/main",
            "refs/heads/develop",
            "refs/tags/v1.0",
            "refs/tags/vurgent",
        ] {
            assert_eq!(
                run(&remote_path, &["rev-parse", reference]).unwrap(),
                run(&path, &["rev-parse", reference]).unwrap()
            );
        }
        let published_develop = run(&remote_path, &["rev-parse", "develop"]).unwrap();
        run(
            &remote_path,
            &["update-ref", "refs/heads/main", published_develop.trim()],
        )
        .unwrap();
        fs::write(root.join("later.txt"), "later\n").unwrap();
        run(&path, &["add", "later.txt"]).unwrap();
        run(&path, &["commit", "-m", "Later development"]).unwrap();
        assert!(
            tauri::async_runtime::block_on(crate::git::git_flow_publish(path.clone())).is_err()
        );
        assert_eq!(
            run(&remote_path, &["rev-parse", "develop"]).unwrap(),
            published_develop
        );

        fs::write(root.join("pending.txt"), "pending").unwrap();
        assert!(start(&path, "feature", "blocked")
            .unwrap_err()
            .contains("Commit or stash"));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(remote).unwrap();
    }

    #[test]
    fn preserves_the_feature_branch_when_a_finish_merge_conflicts() {
        let root = std::env::temp_dir().join(format!("weave-git-flow-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init", "-b", "main"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        fs::write(root.join("shared.txt"), "base\n").unwrap();
        run(&path, &["add", "."]).unwrap();
        run(&path, &["commit", "-m", "Base"]).unwrap();
        setup(&path, &settings(), false).unwrap();
        start(&path, "feature", "conflict").unwrap();
        fs::write(root.join("shared.txt"), "feature\n").unwrap();
        run(&path, &["commit", "-am", "Feature change"]).unwrap();
        run(&path, &["switch", "develop"]).unwrap();
        fs::write(root.join("shared.txt"), "develop\n").unwrap();
        run(&path, &["commit", "-am", "Development change"]).unwrap();
        run(&path, &["switch", "feature/conflict"]).unwrap();
        let error = finish(&path).unwrap_err();
        assert!(error.contains("Resolve the conflict or abort the merge"));
        assert!(branch_exists(&path, "feature/conflict"));
        assert_eq!(
            run(&path, &["branch", "--show-current"]).unwrap().trim(),
            "develop"
        );
        assert!(operation_in_progress(&path).unwrap());
        run(&path, &["merge", "--abort"]).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explains_a_release_that_stops_after_updating_production() {
        let root = std::env::temp_dir().join(format!("weave-git-flow-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.to_string_lossy().to_string();
        run(&path, &["init", "-b", "main"]).unwrap();
        run(&path, &["config", "user.name", "Weave Test"]).unwrap();
        run(&path, &["config", "user.email", "weave@example.test"]).unwrap();
        fs::write(root.join("shared.txt"), "base\n").unwrap();
        run(&path, &["add", "."]).unwrap();
        run(&path, &["commit", "-m", "Base"]).unwrap();
        setup(&path, &settings(), false).unwrap();
        start(&path, "release", "2.0").unwrap();
        fs::write(root.join("shared.txt"), "release\n").unwrap();
        run(&path, &["commit", "-am", "Release change"]).unwrap();
        run(&path, &["switch", "develop"]).unwrap();
        fs::write(root.join("shared.txt"), "develop\n").unwrap();
        run(&path, &["commit", "-am", "Development change"]).unwrap();
        run(&path, &["switch", "release/2.0"]).unwrap();
        let error = finish(&path).unwrap_err();
        assert!(error.contains("production branch was already updated"));
        assert!(branch_exists(&path, "release/2.0"));
        assert!(run(&path, &["show-ref", "--verify", "refs/tags/v2.0"]).is_err());
        assert_eq!(
            run(&path, &["show", "main:shared.txt"]).unwrap(),
            "release\n"
        );
        assert!(operation_in_progress(&path).unwrap());
        run(&path, &["merge", "--abort"]).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
