//! Gitea and Forgejo administration through their REST API (v1): browse,
//! create and delete repositories and manage collaborators. Also edits
//! Gitolite's access rules through its gitolite-admin repository.

use crate::{
    external_forge::{client, send},
    git::run,
};
use keyring::{Entry, Error as KeyringError};
use reqwest::Method;
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use url::Url;
use zeroize::Zeroizing;

const LABEL: &str = "Gitea";

fn base(server: &str) -> Result<String, String> {
    let url = Url::parse(server.trim())
        .map_err(|_| "Enter the server address, such as https://git.example.com.")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1"));
    if (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.query().is_some()
    {
        return Err("Use an HTTPS address for the Gitea or Forgejo server.".into());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn entry(server: &str) -> Result<Entry, String> {
    Entry::new("app.weave.desktop", &format!("gitea:{server}"))
        .map_err(|error| format!("Could not open the system keychain: {error}"))
}

fn token(server: &str) -> Result<Zeroizing<String>, String> {
    entry(server)?
        .get_password()
        .map(Zeroizing::new)
        .map_err(|error| match error {
            KeyringError::NoEntry => "Connect this Gitea server first.".to_string(),
            _ => format!("Could not read the system keychain: {error}"),
        })
}

fn call(
    server: &str,
    token: &str,
    method: Method,
    endpoint: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let mut request = client()?
        .request(method, format!("{server}/api/v1{endpoint}"))
        .header("Authorization", format!("token {token}"))
        .header("Accept", "application/json");
    if let Some(body) = body {
        request = request.json(&body);
    }
    send(request, LABEL)
}

fn segment(value: &str, what: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 100
        || value.starts_with('.')
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(format!(
            "Enter a valid {what} using letters, numbers, dots, dashes or underscores."
        ));
    }
    Ok(value.to_string())
}

fn full_name(value: &str) -> Result<(String, String), String> {
    let (owner, repo) = value.split_once('/').ok_or("Choose a repository.")?;
    Ok((segment(owner, "owner")?, segment(repo, "repository name")?))
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GiteaRepository {
    full_name: String,
    description: String,
    private: bool,
    clone_url: String,
    web_url: String,
    default_branch: String,
}

fn repository(value: &Value) -> GiteaRepository {
    let text = |key: &str| value[key].as_str().unwrap_or("").to_string();
    GiteaRepository {
        full_name: text("full_name"),
        description: text("description"),
        private: value["private"].as_bool().unwrap_or(false),
        clone_url: text("clone_url"),
        web_url: text("html_url"),
        default_branch: text("default_branch"),
    }
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Collaborator {
    login: String,
    permission: String,
}

pub(crate) fn connect_at(server: &str, token: &str) -> Result<String, String> {
    let user = call(server, token, Method::GET, "/user", None)?;
    user["login"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "The server did not return your account.".into())
}

pub(crate) fn repositories_at(server: &str, token: &str) -> Result<Vec<GiteaRepository>, String> {
    let mut all = Vec::new();
    for page in 1..=20 {
        let result = call(
            server,
            token,
            Method::GET,
            &format!("/user/repos?limit=50&page={page}"),
            None,
        )?;
        let values = result.as_array().cloned().unwrap_or_default();
        let done = values.len() < 50;
        all.extend(values.iter().map(repository));
        if done {
            break;
        }
    }
    all.sort_by_key(|repo| repo.full_name.to_lowercase());
    Ok(all)
}

pub(crate) fn create_at(
    server: &str,
    token: &str,
    organization: &str,
    name: &str,
    description: &str,
    private: bool,
) -> Result<GiteaRepository, String> {
    let name = segment(name, "repository name")?;
    let endpoint = if organization.trim().is_empty() {
        "/user/repos".to_string()
    } else {
        format!("/orgs/{}/repos", segment(organization, "organization")?)
    };
    let body = json!({"name": name, "description": description.trim(), "private": private, "auto_init": false});
    Ok(repository(&call(
        server,
        token,
        Method::POST,
        &endpoint,
        Some(body),
    )?))
}

pub(crate) fn delete_at(
    server: &str,
    token: &str,
    full: &str,
    confirmation: &str,
) -> Result<(), String> {
    let (owner, repo) = full_name(full)?;
    if confirmation.trim() != format!("{owner}/{repo}") {
        return Err(format!(
            "Type {owner}/{repo} to confirm permanent deletion."
        ));
    }
    call(
        server,
        token,
        Method::DELETE,
        &format!("/repos/{owner}/{repo}"),
        None,
    )?;
    Ok(())
}

pub(crate) fn collaborators_at(
    server: &str,
    token: &str,
    full: &str,
) -> Result<Vec<Collaborator>, String> {
    let (owner, repo) = full_name(full)?;
    let users = call(
        server,
        token,
        Method::GET,
        &format!("/repos/{owner}/{repo}/collaborators?limit=100"),
        None,
    )?;
    let mut collaborators = Vec::new();
    for user in users.as_array().into_iter().flatten() {
        let login = user["login"].as_str().unwrap_or("").to_string();
        let permission = call(
            server,
            token,
            Method::GET,
            &format!("/repos/{owner}/{repo}/collaborators/{login}/permission"),
            None,
        )
        .ok()
        .and_then(|value| value["permission"].as_str().map(str::to_string))
        .unwrap_or_else(|| "read".into());
        collaborators.push(Collaborator { login, permission });
    }
    Ok(collaborators)
}

pub(crate) fn set_collaborator_at(
    server: &str,
    token: &str,
    full: &str,
    user: &str,
    permission: &str,
) -> Result<(), String> {
    let (owner, repo) = full_name(full)?;
    let user = segment(user, "user name")?;
    let endpoint = format!("/repos/{owner}/{repo}/collaborators/{user}");
    match permission {
        "remove" => call(server, token, Method::DELETE, &endpoint, None)?,
        "read" | "write" | "admin" => call(
            server,
            token,
            Method::PUT,
            &endpoint,
            Some(json!({"permission": permission})),
        )?,
        _ => return Err("Choose read, write, admin or remove.".into()),
    };
    Ok(())
}

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| format!("The server request stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn gitea_connect(server: String, token: String) -> Result<String, String> {
    blocking(move || {
        let server = base(&server)?;
        let login = connect_at(&server, token.trim())?;
        entry(&server)?
            .set_password(token.trim())
            .map_err(|error| format!("Could not save the token in the system keychain: {error}"))?;
        Ok(login)
    })
    .await
}

#[tauri::command]
pub fn gitea_disconnect(server: String) -> Result<(), String> {
    match entry(&base(&server)?)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(format!("Could not remove the token: {error}")),
    }
}

#[tauri::command]
pub async fn gitea_repositories(server: String) -> Result<Vec<GiteaRepository>, String> {
    blocking(move || {
        let server = base(&server)?;
        repositories_at(&server, &token(&server)?)
    })
    .await
}

#[tauri::command]
pub async fn gitea_create(
    server: String,
    organization: String,
    name: String,
    description: String,
    private: bool,
) -> Result<GiteaRepository, String> {
    blocking(move || {
        let server = base(&server)?;
        create_at(
            &server,
            &token(&server)?,
            &organization,
            &name,
            &description,
            private,
        )
    })
    .await
}

#[tauri::command]
pub async fn gitea_delete(
    server: String,
    full_name: String,
    confirmation: String,
) -> Result<(), String> {
    blocking(move || {
        let server = base(&server)?;
        delete_at(&server, &token(&server)?, &full_name, &confirmation)
    })
    .await
}

#[tauri::command]
pub async fn gitea_collaborators(
    server: String,
    full_name: String,
) -> Result<Vec<Collaborator>, String> {
    blocking(move || {
        let server = base(&server)?;
        collaborators_at(&server, &token(&server)?, &full_name)
    })
    .await
}

#[tauri::command]
pub async fn gitea_collaborator_set(
    server: String,
    full_name: String,
    user: String,
    permission: String,
) -> Result<(), String> {
    blocking(move || {
        let server = base(&server)?;
        set_collaborator_at(&server, &token(&server)?, &full_name, &user, &permission)
    })
    .await
}

// ---------------------------------------------------------------------------
// Gitolite

fn git_with_ssh(args: &[&str], ssh_command: Option<&str>) -> Result<String, String> {
    let mut command = crate::platform::command("git");
    command.args(args).env("GIT_TERMINAL_PROMPT", "0");
    if let Some(ssh) = ssh_command {
        command.env("GIT_SSH_COMMAND", ssh);
    }
    let output = command
        .output()
        .map_err(|error| format!("Git could not start: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn ssh_command_for(key: Option<&str>) -> Option<String> {
    key.filter(|key| !key.is_empty() && !key.contains(['"', '\'']))
        .map(|key| format!("ssh -o BatchMode=yes -i \"{key}\""))
}

/// Reads conf/gitolite.conf from a gitolite-admin repository URL.
pub(crate) fn gitolite_read(admin_url: &str, key: Option<&str>) -> Result<String, String> {
    let work = tempfile::tempdir().map_err(|error| error.to_string())?;
    let target = work.path().join("admin");
    git_with_ssh(
        &[
            "clone",
            "-q",
            "--depth",
            "1",
            "--",
            admin_url,
            &target.to_string_lossy(),
        ],
        ssh_command_for(key).as_deref(),
    )
    .map_err(|error| format!("Could not open gitolite-admin: {error}"))?;
    fs::read_to_string(target.join("conf/gitolite.conf")).map_err(|_| {
        "This repository has no conf/gitolite.conf. Check that it is gitolite-admin.".into()
    })
}

/// Commits a new conf/gitolite.conf and pushes it; Gitolite applies it on push.
pub(crate) fn gitolite_write(
    admin_url: &str,
    key: Option<&str>,
    config: &str,
    message: &str,
) -> Result<String, String> {
    if config.trim().is_empty() || config.len() > 1_000_000 {
        return Err("The configuration is empty or too large.".into());
    }
    let ssh = ssh_command_for(key);
    let work = tempfile::tempdir().map_err(|error| error.to_string())?;
    let target = work.path().join("admin");
    let target_text = target.to_string_lossy().to_string();
    git_with_ssh(
        &["clone", "-q", "--", admin_url, &target_text],
        ssh.as_deref(),
    )
    .map_err(|error| format!("Could not open gitolite-admin: {error}"))?;
    let conf = target.join("conf/gitolite.conf");
    if !conf.exists() {
        return Err(
            "This repository has no conf/gitolite.conf. Check that it is gitolite-admin.".into(),
        );
    }
    fs::write(&conf, config).map_err(|error| error.to_string())?;
    let repo = target_text.as_str();
    if run(repo, &["status", "--porcelain"])?.is_empty() {
        return Ok("Nothing changed.".into());
    }
    run(repo, &["add", "conf/gitolite.conf"])?;
    let message = if message.trim().is_empty() {
        "Update access rules from Weave"
    } else {
        message.trim()
    };
    run(
        repo,
        &[
            "-c",
            "user.name=Weave",
            "-c",
            "user.email=weave@localhost",
            "commit",
            "-q",
            "-m",
            message,
        ],
    )?;
    let output = git_with_ssh(
        &["-C", repo, "push", "-q", "origin", "HEAD"],
        ssh.as_deref(),
    )
    .map_err(|error| format!("Gitolite rejected the change: {error}"))?;
    Ok(if output.trim().is_empty() {
        "Access rules updated.".into()
    } else {
        output
    })
}

#[tauri::command]
pub async fn gitolite_config(
    admin_url: String,
    key_path: Option<String>,
) -> Result<String, String> {
    blocking(move || gitolite_read(admin_url.trim(), key_path.as_deref())).await
}

#[tauri::command]
pub async fn gitolite_config_save(
    admin_url: String,
    key_path: Option<String>,
    config: String,
    message: String,
) -> Result<String, String> {
    blocking(move || gitolite_write(admin_url.trim(), key_path.as_deref(), &config, &message)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn serve(replies: Vec<&'static str>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut seen = Vec::new();
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut bytes = [0_u8; 4096];
                loop {
                    let count = stream.read(&mut bytes).unwrap();
                    request.extend_from_slice(&bytes[..count]);
                    let text = String::from_utf8_lossy(&request).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{reply}", reply.len()).unwrap();
                seen.push(String::from_utf8(request).unwrap());
            }
            seen
        });
        (base, handle)
    }

    #[test]
    fn manages_gitea_repositories_and_collaborators() {
        let (server, handle) = serve(vec![
            r#"{"login":"ada"}"#,
            r#"[{"full_name":"ada/zeta","private":true,"clone_url":"https://git.test/ada/zeta.git","html_url":"https://git.test/ada/zeta","default_branch":"main"},{"full_name":"ada/alpha","private":false}]"#,
            r#"{"full_name":"team/new","private":true}"#,
            r#"[{"login":"lin"}]"#,
            r#"{"permission":"write"}"#,
            "",
            "",
        ]);
        assert_eq!(connect_at(&server, "tok").unwrap(), "ada");
        let repos = repositories_at(&server, "tok").unwrap();
        assert_eq!(
            repos
                .iter()
                .map(|repo| repo.full_name.as_str())
                .collect::<Vec<_>>(),
            ["ada/alpha", "ada/zeta"]
        );
        assert_eq!(
            create_at(&server, "tok", "team", "new", "", true)
                .unwrap()
                .full_name,
            "team/new"
        );
        assert_eq!(
            collaborators_at(&server, "tok", "ada/zeta").unwrap(),
            vec![Collaborator {
                login: "lin".into(),
                permission: "write".into()
            }]
        );
        set_collaborator_at(&server, "tok", "ada/zeta", "lin", "admin").unwrap();
        assert!(delete_at(&server, "tok", "ada/zeta", "zeta").is_err());
        delete_at(&server, "tok", "ada/zeta", "ada/zeta").unwrap();
        let requests = handle.join().unwrap();
        assert!(requests[0]
            .to_ascii_lowercase()
            .contains("authorization: token tok"));
        assert!(requests[2].starts_with("POST /api/v1/orgs/team/repos HTTP/1.1"));
        assert!(requests[5].starts_with("PUT /api/v1/repos/ada/zeta/collaborators/lin HTTP/1.1"));
        assert!(requests[5].ends_with(r#"{"permission":"admin"}"#));
        assert!(requests[6].starts_with("DELETE /api/v1/repos/ada/zeta HTTP/1.1"));
        assert!(set_collaborator_at(&server, "tok", "ada/zeta", "lin", "owner").is_err());
        assert!(base("ftp://x").is_err());
    }

    #[test]
    fn reads_and_pushes_gitolite_rules() {
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("gitolite-admin.git");
        let seed = dir.path().join("seed");
        let bare_text = bare.to_string_lossy().to_string();
        run(
            &dir.path().to_string_lossy(),
            &["init", "-q", "--bare", &bare_text],
        )
        .unwrap();
        fs::create_dir_all(seed.join("conf")).unwrap();
        let seed_text = seed.to_string_lossy().to_string();
        run(&seed_text, &["init", "-q", "-b", "master"]).unwrap();
        fs::write(
            seed.join("conf/gitolite.conf"),
            "repo gitolite-admin\n    RW+ = admin\n",
        )
        .unwrap();
        run(&seed_text, &["add", "."]).unwrap();
        run(
            &seed_text,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@e.st",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        )
        .unwrap();
        run(&seed_text, &["push", "-q", &bare_text, "master"]).unwrap();
        run(&bare_text, &["symbolic-ref", "HEAD", "refs/heads/master"]).unwrap();

        let config = gitolite_read(&bare_text, None).unwrap();
        assert!(config.contains("RW+ = admin"));
        let updated = format!("{config}\nrepo project\n    RW = ada\n");
        gitolite_write(&bare_text, None, &updated, "Add project").unwrap();
        assert_eq!(gitolite_read(&bare_text, None).unwrap(), updated);
        assert_eq!(
            gitolite_write(&bare_text, None, &updated, "").unwrap(),
            "Nothing changed."
        );
        assert!(gitolite_read(&seed.join("missing").to_string_lossy(), None).is_err());
    }
}
