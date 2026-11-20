//! Bare Git repository administration on SSH hosts (or this Mac): list,
//! create, configure and delete repositories without a hosting service.
//! Commands run through the noninteractive script runner, so they use key or
//! agent authentication and a trusted host key.

use crate::{profiles, terminal, AppState};
use serde::Serialize;
use tauri::{AppHandle, Manager};

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRepository {
    pub path: String,
    pub description: String,
    pub default_branch: String,
    pub deny_force_push: bool,
}

/// Paths may be absolute or start with `~/`, must end in `.git`, and use a
/// conservative character set so they are safe inside single quotes.
/// A Windows path such as `C:/git/app.git` without its drive, so the same
/// checks apply on every system. Paths use forward slashes.
fn without_drive(path: &str) -> &str {
    let bytes = path.as_bytes();
    if bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/' {
        &path[2..]
    } else {
        path
    }
}

pub(crate) fn valid_repo_path(path: &str) -> bool {
    let path = without_drive(path);
    let body = path.strip_prefix("~/").unwrap_or(path);
    (path.starts_with('/') || path.starts_with("~/"))
        && path.ends_with(".git")
        && path.len() <= 512
        && !body
            .split('/')
            .any(|part| part == ".." || part.starts_with('-'))
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | '@'))
}

fn valid_folder(path: &str) -> bool {
    let path = without_drive(path);
    path == "~"
        || ((path.starts_with('/') || path.starts_with("~/"))
            && path.len() <= 512
            && !path
                .split('/')
                .any(|part| part == ".." || part.starts_with('-'))
            && path.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | '@')
            }))
}

fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with(['-', '/'])
        && !name.ends_with(['/', '.'])
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
}

/// Shell word for a validated path: `~/x` becomes `"$HOME"/'x'`.
fn shell_path(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => format!("\"$HOME\"/'{rest}'"),
        None if path == "~" => "\"$HOME\"".into(),
        None => format!("'{path}'"),
    }
}

fn single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn run(profile: Option<&profiles::Profile>, script: &str) -> Result<String, String> {
    let result = terminal::run_script(profile, None, script)?;
    if result.exit_code == Some(0) {
        Ok(result.stdout)
    } else {
        let detail = result.stderr.trim();
        Err(if detail.is_empty() {
            format!("The command failed with exit code {:?}.", result.exit_code)
        } else {
            detail.lines().last().unwrap_or(detail).to_owned()
        })
    }
}

const FIELD: char = '\u{1f}';

pub(crate) fn list(
    profile: Option<&profiles::Profile>,
    folder: &str,
) -> Result<Vec<RemoteRepository>, String> {
    if !valid_folder(folder) {
        return Err("Enter a folder such as ~/git or /srv/git.".into());
    }
    let script = format!(
        r#"set -eu
base={base}
[ -d "$base" ] || {{ echo "Folder does not exist: $base" >&2; exit 2; }}
find "$base" -maxdepth 3 -type d -name '*.git' -prune 2>/dev/null | sort | head -n 500 | while IFS= read -r repo; do
  [ -f "$repo/HEAD" ] && [ -d "$repo/objects" ] || continue
  [ "$(git --git-dir="$repo" rev-parse --is-bare-repository 2>/dev/null)" = true ] || continue
  desc=$(head -n 1 "$repo/description" 2>/dev/null | tr -d '\037' || true)
  case "$desc" in Unnamed\ repository*) desc="" ;; esac
  head=$(git --git-dir="$repo" symbolic-ref --short HEAD 2>/dev/null || true)
  deny=$(git --git-dir="$repo" config --bool receive.denyNonFastForwards 2>/dev/null || echo false)
  printf '%s\037%s\037%s\037%s\n' "$repo" "$desc" "$head" "$deny"
done
"#,
        base = shell_path(folder)
    );
    Ok(run(profile, &script)?
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(FIELD).collect();
            (parts.len() == 4).then(|| RemoteRepository {
                path: parts[0].into(),
                description: parts[1].into(),
                default_branch: parts[2].into(),
                deny_force_push: parts[3] == "true",
            })
        })
        .collect())
}

pub(crate) fn create(
    profile: Option<&profiles::Profile>,
    path: &str,
    branch: &str,
    description: &str,
) -> Result<String, String> {
    if !valid_repo_path(path) {
        return Err("Enter a repository path ending in .git, such as ~/git/project.git.".into());
    }
    if !valid_branch(branch) {
        return Err("Enter a valid default branch name, such as main.".into());
    }
    if description.len() > 300 || description.chars().any(char::is_control) {
        return Err("Keep the description on one line and under 300 characters.".into());
    }
    let script = format!(
        r#"set -eu
repo={repo}
if [ -e "$repo" ]; then echo "Something already exists at $repo." >&2; exit 2; fi
mkdir -p "$(dirname "$repo")"
git init --quiet --bare "$repo"
git --git-dir="$repo" symbolic-ref HEAD refs/heads/{branch}
printf '%s\n' {description} > "$repo/description"
cd "$repo" && pwd -P
"#,
        repo = shell_path(path),
        branch = branch,
        description = single_quote(description.trim()),
    );
    Ok(run(profile, &script)?.trim().to_owned())
}

pub(crate) fn configure(
    profile: Option<&profiles::Profile>,
    path: &str,
    branch: &str,
    description: &str,
    deny_force_push: bool,
) -> Result<(), String> {
    if !valid_repo_path(path) {
        return Err("Choose a repository path ending in .git.".into());
    }
    if !valid_branch(branch) {
        return Err("Enter a valid default branch name, such as main.".into());
    }
    if description.len() > 300 || description.chars().any(char::is_control) {
        return Err("Keep the description on one line and under 300 characters.".into());
    }
    let script = format!(
        r#"set -eu
repo={repo}
[ "$(git --git-dir="$repo" rev-parse --is-bare-repository 2>/dev/null)" = true ] || {{ echo "$repo is not a bare Git repository." >&2; exit 2; }}
git --git-dir="$repo" symbolic-ref HEAD refs/heads/{branch}
printf '%s\n' {description} > "$repo/description"
git --git-dir="$repo" config receive.denyNonFastForwards {deny}
git --git-dir="$repo" config receive.denyDeletes {deny}
"#,
        repo = shell_path(path),
        description = single_quote(description.trim()),
        deny = deny_force_push,
    );
    run(profile, &script).map(|_| ())
}

pub(crate) fn delete(
    profile: Option<&profiles::Profile>,
    path: &str,
    confirmation: &str,
) -> Result<(), String> {
    if !valid_repo_path(path) {
        return Err("Choose a repository path ending in .git.".into());
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    if confirmation.trim() != name {
        return Err(format!("Type {name} to confirm permanent deletion."));
    }
    let script = format!(
        r#"set -eu
repo={repo}
[ -f "$repo/HEAD" ] && [ -d "$repo/objects" ] && [ "$(git --git-dir="$repo" rev-parse --is-bare-repository 2>/dev/null)" = true ] || {{ echo "$repo is not a bare Git repository, so it was not deleted." >&2; exit 2; }}
rm -rf -- "$repo"
"#,
        repo = shell_path(path)
    );
    run(profile, &script).map(|_| ())
}

pub const SERVER_HOOKS: [&str; 6] = [
    "pre-receive",
    "update",
    "post-receive",
    "post-update",
    "push-to-checkout",
    "reference-transaction",
];

fn hook_script(path: &str, hook: &str) -> Result<String, String> {
    if !valid_repo_path(path) {
        return Err("Choose a repository path ending in .git.".into());
    }
    if !SERVER_HOOKS.contains(&hook) {
        return Err(format!(
            "Choose one of these hooks: {}.",
            SERVER_HOOKS.join(", ")
        ));
    }
    Ok(format!(
        r#"set -eu
repo={repo}
[ "$(git --git-dir="$repo" rev-parse --is-bare-repository 2>/dev/null)" = true ] || {{ echo "$repo is not a bare Git repository." >&2; exit 2; }}
hooks=$(git --git-dir="$repo" rev-parse --git-path hooks)
case "$hooks" in /*) ;; *) hooks="$repo/$hooks" ;; esac
file="$hooks/{hook}"
"#,
        repo = shell_path(path)
    ))
}

/// Names of hooks that are installed (sample files are ignored).
pub(crate) fn hooks_list(
    profile: Option<&profiles::Profile>,
    path: &str,
) -> Result<Vec<String>, String> {
    let script = hook_script(path, "update")?
        + r#"for name in pre-receive update post-receive post-update push-to-checkout reference-transaction; do
  [ -f "$hooks/$name" ] && echo "$name"
done
true
"#;
    Ok(run(profile, &script)?.lines().map(String::from).collect())
}

pub(crate) fn hook_read(
    profile: Option<&profiles::Profile>,
    path: &str,
    hook: &str,
) -> Result<String, String> {
    let script = hook_script(path, hook)? + r#"[ -f "$file" ] && cat "$file" || true"#;
    run(profile, &script)
}

pub(crate) fn hook_save(
    profile: Option<&profiles::Profile>,
    path: &str,
    hook: &str,
    content: &str,
) -> Result<(), String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    if content.len() > 64 * 1024 || content.contains('\0') {
        return Err("Keep the hook under 64 KB of text.".into());
    }
    if !content.starts_with("#!") {
        return Err(
            "Start the hook with a line such as #!/bin/sh so the server knows how to run it."
                .into(),
        );
    }
    let encoded = STANDARD.encode(content.as_bytes());
    let script = hook_script(path, hook)?
        + &format!(
            r#"mkdir -p "$hooks"
printf '%s' '{encoded}' | base64 --decode > "$file.weave-tmp"
chmod 755 "$file.weave-tmp"
mv "$file.weave-tmp" "$file"
"#
        );
    run(profile, &script).map(|_| ())
}

pub(crate) fn hook_delete(
    profile: Option<&profiles::Profile>,
    path: &str,
    hook: &str,
) -> Result<(), String> {
    let script = hook_script(path, hook)? + r#"rm -f "$file""#;
    run(profile, &script).map(|_| ())
}

/// `ssh://user@host:port/path` for a repository on a profile's host.
pub(crate) fn clone_url(profile: Option<&profiles::Profile>, path: &str) -> String {
    match profile {
        Some(profile) => {
            let path = match path.strip_prefix("~/") {
                Some(rest) => format!("~{}/{rest}", profile.username),
                None => path.to_owned(),
            };
            format!(
                "ssh://{}@{}:{}/{}",
                profile.username,
                profile.host,
                profile.port,
                path.trim_start_matches('/')
            )
        }
        None => path.to_owned(),
    }
}

fn load_profile(
    app: &AppHandle,
    profile_id: Option<&str>,
) -> Result<Option<profiles::Profile>, String> {
    let Some(id) = profile_id.filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    let profile = profiles::get_profile(&app.state::<AppState>(), id)?;
    if profile.protocol != "ssh" {
        return Err("Choose an SSH host to manage Git repositories.".into());
    }
    Ok(Some(profile))
}

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| format!("Remote Git command stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn git_remote_repos_list(
    app: AppHandle,
    profile_id: Option<String>,
    folder: String,
) -> Result<Vec<RemoteRepository>, String> {
    blocking(move || {
        list(
            load_profile(&app, profile_id.as_deref())?.as_ref(),
            folder.trim(),
        )
    })
    .await
}

#[tauri::command]
pub async fn git_remote_repo_create(
    app: AppHandle,
    profile_id: Option<String>,
    path: String,
    default_branch: String,
    description: String,
) -> Result<String, String> {
    blocking(move || {
        let profile = load_profile(&app, profile_id.as_deref())?;
        let created = create(
            profile.as_ref(),
            path.trim(),
            default_branch.trim(),
            &description,
        )?;
        Ok(clone_url(
            profile.as_ref(),
            if profile.is_some() {
                path.trim()
            } else {
                &created
            },
        ))
    })
    .await
}

#[tauri::command]
pub async fn git_remote_repo_configure(
    app: AppHandle,
    profile_id: Option<String>,
    path: String,
    default_branch: String,
    description: String,
    deny_force_push: bool,
) -> Result<(), String> {
    blocking(move || {
        configure(
            load_profile(&app, profile_id.as_deref())?.as_ref(),
            path.trim(),
            default_branch.trim(),
            &description,
            deny_force_push,
        )
    })
    .await
}

#[tauri::command]
pub async fn git_remote_repo_delete(
    app: AppHandle,
    profile_id: Option<String>,
    path: String,
    confirmation: String,
) -> Result<(), String> {
    blocking(move || {
        delete(
            load_profile(&app, profile_id.as_deref())?.as_ref(),
            path.trim(),
            &confirmation,
        )
    })
    .await
}

#[tauri::command]
pub async fn git_remote_hooks(
    app: AppHandle,
    profile_id: Option<String>,
    path: String,
) -> Result<Vec<String>, String> {
    blocking(move || {
        hooks_list(
            load_profile(&app, profile_id.as_deref())?.as_ref(),
            path.trim(),
        )
    })
    .await
}

#[tauri::command]
pub async fn git_remote_hook_read(
    app: AppHandle,
    profile_id: Option<String>,
    path: String,
    hook: String,
) -> Result<String, String> {
    blocking(move || {
        hook_read(
            load_profile(&app, profile_id.as_deref())?.as_ref(),
            path.trim(),
            &hook,
        )
    })
    .await
}

#[tauri::command]
pub async fn git_remote_hook_save(
    app: AppHandle,
    profile_id: Option<String>,
    path: String,
    hook: String,
    content: String,
) -> Result<(), String> {
    blocking(move || {
        hook_save(
            load_profile(&app, profile_id.as_deref())?.as_ref(),
            path.trim(),
            &hook,
            &content,
        )
    })
    .await
}

#[tauri::command]
pub async fn git_remote_hook_delete(
    app: AppHandle,
    profile_id: Option<String>,
    path: String,
    hook: String,
) -> Result<(), String> {
    blocking(move || {
        hook_delete(
            load_profile(&app, profile_id.as_deref())?.as_ref(),
            path.trim(),
            &hook,
        )
    })
    .await
}

#[cfg(test)]
#[cfg_attr(windows, allow(unused_imports, dead_code))]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn manages_server_hooks_that_run_on_push() {
        let root = tempfile::tempdir().unwrap();
        let folder = dunce::canonicalize(root.path()).unwrap();
        let repo = format!("{}/hooked.git", folder.display());
        create(None, &repo, "main", "").unwrap();
        assert!(hooks_list(None, &repo).unwrap().is_empty());
        assert!(hook_save(None, &repo, "post-receive", "echo hi")
            .unwrap_err()
            .contains("#!/bin/sh"));
        assert!(hook_save(None, &repo, "pre-commit", "#!/bin/sh\n").is_err());
        let marker = folder.join("pushed");
        let hook = format!("#!/bin/sh\n# it's quoted\ntouch '{}'\n", marker.display());
        hook_save(None, &repo, "post-receive", &hook).unwrap();
        assert_eq!(hooks_list(None, &repo).unwrap(), ["post-receive"]);
        assert_eq!(hook_read(None, &repo, "post-receive").unwrap(), hook);

        let work = folder.join("work");
        run_git(&["clone", "-q", &repo, &work.to_string_lossy()]);
        std::fs::write(work.join("a.txt"), "a").unwrap();
        let work = work.to_string_lossy();
        run_git(&[
            "-C",
            &work,
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.test",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ]);
        run_git(&["-C", &work, "push", "-q", "origin", "HEAD:main"]);
        assert!(marker.exists());

        hook_delete(None, &repo, "post-receive").unwrap();
        assert!(hooks_list(None, &repo).unwrap().is_empty());
    }

    fn run_git(args: &[&str]) {
        let output = crate::platform::command("git").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn validates_repository_paths_and_branches() {
        assert!(valid_repo_path("~/git/project.git"));
        assert!(valid_repo_path("/srv/git/team/app.git"));
        assert!(!valid_repo_path("relative.git"));
        assert!(!valid_repo_path("/srv/git/app"));
        assert!(!valid_repo_path("/srv/../etc.git"));
        assert!(!valid_repo_path("/srv/git/it's.git"));
        assert!(!valid_repo_path("/srv/-x.git"));
        assert!(valid_branch("main"));
        assert!(valid_branch("release/2026"));
        assert!(!valid_branch("-f"));
        assert!(!valid_branch("a..b"));
        assert!(!valid_branch("a b"));
    }

    #[test]
    fn creates_configures_lists_clones_and_deletes_bare_repositories() {
        let root = tempfile::tempdir().unwrap();
        let folder = dunce::canonicalize(root.path()).unwrap();
        let folder = folder.to_string_lossy().replace('\\', "/");
        let repo = format!("{folder}/team/app.git");
        assert!(valid_repo_path("C:/git/app.git"));
        assert!(!valid_repo_path("C:git/app.git"));
        let created = create(None, &repo, "trunk", "Team app — it's ours").unwrap();
        assert!(created.ends_with("/team/app.git"));
        assert!(create(None, &repo, "trunk", "")
            .unwrap_err()
            .contains("already exists"));

        let listed = list(None, &folder).unwrap();
        assert_eq!(
            listed,
            vec![RemoteRepository {
                path: repo.clone(),
                description: "Team app — it's ours".into(),
                default_branch: "trunk".into(),
                deny_force_push: false,
            }]
        );

        configure(None, &repo, "main", "Renamed", true).unwrap();
        let listed = list(None, &folder).unwrap();
        assert_eq!(listed[0].default_branch, "main");
        assert!(listed[0].deny_force_push);

        let work = root.path().join("work");
        let output = crate::platform::command("git")
            .args(["clone", "-q", &repo])
            .arg(&work)
            .output()
            .unwrap();
        assert!(output.status.success());

        // Folders that are not bare repositories are never deleted.
        let fake = format!("{folder}/fake.git");
        std::fs::create_dir(&fake).unwrap();
        assert!(delete(None, &fake, "fake.git")
            .unwrap_err()
            .contains("not a bare"));
        assert!(std::path::Path::new(&fake).exists());
        assert!(delete(None, &repo, "wrong.git")
            .unwrap_err()
            .contains("Type app.git"));
        delete(None, &repo, "app.git").unwrap();
        assert!(!std::path::Path::new(&repo).exists());
        assert!(list(None, &format!("{folder}/missing")).is_err());
    }

    #[test]
    fn builds_ssh_clone_urls() {
        let profile = profiles::Profile {
            id: "h".into(),
            name: "NAS".into(),
            host: "nas.local".into(),
            protocol: "ssh".into(),
            port: 2222,
            username: "git".into(),
            key_path: None,
            jump_host: None,
            color: "#000".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        assert_eq!(
            clone_url(Some(&profile), "/srv/git/app.git"),
            "ssh://git@nas.local:2222/srv/git/app.git"
        );
        assert_eq!(
            clone_url(Some(&profile), "~/repos/app.git"),
            "ssh://git@nas.local:2222/~git/repos/app.git"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires the local SSH test server"]
    fn manages_repositories_on_a_real_ssh_host() {
        let port = std::env::var("WEAVE_SSH_TEST_PORT").unwrap();
        let key = std::env::var("WEAVE_SSH_TEST_KEY").unwrap();
        let known_hosts = std::env::var("WEAVE_SSH_TEST_KNOWN_HOSTS").unwrap();
        let username = std::env::var("WEAVE_SSH_TEST_USER").unwrap();
        let profile = profiles::Profile {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Local SSH test".into(),
            host: "127.0.0.1".into(),
            protocol: "ssh".into(),
            port: port.parse().unwrap(),
            username: username.clone(),
            key_path: Some(key.clone()),
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let socket = crate::ssh::control_path(&profile).unwrap();
        let master = crate::platform::command("ssh")
            .args([
                "-MNf",
                "-o",
                "ControlMaster=yes",
                "-o",
                "ControlPersist=10",
                "-o",
                &format!("ControlPath={}", socket.display()),
                "-o",
                &format!("UserKnownHostsFile={known_hosts}"),
                "-o",
                "StrictHostKeyChecking=yes",
                "-p",
                &port,
                "-i",
                &key,
                "-l",
                &username,
                "127.0.0.1",
            ])
            .output()
            .unwrap();
        assert!(master.status.success());

        let root = tempfile::tempdir().unwrap();
        let folder = dunce::canonicalize(root.path()).unwrap();
        let folder = folder.to_string_lossy();
        let repo = format!("{folder}/served/app.git");
        create(Some(&profile), &repo, "main", "Served over SSH").unwrap();
        let listed = list(Some(&profile), &folder).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].description, "Served over SSH");

        let url = clone_url(Some(&profile), &repo);
        let work = root.path().join("work");
        let cloned = crate::platform::command("git")
            .args(["clone", "-q", &url])
            .arg(&work)
            .env(
                "GIT_SSH_COMMAND",
                format!(
                    "ssh -i {key} -o UserKnownHostsFile={known_hosts} -o StrictHostKeyChecking=yes -o BatchMode=yes"
                ),
            )
            .output()
            .unwrap();
        assert!(
            cloned.status.success(),
            "{}",
            String::from_utf8_lossy(&cloned.stderr)
        );
        configure(Some(&profile), &repo, "main", "Locked", true).unwrap();
        assert!(list(Some(&profile), &folder).unwrap()[0].deny_force_push);
        delete(Some(&profile), &repo, "app.git").unwrap();
        assert!(list(Some(&profile), &folder).unwrap().is_empty());
    }
}
