use keyring::{Entry, Error as KeyringError};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::process::Stdio;
use url::Url;
use zeroize::Zeroizing;

use crate::git;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    GitHub,
    GitLab,
}

impl Provider {
    fn name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
        }
    }

    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "GitHub" => Ok(Self::GitHub),
            "GitLab" => Ok(Self::GitLab),
            _ => Err("Choose GitHub or GitLab.".into()),
        }
    }
}

#[derive(Clone, Debug)]
struct ForgeRemote {
    provider: Provider,
    host: String,
    project: String,
}

pub(crate) struct GitAuth {
    pub username: String,
    pub host: String,
    pub token: Zeroizing<String>,
}

impl ForgeRemote {
    fn parse(remote: &str) -> Result<Self, String> {
        Self::parse_with_provider(remote, None)
    }

    fn parse_with_provider(remote: &str, selected: Option<Provider>) -> Result<Self, String> {
        let remote = remote.trim();
        let (host, path) = if let Some(rest) = remote.strip_prefix("git@") {
            let (host, path) = rest
                .split_once(':')
                .ok_or("The origin SSH remote needs a host and repository path.")?;
            let parsed = Url::parse(&format!("https://{host}"))
                .map_err(|_| "The SSH origin has an invalid server name.")?;
            if parsed.host_str() != Some(host)
                || !parsed.username().is_empty()
                || parsed.port().is_some()
                || parsed.path() != "/"
            {
                return Err("The SSH origin has an invalid server name.".into());
            }
            (host.to_owned(), path.to_owned())
        } else {
            let url = Url::parse(remote)
                .map_err(|_| "The origin remote is not a valid GitHub or GitLab URL.")?;
            if !matches!(url.scheme(), "https" | "ssh") {
                return Err("Use an HTTPS or SSH origin remote for GitHub and GitLab.".into());
            }
            if url.password().is_some()
                || (url.scheme() == "https" && !url.username().is_empty())
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(
                    "Remove embedded credentials or query text from the origin remote URL.".into(),
                );
            }
            if url.port().is_some() {
                return Err(
                    "Custom ports are not supported for GitHub and GitLab accounts.".into(),
                );
            }
            let host = url
                .host_str()
                .ok_or("The origin remote URL has no host.")?
                .to_owned();
            if url.scheme() == "ssh" && url.username() != "git" {
                return Err("The SSH origin remote must use the git account.".into());
            }
            (host, url.path().trim_start_matches('/').to_owned())
        };
        let provider = match host.as_str() {
            "github.com" => Provider::GitHub,
            "gitlab.com" => Provider::GitLab,
            _ => match selected {
                Some(provider) => provider,
                None => Self::saved_provider(&host)?,
            },
        };
        if selected.is_some_and(|selected| selected != provider) {
            return Err(format!("{host} is already known as {}.", provider.name()));
        }
        let project = path.strip_suffix(".git").unwrap_or(&path).trim_matches('/');
        let segments: Vec<&str> = project.split('/').collect();
        if segments.len() < 2
            || (provider == Provider::GitHub && segments.len() != 2)
            || segments.iter().any(|part| {
                part.is_empty()
                    || *part == "."
                    || *part == ".."
                    || part.contains(['?', '#', '\\', '@'])
            })
        {
            return Err(
                "The origin remote needs a valid owner or group and repository name.".into(),
            );
        }
        Ok(Self {
            provider,
            host,
            project: project.to_owned(),
        })
    }

    fn saved_provider(host: &str) -> Result<Provider, String> {
        let entry = Entry::new("app.weave.desktop", &format!("forge:host:{host}:provider"))
            .map_err(|error| format!("Could not open the system keychain: {error}"))?;
        match entry.get_password() {
            Ok(provider) => Provider::parse(&provider),
            Err(KeyringError::NoEntry) => Err(format!(
                "{host} is not connected as a self-hosted GitHub or GitLab server. Choose its provider and connect a token."
            )),
            Err(error) => Err(format!("Could not read the system keychain: {error}")),
        }
    }

    fn remember_provider(&self) -> Result<(), String> {
        if matches!(self.host.as_str(), "github.com" | "gitlab.com") {
            return Ok(());
        }
        Entry::new(
            "app.weave.desktop",
            &format!("forge:host:{}:provider", self.host),
        )
        .map_err(|error| format!("Could not open the system keychain: {error}"))?
        .set_password(self.provider.name())
        .map_err(|error| format!("Could not save the server type: {error}"))
    }

    fn from_repo(path: &str) -> Result<Self, String> {
        let remote = git::run(path, &["config", "--get", "remote.origin.url"]).map_err(|_| {
            "This repository has no origin remote. Add a GitHub or GitLab origin first."
        })?;
        Self::parse(&remote)
    }

    fn api_base(&self) -> String {
        match (self.provider, self.host.as_str()) {
            (Provider::GitHub, "github.com") => "https://api.github.com".into(),
            (Provider::GitHub, host) => format!("https://{host}/api/v3"),
            (Provider::GitLab, host) => format!("https://{host}/api/v4"),
        }
    }

    fn graphql_base(&self) -> String {
        if self.host == "github.com" {
            "https://api.github.com".into()
        } else {
            format!("https://{}/api", self.host)
        }
    }

    fn project_path(&self) -> String {
        match self.provider {
            Provider::GitHub => format!("/repos/{}", self.project),
            Provider::GitLab => format!(
                "/projects/{}",
                utf8_percent_encode(&self.project, NON_ALPHANUMERIC)
            ),
        }
    }

    fn account_entry(&self) -> Result<Entry, String> {
        let key = if matches!(self.host.as_str(), "github.com" | "gitlab.com") {
            format!("forge:{}:token", self.provider.name())
        } else {
            format!("forge:{}:{}:token", self.provider.name(), self.host)
        };
        Entry::new("app.weave.desktop", &key)
            .map_err(|error| format!("Could not open the system keychain: {error}"))
    }

    fn token(&self) -> Result<Zeroizing<String>, String> {
        let entry = self.account_entry()?;
        crate::forge_oauth::refresh_if_needed(&self.host, &entry)?;
        match entry.get_password() {
            Ok(token) => Ok(Zeroizing::new(token)),
            Err(KeyringError::NoEntry) => Err(format!(
                "Connect your {} account in this repository before using its online features.",
                self.provider.name()
            )),
            Err(error) => Err(format!(
                "Could not read your account from the system keychain: {error}"
            )),
        }
    }
}

pub(crate) fn git_auth_for_url(url: &str) -> Result<Option<GitAuth>, String> {
    if !Url::parse(url).is_ok_and(|parsed| parsed.scheme() == "https") {
        return Ok(None);
    }
    let remote = match ForgeRemote::parse(url) {
        Ok(remote) => remote,
        Err(_) => return crate::external_forge::git_auth_for_url(url),
    };
    let entry = remote.account_entry()?;
    crate::forge_oauth::refresh_if_needed(&remote.host, &entry)?;
    let token = match entry.get_password() {
        Ok(token) => Zeroizing::new(token),
        Err(KeyringError::NoEntry) => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Could not read Git credentials from the system keychain: {error}"
            ))
        }
    };
    let username = match remote.provider {
        Provider::GitHub => "x-access-token",
        Provider::GitLab => "oauth2",
    }
    .to_string();
    Ok(Some(GitAuth {
        username,
        host: remote.host,
        token,
    }))
}

pub(crate) fn git_auth_for_repo(path: &str) -> Result<Option<GitAuth>, String> {
    let url = match git::run(path, &["config", "--get", "remote.origin.url"]) {
        Ok(url) => url,
        Err(_) => return Ok(None),
    };
    git_auth_for_url(url.trim())
}

fn askpass_answer<'a>(prompt: &str, host: &str, username: &'a str, token: &'a str) -> &'a str {
    let quoted_url = prompt.split('\'').nth(1).unwrap_or("");
    let matches_host = Url::parse(quoted_url)
        .ok()
        .is_some_and(|url| url.scheme() == "https" && url.host_str() == Some(host));
    if !matches_host {
        return "";
    }
    let prompt = prompt.to_ascii_lowercase();
    if prompt.starts_with("username") {
        username
    } else if prompt.starts_with("password") {
        token
    } else {
        ""
    }
}

pub(crate) fn serve_git_askpass() -> bool {
    if std::env::var_os("WEAVE_GIT_ASKPASS").is_none() {
        return false;
    }
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let username = std::env::var("WEAVE_GIT_USERNAME").unwrap_or_default();
    let host = std::env::var("WEAVE_GIT_HOST").unwrap_or_default();
    let token = Zeroizing::new(std::env::var("WEAVE_GIT_TOKEN").unwrap_or_default());
    println!("{}", askpass_answer(&prompt, &host, &username, &token));
    true
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeInfo {
    provider: String,
    host: String,
    project: String,
    connected: bool,
    url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeItem {
    number: i64,
    title: String,
    state: String,
    url: String,
    detail: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgePage {
    items: Vec<ForgeItem>,
    has_more: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeAccount {
    provider: String,
    connected: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeRepository {
    name: String,
    https_url: String,
    ssh_url: String,
    private: bool,
}

fn account_remote(provider: Provider) -> ForgeRemote {
    ForgeRemote {
        provider,
        host: match provider {
            Provider::GitHub => "github.com",
            Provider::GitLab => "gitlab.com",
        }
        .into(),
        project: String::new(),
    }
}

fn repository(value: &Value, remote: &ForgeRemote) -> Option<ForgeRepository> {
    let (name, https_url, ssh_url, private) = match remote.provider {
        Provider::GitHub => (
            field(value, "full_name"),
            field(value, "clone_url"),
            field(value, "ssh_url"),
            value["private"].as_bool().unwrap_or(false),
        ),
        Provider::GitLab => (
            field(value, "path_with_namespace"),
            field(value, "http_url_to_repo"),
            field(value, "ssh_url_to_repo"),
            field(value, "visibility") == "private",
        ),
    };
    let https = ForgeRemote::parse_with_provider(https_url, Some(remote.provider)).ok()?;
    let ssh = ForgeRemote::parse_with_provider(ssh_url, Some(remote.provider)).ok()?;
    if name.is_empty()
        || https.host != remote.host
        || ssh.host != remote.host
        || https.project != name
        || ssh.project != name
    {
        return None;
    }
    Some(ForgeRepository {
        name: name.to_owned(),
        https_url: https_url.to_owned(),
        ssh_url: ssh_url.to_owned(),
        private,
    })
}

struct ForgeRequest<'a> {
    remote: &'a ForgeRemote,
    token: &'a str,
    method: &'static str,
    endpoint: String,
    payload: Option<Value>,
}

impl ForgeRequest<'_> {
    fn json(mut self, payload: Value) -> Self {
        self.payload = Some(payload);
        self
    }
}

fn request<'a>(
    remote: &'a ForgeRemote,
    token: &'a str,
    method: &'static str,
    endpoint: &str,
) -> ForgeRequest<'a> {
    ForgeRequest {
        remote,
        token,
        method,
        endpoint: endpoint.to_owned(),
        payload: None,
    }
}

fn curl_quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

fn send(request: ForgeRequest<'_>) -> Result<Value, String> {
    let base = request.remote.api_base();
    send_at(request, &base)
}

fn send_at(request: ForgeRequest<'_>, base: &str) -> Result<Value, String> {
    let provider = request.remote.provider;
    let url = format!("{}{}", base, request.endpoint);
    let mut config = Zeroizing::new(format!(
        "url = {}\nrequest = {}\nheader = {}\n",
        curl_quote(&url),
        curl_quote(request.method),
        curl_quote("User-Agent: Weave/0.1")
    ));
    let auth = Zeroizing::new(match provider {
        Provider::GitHub => {
            config.push_str(&format!(
                "header = {}\n",
                curl_quote("Accept: application/vnd.github+json")
            ));
            if request.remote.host == "github.com" {
                config.push_str(&format!(
                    "header = {}\n",
                    curl_quote("X-GitHub-Api-Version: 2026-03-10")
                ));
            }
            format!("Authorization: Bearer {}", request.token)
        }
        Provider::GitLab => format!("PRIVATE-TOKEN: {}", request.token),
    });
    config.push_str(&format!("header = {}\n", curl_quote(&auth)));
    if let Some(payload) = request.payload {
        config.push_str(&format!(
            "header = {}\n",
            curl_quote("Content-Type: application/json")
        ));
        config.push_str(&format!(
            "data-binary = {}\n",
            curl_quote(&payload.to_string())
        ));
    }
    let mut child = crate::platform::command("curl")
        .args([
            "-q",
            "--config",
            "-",
            "--silent",
            "--show-error",
            "--max-time",
            "20",
            "--write-out",
            "\n%{http_code}",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start curl: {error}"))?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(config.as_bytes())
        .map_err(|error| format!("Could not send the {} request: {error}", provider.name()))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("Could not read the {} response: {error}", provider.name()))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Could not reach {}: {}",
            provider.name(),
            detail.trim()
        ));
    }
    let output = String::from_utf8(output.stdout)
        .map_err(|_| format!("{} returned text that could not be read.", provider.name()))?;
    let (body_text, status_text) = output
        .rsplit_once('\n')
        .ok_or_else(|| format!("{} returned an incomplete response.", provider.name()))?;
    let status = status_text
        .parse::<u16>()
        .map_err(|_| format!("{} returned an invalid response status.", provider.name()))?;
    if (300..400).contains(&status) {
        return Err(format!(
            "{} redirected the API request. Check the repository URL and account configuration.",
            provider.name()
        ));
    }
    let body: Value = if body_text.trim().is_empty() {
        Value::Null
    } else {
        match serde_json::from_str(body_text) {
            Ok(body) => body,
            Err(_) if status >= 400 => {
                return Err(format!(
                    "{} rejected the request (HTTP {status}). Check your token permissions and try again.",
                    provider.name()
                ));
            }
            Err(_) => {
                return Err(format!(
                    "{} returned a response Weave could not read.",
                    provider.name()
                ))
            }
        }
    };
    if !(200..300).contains(&status) {
        let message = provider_message(&body);
        let explanation = match status {
            401 => "Your access token is invalid or expired. Reconnect the account.",
            403 => "Access was denied. Check your token permissions and repository access.",
            404 => "The repository or item was not found, or your token cannot access it.",
            429 => "The provider rate limit was reached. Try again later.",
            _ => &message,
        };
        return Err(format!(
            "{}: {explanation} (HTTP {status})",
            provider.name()
        ));
    }
    Ok(body)
}

fn provider_message(body: &Value) -> String {
    let message = body.get("message").or_else(|| body.get("error"));
    match message {
        Some(Value::String(message)) => message.clone(),
        Some(Value::Object(fields)) => fields
            .iter()
            .map(|(name, value)| {
                let detail = value
                    .as_array()
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_else(|| value.as_str().unwrap_or("Invalid value").to_owned());
                format!("{name}: {detail}")
            })
            .collect::<Vec<_>>()
            .join("; "),
        _ => "The request was rejected.".into(),
    }
}

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn item(value: &Value, provider: Provider, kind: &str) -> Option<ForgeItem> {
    let number = value
        .get(if provider == Provider::GitLab {
            "iid"
        } else {
            "number"
        })?
        .as_i64()?;
    let url = field(
        value,
        if provider == Provider::GitLab {
            "web_url"
        } else {
            "html_url"
        },
    );
    if url.is_empty() {
        return None;
    }
    let detail = match kind {
        "reviews" => format!(
            "{} → {}",
            if provider == Provider::GitHub {
                field(&value["head"], "ref")
            } else {
                field(value, "source_branch")
            },
            if provider == Provider::GitHub {
                field(&value["base"], "ref")
            } else {
                field(value, "target_branch")
            }
        ),
        _ => value["user"]["login"]
            .as_str()
            .or_else(|| value["author"]["username"].as_str())
            .unwrap_or("")
            .to_owned(),
    };
    Some(ForgeItem {
        number,
        title: field(value, "title").to_owned(),
        state: field(value, "state").to_owned(),
        url: url.to_owned(),
        detail,
    })
}

fn list(path: &str, kind: &str, page: u32, state: &str) -> Result<ForgePage, String> {
    if !(1..=1000).contains(&page) {
        return Err("Choose a valid page of provider results.".into());
    }
    if !matches!(state, "open" | "all") {
        return Err("Choose open or all provider items.".into());
    }
    let remote = ForgeRemote::from_repo(path)?;
    let token = remote.token()?;
    let project = remote.project_path();
    let github_state = if state == "all" { "all" } else { "open" };
    let gitlab_state = if state == "all" { "all" } else { "opened" };
    let endpoint = match (remote.provider, kind) {
        (Provider::GitHub, "reviews") => {
            format!("{project}/pulls?state={github_state}&per_page=50&page={page}")
        }
        (Provider::GitLab, "reviews") => {
            format!("{project}/merge_requests?state={gitlab_state}&per_page=50&page={page}")
        }
        (Provider::GitHub, "issues") => {
            format!("{project}/issues?state={github_state}&per_page=50&page={page}")
        }
        (Provider::GitLab, "issues") => {
            format!("{project}/issues?state={gitlab_state}&per_page=50&page={page}")
        }
        (Provider::GitHub, "ci") => {
            let sha = git::run(path, &["rev-parse", "HEAD"])?;
            format!(
                "{project}/commits/{}/check-runs?per_page=50&page={page}",
                sha.trim()
            )
        }
        (Provider::GitLab, "ci") => format!("{project}/pipelines?per_page=50&page={page}"),
        _ => return Err("Choose reviews, issues, or CI status.".into()),
    };
    let body = send(request(&remote, &token, "GET", &endpoint))?;
    let values = if kind == "ci" && remote.provider == Provider::GitHub {
        body["check_runs"].as_array()
    } else {
        body.as_array()
    }
    .ok_or_else(|| {
        format!(
            "{} returned an unexpected list format.",
            remote.provider.name()
        )
    })?;
    let has_more = values.len() == 50;
    let items = values
        .iter()
        .filter(|value| {
            !(kind == "issues"
                && remote.provider == Provider::GitHub
                && value.get("pull_request").is_some())
        })
        .filter_map(|value| {
            if kind == "ci" {
                let number = value["id"].as_i64()?;
                let url = field(
                    value,
                    if remote.provider == Provider::GitHub {
                        "html_url"
                    } else {
                        "web_url"
                    },
                );
                Some(ForgeItem {
                    number,
                    title: if remote.provider == Provider::GitHub {
                        field(value, "name")
                    } else {
                        field(value, "ref")
                    }
                    .to_owned(),
                    state: field(
                        value,
                        if remote.provider == Provider::GitHub {
                            "conclusion"
                        } else {
                            "status"
                        },
                    )
                    .to_owned(),
                    url: url.to_owned(),
                    detail: field(value, "status").to_owned(),
                })
            } else {
                item(value, remote.provider, kind)
            }
        })
        .collect();
    Ok(ForgePage { items, has_more })
}

fn update_issue_state_at(
    remote: &ForgeRemote,
    token: &str,
    number: i64,
    close: bool,
    base: &str,
) -> Result<(), String> {
    if number <= 0 {
        return Err("Choose a valid issue.".into());
    }
    let endpoint = format!("{}/issues/{number}", remote.project_path());
    let (method, payload) = match remote.provider {
        Provider::GitHub => (
            "PATCH",
            json!({"state": if close { "closed" } else { "open" }}),
        ),
        Provider::GitLab => (
            "PUT",
            json!({"state_event": if close { "close" } else { "reopen" }}),
        ),
    };
    send_at(
        request(remote, token, method, &endpoint).json(payload),
        base,
    )?;
    Ok(())
}

#[tauri::command]
pub fn forge_info(path: String) -> Result<ForgeInfo, String> {
    let remote = ForgeRemote::from_repo(&path)?;
    let connected = match remote.account_entry()?.get_password() {
        Ok(_) => true,
        Err(KeyringError::NoEntry) => false,
        Err(error) => return Err(format!("Could not read the system keychain: {error}")),
    };
    Ok(ForgeInfo {
        provider: remote.provider.name().to_owned(),
        host: remote.host.clone(),
        project: remote.project.clone(),
        connected,
        url: format!("https://{}/{}", remote.host, remote.project),
    })
}

#[tauri::command]
pub async fn forge_connect(path: String, token: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = ForgeRemote::from_repo(&path)?;
        connect_account(&remote, token)
    })
    .await
    .map_err(|error| format!("Account connection stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_connect_url(url: String, token: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = ForgeRemote::parse(&url)?;
        connect_account(&remote, token)
    })
    .await
    .map_err(|error| format!("Account connection stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_connect_url_as(
    url: String,
    provider: String,
    token: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = ForgeRemote::parse_with_provider(&url, Some(Provider::parse(&provider)?))?;
        connect_account(&remote, token)
    })
    .await
    .map_err(|error| format!("Account connection stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_connect_self_hosted(
    path: String,
    provider: String,
    token: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let origin = git::run(&path, &["config", "--get", "remote.origin.url"])
            .map_err(|_| "This repository has no origin remote. Add one before connecting.")?;
        let remote = ForgeRemote::parse_with_provider(&origin, Some(Provider::parse(&provider)?))?;
        connect_account(&remote, token)
    })
    .await
    .map_err(|error| format!("Account connection stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_connect_provider(provider: String, token: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = account_remote(Provider::parse(&provider)?);
        connect_account(&remote, token)
    })
    .await
    .map_err(|error| format!("Account connection stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub fn forge_accounts() -> Result<Vec<ForgeAccount>, String> {
    [Provider::GitHub, Provider::GitLab]
        .into_iter()
        .map(|provider| {
            let connected = match account_remote(provider).account_entry()?.get_password() {
                Ok(_) => true,
                Err(KeyringError::NoEntry) => false,
                Err(error) => return Err(format!("Could not read the system keychain: {error}")),
            };
            Ok(ForgeAccount {
                provider: provider.name().into(),
                connected,
            })
        })
        .collect()
}

#[tauri::command]
pub async fn forge_repositories(
    provider: String,
    page: u32,
) -> Result<Vec<ForgeRepository>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = account_remote(Provider::parse(&provider)?);
        list_repositories(&remote, page)
    })
    .await
    .map_err(|error| format!("Repository search stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_repositories_for_url(
    url: String,
    page: u32,
) -> Result<Vec<ForgeRepository>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = ForgeRemote::parse(&url)?;
        list_repositories(&remote, page)
    })
    .await
    .map_err(|error| format!("Repository search stopped unexpectedly: {error}"))?
}

fn list_repositories(remote: &ForgeRemote, page: u32) -> Result<Vec<ForgeRepository>, String> {
    if !(1..=1000).contains(&page) {
        return Err("Choose a valid repository page.".into());
    }
    let token = remote.token()?;
    let endpoint = match remote.provider {
        Provider::GitHub => format!("/user/repos?affiliation=owner%2Ccollaborator%2Corganization_member&sort=updated&per_page=50&page={page}"),
        Provider::GitLab => format!("/projects?membership=true&simple=true&order_by=last_activity_at&per_page=50&page={page}"),
    };
    let body = send(request(remote, &token, "GET", &endpoint))?;
    let values = body.as_array().ok_or_else(|| {
        format!(
            "{} returned an unexpected repository list.",
            remote.provider.name()
        )
    })?;
    Ok(values
        .iter()
        .filter_map(|value| repository(value, remote))
        .collect())
}

fn new_repository_request<'a>(
    remote: &'a ForgeRemote,
    token: &'a str,
    name: &str,
    description: &str,
    private: bool,
    namespace: Option<&str>,
    namespace_id: Option<u64>,
) -> Result<ForgeRequest<'a>, String> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 100
        || name.starts_with(['.', '-'])
        || name.ends_with(['.', '-'])
        || !name
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || b"._-".contains(&character))
    {
        return Err(
            "Enter a repository name of 1 to 100 letters, numbers, dots, dashes, or underscores."
                .into(),
        );
    }
    let description = description.trim();
    if description.len() > 500 || description.chars().any(char::is_control) {
        return Err(
            "Keep the repository description under 500 characters without line breaks.".into(),
        );
    }
    let (endpoint, mut payload) = match remote.provider {
        Provider::GitHub => (
            namespace
                .map(|organization| format!("/orgs/{organization}/repos"))
                .unwrap_or_else(|| "/user/repos".into()),
            json!({"name": name, "description": description, "private": private}),
        ),
        Provider::GitLab => (
            "/projects".into(),
            json!({"name": name, "description": description, "visibility": if private { "private" } else { "public" }}),
        ),
    };
    if let Some(id) = namespace_id {
        payload["namespace_id"] = json!(id);
    }
    Ok(request(remote, token, "POST", &endpoint).json(payload))
}

fn checked_namespace(provider: Provider, namespace: &str) -> Result<Option<String>, String> {
    let namespace = namespace.trim();
    if namespace.is_empty() {
        return Ok(None);
    }
    let segments: Vec<_> = namespace.split('/').collect();
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment.len() <= 100
            && !segment.starts_with(['.', '-'])
            && !segment.ends_with(['.', '-'])
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    };
    let valid = match provider {
        Provider::GitHub => {
            segments.len() == 1
                && namespace.len() <= 39
                && valid_segment(namespace)
                && namespace
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        }
        Provider::GitLab => {
            namespace.len() <= 255
                && segments.len() <= 10
                && segments.iter().all(|part| valid_segment(part))
        }
    };
    if !valid {
        return Err(match provider {
            Provider::GitHub => "Enter one GitHub organization name using letters, numbers, or dashes.",
            Provider::GitLab => "Enter a GitLab group path such as team/subgroup using letters, numbers, dots, dashes, or underscores.",
        }.into());
    }
    Ok(Some(namespace.to_owned()))
}

fn gitlab_namespace_id(remote: &ForgeRemote, token: &str, namespace: &str) -> Result<u64, String> {
    gitlab_namespace_id_at(remote, token, namespace, &remote.api_base())
}

fn gitlab_namespace_id_at(
    remote: &ForgeRemote,
    token: &str,
    namespace: &str,
    base: &str,
) -> Result<u64, String> {
    let encoded = utf8_percent_encode(namespace, NON_ALPHANUMERIC).to_string();
    let endpoint = format!("/namespaces/{encoded}");
    let value = send_at(request(remote, token, "GET", &endpoint), base)?;
    if value["full_path"].as_str() != Some(namespace) || value["kind"].as_str() != Some("group") {
        return Err("GitLab did not return that exact group namespace. Check the group path and your access.".into());
    }
    value["id"]
        .as_u64()
        .ok_or_else(|| "GitLab returned a group without a valid namespace ID.".into())
}

#[tauri::command]
pub async fn forge_create_repository(
    provider: String,
    server_url: Option<String>,
    name: String,
    description: String,
    private: bool,
    namespace: String,
) -> Result<ForgeRepository, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let provider = Provider::parse(&provider)?;
        let remote = match server_url.as_deref().filter(|url| !url.trim().is_empty()) {
            Some(url) => ForgeRemote::parse_with_provider(url, Some(provider))?,
            None => account_remote(provider),
        };
        let token = remote.token()?;
        let namespace = checked_namespace(provider, &namespace)?;
        let namespace_id = match (provider, namespace.as_deref()) {
            (Provider::GitLab, Some(group)) => Some(gitlab_namespace_id(&remote, &token, group)?),
            _ => None,
        };
        let response = send(new_repository_request(
            &remote,
            &token,
            &name,
            &description,
            private,
            namespace.as_deref(),
            namespace_id,
        )?)?;
        repository(&response, &remote).ok_or_else(|| {
            format!(
                "{} created the repository, but returned clone URLs Weave could not verify. Refresh the repository list before cloning.",
                remote.provider.name()
            )
        })
    })
    .await
    .map_err(|error| format!("Repository creation stopped unexpectedly: {error}"))?
}

/// Connects a cloud GitHub or GitLab account with a token from OAuth sign-in.
pub(crate) fn connect_provider_token(provider: &str, token: String) -> Result<(), String> {
    connect_account(&account_remote(Provider::parse(provider)?), token)
}

fn connect_account(remote: &ForgeRemote, token: String) -> Result<(), String> {
    let token = Zeroizing::new(token.trim().to_owned());
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return Err("Enter a valid personal access token without spaces.".into());
    }
    send(request(remote, &token, "GET", "/user"))?;
    remote
        .account_entry()?
        .set_password(&token)
        .map_err(|error| format!("Could not save the token in the system keychain: {error}"))?;
    remote.remember_provider()
}

#[tauri::command]
pub fn forge_disconnect(path: String) -> Result<(), String> {
    let remote = ForgeRemote::from_repo(&path)?;
    crate::forge_oauth::forget_refresh(&remote.host)?;
    match remote.account_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(format!("Could not remove the saved account token: {error}")),
    }
}

#[tauri::command]
pub async fn forge_list(
    path: String,
    kind: String,
    page: u32,
    state: String,
) -> Result<ForgePage, String> {
    tauri::async_runtime::spawn_blocking(move || list(&path, &kind, page, &state))
        .await
        .map_err(|error| format!("Provider request stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_issue_state(path: String, number: i64, close: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = ForgeRemote::from_repo(&path)?;
        let token = remote.token()?;
        update_issue_state_at(&remote, &token, number, close, &remote.api_base())
    })
    .await
    .map_err(|error| format!("Issue update stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_create(
    path: String,
    kind: String,
    title: String,
    description: String,
    source: Option<String>,
    target: Option<String>,
) -> Result<ForgeItem, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = ForgeRemote::from_repo(&path)?;
        let token = remote.token()?;
        let title = title.trim();
        if title.is_empty() {
            return Err("Enter a title.".into());
        }
        let project = remote.project_path();
        let (endpoint, payload) = match (remote.provider, kind.as_str()) {
            (Provider::GitHub, "reviews") | (Provider::GitLab, "reviews") => {
                let source = source.as_deref().unwrap_or("").trim();
                let target = target.as_deref().unwrap_or("").trim();
                if source.is_empty() || target.is_empty() || source == target {
                    return Err("Choose different source and target branches.".into());
                }
                match remote.provider {
                    Provider::GitHub => (format!("{project}/pulls"), json!({"title": title, "body": description, "head": source, "base": target})),
                    Provider::GitLab => (format!("{project}/merge_requests"), json!({"title": title, "description": description, "source_branch": source, "target_branch": target})),
                }
            }
            (Provider::GitHub, "issues") => (format!("{project}/issues"), json!({"title": title, "body": description})),
            (Provider::GitLab, "issues") => (format!("{project}/issues"), json!({"title": title, "description": description})),
            _ => return Err("Choose a pull or merge request, or an issue.".into()),
        };
        let body = send(request(&remote, &token, "POST", &endpoint).json(payload))?;
        item(&body, remote.provider, &kind)
            .ok_or_else(|| format!("{} created the item but returned an unexpected response.", remote.provider.name()))
    })
    .await
    .map_err(|error| format!("Provider request stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_review(path: String, number: i64) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if number <= 0 {
            return Err("Choose a valid pull or merge request.".into());
        }
        let remote = ForgeRemote::from_repo(&path)?;
        let token = remote.token()?;
        let endpoint = match remote.provider {
            Provider::GitHub => format!("{}/pulls/{number}/reviews", remote.project_path()),
            Provider::GitLab => {
                format!("{}/merge_requests/{number}/approve", remote.project_path())
            }
        };
        let builder = request(&remote, &token, "POST", &endpoint);
        let builder = if remote.provider == Provider::GitHub {
            builder.json(json!({"event": "APPROVE"}))
        } else {
            builder
        };
        send(builder)?;
        Ok(())
    })
    .await
    .map_err(|error| format!("Provider request stopped unexpectedly: {error}"))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeComment {
    pub id: i64,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeCommentsPage {
    pub items: Vec<ForgeComment>,
    pub has_more: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeReviewThread {
    pub id: String,
    pub provider_id: Option<String>,
    pub path: String,
    pub line: Option<i64>,
    pub side: String,
    pub resolved: bool,
    pub comments: Vec<ForgeComment>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeReviewThreadsPage {
    pub items: Vec<ForgeReviewThread>,
    pub has_more: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeReviewFile {
    pub old_path: String,
    pub new_path: String,
    pub patch: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeReviewFilesPage {
    pub items: Vec<ForgeReviewFile>,
    pub has_more: bool,
}

fn review_files_at(
    remote: &ForgeRemote,
    token: &str,
    number: i64,
    page: u32,
    base: &str,
) -> Result<ForgeReviewFilesPage, String> {
    if number <= 0 || !(1..=1000).contains(&page) {
        return Err("Choose a valid review request and file page.".into());
    }
    let endpoint = match remote.provider {
        Provider::GitHub => format!(
            "{}/pulls/{number}/files?per_page=100&page={page}",
            remote.project_path()
        ),
        Provider::GitLab => format!(
            "{}/merge_requests/{number}/diffs?per_page=100&page={page}",
            remote.project_path()
        ),
    };
    let response = send_at(request(remote, token, "GET", &endpoint), base)?;
    let values = response.as_array().ok_or_else(|| {
        format!(
            "{} returned changed files in an unexpected format.",
            remote.provider.name()
        )
    })?;
    let items = values
        .iter()
        .filter_map(|value| {
            let new_path = match remote.provider {
                Provider::GitHub => field(value, "filename"),
                Provider::GitLab => field(value, "new_path"),
            };
            if new_path.is_empty() {
                return None;
            }
            let old_path = match remote.provider {
                Provider::GitHub => value
                    .get("previous_filename")
                    .and_then(Value::as_str)
                    .unwrap_or(new_path),
                Provider::GitLab => value
                    .get("old_path")
                    .and_then(Value::as_str)
                    .unwrap_or(new_path),
            };
            Some(ForgeReviewFile {
                old_path: old_path.into(),
                new_path: new_path.into(),
                patch: match remote.provider {
                    Provider::GitHub => field(value, "patch"),
                    Provider::GitLab => field(value, "diff"),
                }
                .chars()
                .take(200_000)
                .collect(),
            })
        })
        .collect();
    Ok(ForgeReviewFilesPage {
        items,
        has_more: values.len() == 100,
    })
}

struct ReviewDraft<'a> {
    file: &'a ForgeReviewFile,
    line: i64,
    side: &'a str,
    other_line: Option<i64>,
    body: &'a str,
}

fn review_create_at(
    remote: &ForgeRemote,
    token: &str,
    number: i64,
    draft: ReviewDraft<'_>,
    details: &Value,
    base: &str,
) -> Result<(), String> {
    let ReviewDraft {
        file,
        line,
        side,
        other_line,
        body,
    } = draft;
    if number <= 0
        || line <= 0
        || line > 1_000_000
        || other_line.is_some_and(|value| value <= 0 || value > 1_000_000)
        || !matches!(side, "LEFT" | "RIGHT")
    {
        return Err("Choose a valid review request, side, and line number.".into());
    }
    let body = body.trim();
    if body.is_empty() || body.chars().count() > 100_000 {
        return Err("Write a review comment under 100,000 characters.".into());
    }
    if file.new_path.is_empty()
        || file.old_path.is_empty()
        || file.new_path.starts_with('/')
        || file.old_path.starts_with('/')
        || [file.new_path.as_str(), file.old_path.as_str()]
            .iter()
            .any(|path| path.split('/').any(|part| part == ".." || part == "."))
    {
        return Err("Choose a changed file from this review request.".into());
    }
    let endpoint = match remote.provider {
        Provider::GitHub => format!("{}/pulls/{number}/comments", remote.project_path()),
        Provider::GitLab => format!(
            "{}/merge_requests/{number}/discussions",
            remote.project_path()
        ),
    };
    let payload = match remote.provider {
        Provider::GitHub => {
            let commit = details.pointer("/head/sha").and_then(Value::as_str).filter(|value| !value.is_empty())
                .ok_or("GitHub did not provide the latest review commit. Refresh the request and retry.")?;
            json!({"body": body, "commit_id": commit, "path": file.new_path, "line": line, "side": side})
        }
        Provider::GitLab => {
            let refs = details.get("diff_refs").ok_or("GitLab has not prepared this merge request diff yet. Retry after the diff appears.")?;
            let sha = |name| {
                refs.get(name).and_then(Value::as_str).filter(|value| !value.is_empty())
                .ok_or_else(|| format!("GitLab did not provide {name} for this merge request diff. Refresh and retry."))
            };
            let mut position = json!({
                "base_sha": sha("base_sha")?, "start_sha": sha("start_sha")?, "head_sha": sha("head_sha")?,
                "position_type": "text", "old_path": file.old_path, "new_path": file.new_path,
            });
            position[if side == "RIGHT" {
                "new_line"
            } else {
                "old_line"
            }] = json!(line);
            if let Some(other) = other_line {
                position[if side == "RIGHT" {
                    "old_line"
                } else {
                    "new_line"
                }] = json!(other);
            }
            json!({"body": body, "position": position})
        }
    };
    send_at(
        request(remote, token, "POST", &endpoint).json(payload),
        base,
    )?;
    Ok(())
}

#[tauri::command]
pub async fn forge_review_files(
    path: String,
    number: i64,
    page: u32,
) -> Result<ForgeReviewFilesPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = ForgeRemote::from_repo(&path)?;
        let token = remote.token()?;
        review_files_at(&remote, &token, number, page, &remote.api_base())
    })
    .await
    .map_err(|error| format!("Changed files request stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_review_create(input: ForgeReviewCreateInput) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let ForgeReviewCreateInput {
            path,
            number,
            old_path,
            new_path,
            line,
            side,
            other_line,
            body,
        } = input;
        if number <= 0 || line <= 0 {
            return Err("Choose a valid review request and line number.".into());
        }
        let remote = ForgeRemote::from_repo(&path)?;
        let token = remote.token()?;
        let base = remote.api_base();
        let endpoint = match remote.provider {
            Provider::GitHub => format!("{}/pulls/{number}", remote.project_path()),
            Provider::GitLab => format!("{}/merge_requests/{number}", remote.project_path()),
        };
        let details = send_at(request(&remote, &token, "GET", &endpoint), &base)?;
        review_create_at(
            &remote,
            &token,
            number,
            ReviewDraft {
                file: &ForgeReviewFile {
                    old_path,
                    new_path,
                    patch: String::new(),
                },
                line,
                side: &side,
                other_line,
                body: &body,
            },
            &details,
            &base,
        )
    })
    .await
    .map_err(|error| format!("Review comment request stopped unexpectedly: {error}"))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeReviewCreateInput {
    path: String,
    number: i64,
    old_path: String,
    new_path: String,
    line: i64,
    side: String,
    other_line: Option<i64>,
    body: String,
}

fn review_comment(value: &Value, provider: Provider) -> Option<ForgeComment> {
    let id = value.get("id")?.as_i64()?;
    let author = match provider {
        Provider::GitHub => value.pointer("/user/login"),
        Provider::GitLab => value.pointer("/author/username"),
    }
    .and_then(Value::as_str)
    .unwrap_or("Unknown author")
    .to_owned();
    Some(ForgeComment {
        id,
        author,
        body: value.get("body")?.as_str()?.to_owned(),
        created_at: field(value, "created_at").to_owned(),
    })
}

fn review_threads_at(
    remote: &ForgeRemote,
    token: &str,
    number: i64,
    page: u32,
    base: &str,
) -> Result<ForgeReviewThreadsPage, String> {
    if number <= 0 || !(1..=1000).contains(&page) {
        return Err("Choose a valid review request and page.".into());
    }
    let endpoint = match remote.provider {
        Provider::GitHub => format!(
            "{}/pulls/{number}/comments?per_page=100&page={page}",
            remote.project_path()
        ),
        Provider::GitLab => format!(
            "{}/merge_requests/{number}/discussions?per_page=100&page={page}",
            remote.project_path()
        ),
    };
    let response = send_at(request(remote, token, "GET", &endpoint), base)?;
    let values = response.as_array().ok_or_else(|| {
        format!(
            "{} returned review threads in an unexpected format.",
            remote.provider.name()
        )
    })?;
    let mut items: Vec<ForgeReviewThread> = Vec::new();
    match remote.provider {
        Provider::GitHub => {
            let mut locations = HashMap::new();
            for value in values {
                let Some(comment) = review_comment(value, remote.provider) else {
                    continue;
                };
                let root_id = value["in_reply_to_id"].as_i64().unwrap_or(comment.id);
                let index = if let Some(index) = locations.get(&root_id) {
                    *index
                } else {
                    let index = items.len();
                    items.push(ForgeReviewThread {
                        id: root_id.to_string(),
                        provider_id: None,
                        path: field(value, "path").to_owned(),
                        line: value["line"]
                            .as_i64()
                            .or_else(|| value["original_line"].as_i64()),
                        side: field(value, "side").to_owned(),
                        resolved: false,
                        comments: Vec::new(),
                    });
                    locations.insert(root_id, index);
                    index
                };
                if comment.id == root_id {
                    items[index].path = field(value, "path").to_owned();
                    items[index].line = value["line"]
                        .as_i64()
                        .or_else(|| value["original_line"].as_i64());
                    items[index].side = field(value, "side").to_owned();
                }
                items[index].comments.push(comment);
            }
        }
        Provider::GitLab => {
            for value in values {
                let Some(position) =
                    value
                        .get("notes")
                        .and_then(Value::as_array)
                        .and_then(|notes| {
                            notes.iter().find_map(|note| {
                                note.get("position").filter(|value| value.is_object())
                            })
                        })
                else {
                    continue;
                };
                let comments: Vec<_> = value["notes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|note| review_comment(note, remote.provider))
                    .collect();
                if comments.is_empty() {
                    continue;
                }
                let id = field(value, "id");
                if id.is_empty() {
                    continue;
                }
                let new_line = position["new_line"].as_i64();
                let line = new_line.or_else(|| position["old_line"].as_i64());
                let side = if new_line.is_some() { "RIGHT" } else { "LEFT" };
                items.push(ForgeReviewThread {
                    id: id.to_owned(),
                    provider_id: None,
                    path: field(position, "new_path").to_owned(),
                    line,
                    side: side.into(),
                    resolved: value["notes"][0]["resolved"].as_bool().unwrap_or(false),
                    comments,
                });
            }
        }
    }
    Ok(ForgeReviewThreadsPage {
        items,
        has_more: values.len() == 100,
    })
}

fn github_graphql_data(value: Value) -> Result<Value, String> {
    if let Some(errors) = value.get("errors").and_then(Value::as_array) {
        let details = errors
            .iter()
            .filter_map(|error| error.get("message").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("GitHub review thread request failed: {details}"));
    }
    value
        .get("data")
        .cloned()
        .ok_or_else(|| "GitHub returned an incomplete review thread response.".into())
}

fn github_review_status_at(
    remote: &ForgeRemote,
    token: &str,
    number: i64,
    items: &mut [ForgeReviewThread],
    base: &str,
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let (owner, name) = remote
        .project
        .split_once('/')
        .ok_or("The GitHub repository name is incomplete.")?;
    let mut cursor: Option<String> = None;
    let mut visited = HashSet::new();
    for _ in 0..100 {
        let response = send_at(
            request(remote, token, "POST", "/graphql").json(json!({
                "query": "query($owner: String!, $name: String!, $number: Int!, $after: String) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { reviewThreads(first: 100, after: $after) { nodes { id isResolved comments(first: 1) { nodes { fullDatabaseId } } } pageInfo { hasNextPage endCursor } } } } }",
                "variables": {"owner": owner, "name": name, "number": number, "after": cursor}
            })),
            base,
        )?;
        let data = github_graphql_data(response)?;
        let threads = data
            .pointer("/repository/pullRequest/reviewThreads")
            .ok_or("GitHub did not return review threads for this pull request.")?;
        let nodes = threads["nodes"]
            .as_array()
            .ok_or("GitHub returned review threads in an unexpected format.")?;
        for node in nodes {
            let root = &node["comments"]["nodes"][0]["fullDatabaseId"];
            let root_id = root
                .as_str()
                .map(str::to_owned)
                .or_else(|| root.as_i64().map(|id| id.to_string()));
            let Some(item) = items
                .iter_mut()
                .find(|item| Some(&item.id) == root_id.as_ref())
            else {
                continue;
            };
            item.provider_id = node["id"].as_str().map(str::to_owned);
            item.resolved = node["isResolved"].as_bool().unwrap_or(false);
        }
        if items.iter().all(|item| item.provider_id.is_some()) {
            return Ok(());
        }
        if threads["pageInfo"]["hasNextPage"] != true {
            return Ok(());
        }
        let next = threads["pageInfo"]["endCursor"]
            .as_str()
            .ok_or("GitHub did not provide the next review thread page.")?;
        if !visited.insert(next.to_owned()) {
            return Err("GitHub repeated a review thread page. Try again later.".into());
        }
        cursor = Some(next.to_owned());
    }
    Err("This pull request has too many review threads to load at once.".into())
}

fn review_threads_with_status_at(
    remote: &ForgeRemote,
    token: &str,
    number: i64,
    page: u32,
    rest_base: &str,
    graphql_base: &str,
) -> Result<ForgeReviewThreadsPage, String> {
    let mut threads = review_threads_at(remote, token, number, page, rest_base)?;
    if remote.provider == Provider::GitHub {
        github_review_status_at(remote, token, number, &mut threads.items, graphql_base)?;
    }
    Ok(threads)
}

fn review_reply_at(
    remote: &ForgeRemote,
    token: &str,
    number: i64,
    thread_id: &str,
    body: &str,
    base: &str,
) -> Result<(), String> {
    if number <= 0 {
        return Err("Choose a valid review request.".into());
    }
    let body = body.trim();
    if body.is_empty() || body.chars().count() > 100_000 {
        return Err("Write a reply under 100,000 characters.".into());
    }
    if thread_id.is_empty() || thread_id.len() > 200 {
        return Err("Choose a valid review thread.".into());
    }
    let endpoint = match remote.provider {
        Provider::GitHub => {
            let id = thread_id
                .parse::<i64>()
                .map_err(|_| "Choose a valid GitHub review comment.")?;
            if id <= 0 {
                return Err("Choose a valid GitHub review comment.".into());
            }
            format!(
                "{}/pulls/{number}/comments/{id}/replies",
                remote.project_path()
            )
        }
        Provider::GitLab => format!(
            "{}/merge_requests/{number}/discussions/{}/notes",
            remote.project_path(),
            utf8_percent_encode(thread_id, NON_ALPHANUMERIC)
        ),
    };
    send_at(
        request(remote, token, "POST", &endpoint).json(json!({"body": body})),
        base,
    )?;
    Ok(())
}

fn review_resolve_at(
    remote: &ForgeRemote,
    token: &str,
    number: i64,
    thread_id: &str,
    resolved: bool,
    base: &str,
) -> Result<(), String> {
    if number <= 0 || thread_id.is_empty() || thread_id.len() > 200 {
        return Err("Choose a valid review thread.".into());
    }
    if remote.provider == Provider::GitHub {
        let mutation = if resolved {
            "mutation($threadId: ID!) { resolveReviewThread(input: {threadId: $threadId}) { thread { id isResolved } } }"
        } else {
            "mutation($threadId: ID!) { unresolveReviewThread(input: {threadId: $threadId}) { thread { id isResolved } } }"
        };
        let response = send_at(
            request(remote, token, "POST", "/graphql")
                .json(json!({"query": mutation, "variables": {"threadId": thread_id}})),
            base,
        )?;
        let data = github_graphql_data(response)?;
        let mutation_name = if resolved {
            "resolveReviewThread"
        } else {
            "unresolveReviewThread"
        };
        if data[mutation_name]["thread"]["isResolved"].as_bool() != Some(resolved) {
            return Err("GitHub did not confirm the review thread change.".into());
        }
        return Ok(());
    }
    let endpoint = format!(
        "{}/merge_requests/{number}/discussions/{}",
        remote.project_path(),
        utf8_percent_encode(thread_id, NON_ALPHANUMERIC)
    );
    send_at(
        request(remote, token, "PUT", &endpoint).json(json!({"resolved": resolved})),
        base,
    )?;
    Ok(())
}

#[tauri::command]
pub async fn forge_review_threads(
    path: String,
    number: i64,
    page: u32,
) -> Result<ForgeReviewThreadsPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = ForgeRemote::from_repo(&path)?;
        let token = remote.token()?;
        review_threads_with_status_at(
            &remote,
            &token,
            number,
            page,
            &remote.api_base(),
            &remote.graphql_base(),
        )
    })
    .await
    .map_err(|error| format!("Review threads request stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_review_reply(
    path: String,
    number: i64,
    thread_id: String,
    body: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = ForgeRemote::from_repo(&path)?;
        let token = remote.token()?;
        review_reply_at(
            &remote,
            &token,
            number,
            &thread_id,
            &body,
            &remote.api_base(),
        )
    })
    .await
    .map_err(|error| format!("Review reply stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_review_resolve(
    path: String,
    number: i64,
    thread_id: String,
    resolved: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = ForgeRemote::from_repo(&path)?;
        let token = remote.token()?;
        let base = if remote.provider == Provider::GitHub {
            remote.graphql_base()
        } else {
            remote.api_base()
        };
        review_resolve_at(&remote, &token, number, &thread_id, resolved, &base)
    })
    .await
    .map_err(|error| format!("Thread update stopped unexpectedly: {error}"))?
}

fn comment_endpoint(remote: &ForgeRemote, kind: &str, number: i64) -> Result<String, String> {
    if number <= 0 {
        return Err("Choose a valid review request or issue.".into());
    }
    let project = remote.project_path();
    match (remote.provider, kind) {
        (Provider::GitHub, "reviews" | "issues") => {
            Ok(format!("{project}/issues/{number}/comments"))
        }
        (Provider::GitLab, "reviews") => Ok(format!("{project}/merge_requests/{number}/notes")),
        (Provider::GitLab, "issues") => Ok(format!("{project}/issues/{number}/notes")),
        _ => Err("Comments are available for review requests and issues only.".into()),
    }
}

fn comments_at(
    remote: &ForgeRemote,
    token: &str,
    kind: &str,
    number: i64,
    page: u32,
    base: &str,
) -> Result<ForgeCommentsPage, String> {
    if !(1..=1000).contains(&page) {
        return Err("Choose a comment page between 1 and 1000.".into());
    }
    let endpoint = format!(
        "{}?per_page=100&page={page}",
        comment_endpoint(remote, kind, number)?
    );
    let response = send_at(request(remote, token, "GET", &endpoint), base)?;
    let comments = response.as_array().ok_or_else(|| {
        format!(
            "{} returned comments in an unexpected format.",
            remote.provider.name()
        )
    })?;
    let items = comments
        .iter()
        .filter_map(|value| {
            let id = value.get("id")?.as_i64()?;
            let body = value.get("body")?.as_str()?.to_owned();
            let author = match remote.provider {
                Provider::GitHub => value.pointer("/user/login"),
                Provider::GitLab => value.pointer("/author/username"),
            }
            .and_then(Value::as_str)
            .unwrap_or("Unknown author")
            .to_owned();
            let created_at = value
                .get("created_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            Some(ForgeComment {
                id,
                author,
                body,
                created_at,
            })
        })
        .collect();
    Ok(ForgeCommentsPage {
        items,
        has_more: comments.len() == 100,
    })
}

fn comment_create_at(
    remote: &ForgeRemote,
    token: &str,
    kind: &str,
    number: i64,
    body: &str,
    base: &str,
) -> Result<(), String> {
    let body = body.trim();
    if body.is_empty() {
        return Err("Write a comment before posting it.".into());
    }
    if body.chars().count() > 100_000 {
        return Err("Comment is too long. Keep it under 100,000 characters.".into());
    }
    let endpoint = comment_endpoint(remote, kind, number)?;
    send_at(
        request(remote, token, "POST", &endpoint).json(json!({"body": body})),
        base,
    )?;
    Ok(())
}

#[tauri::command]
pub async fn forge_comments(
    path: String,
    kind: String,
    number: i64,
    page: u32,
) -> Result<ForgeCommentsPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = ForgeRemote::from_repo(&path)?;
        let token = remote.token()?;
        comments_at(&remote, &token, &kind, number, page, &remote.api_base())
    })
    .await
    .map_err(|error| format!("Comments request stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_comment_create(
    path: String,
    kind: String,
    number: i64,
    body: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = ForgeRemote::from_repo(&path)?;
        let token = remote.token()?;
        comment_create_at(&remote, &token, &kind, number, &body, &remote.api_base())
    })
    .await
    .map_err(|error| format!("Comment request stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn forge_merge(path: String, number: i64, confirmed: bool) -> Result<(), String> {
    if !confirmed {
        return Err("Confirm that you want to merge this request.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        if number <= 0 {
            return Err("Choose a valid pull or merge request.".into());
        }
        let remote = ForgeRemote::from_repo(&path)?;
        let token = remote.token()?;
        let endpoint = match remote.provider {
            Provider::GitHub => format!("{}/pulls/{number}/merge", remote.project_path()),
            Provider::GitLab => format!("{}/merge_requests/{number}/merge", remote.project_path()),
        };
        send(request(&remote, &token, "PUT", &endpoint))?;
        Ok(())
    })
    .await
    .map_err(|error| format!("Provider request stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn parses_https_and_ssh_remotes() {
        for remote in [
            "https://github.com/octo/tool.git",
            "git@github.com:octo/tool.git",
            "ssh://git@github.com/octo/tool.git",
        ] {
            let parsed = ForgeRemote::parse(remote).unwrap();
            assert_eq!(parsed.provider, Provider::GitHub);
            assert_eq!(parsed.project, "octo/tool");
            assert_eq!(parsed.project_path(), "/repos/octo/tool");
        }
        let parsed = ForgeRemote::parse("git@gitlab.com:group/team/tool.git").unwrap();
        assert_eq!(parsed.provider, Provider::GitLab);
        assert_eq!(parsed.project_path(), "/projects/group%2Fteam%2Ftool");
    }

    #[test]
    fn parses_self_hosted_remotes_with_explicit_provider() {
        let github = ForgeRemote::parse_with_provider(
            "https://code.example.com/team/tool.git",
            Some(Provider::GitHub),
        )
        .unwrap();
        assert_eq!(github.host, "code.example.com");
        assert_eq!(github.project_path(), "/repos/team/tool");
        assert_eq!(github.api_base(), "https://code.example.com/api/v3");
        let gitlab = ForgeRemote::parse_with_provider(
            "git@git.example.com:group/team/tool.git",
            Some(Provider::GitLab),
        )
        .unwrap();
        assert_eq!(gitlab.project_path(), "/projects/group%2Fteam%2Ftool");
        assert_eq!(gitlab.api_base(), "https://git.example.com/api/v4");
        assert!(ForgeRemote::parse_with_provider(
            "https://github.com/team/tool.git",
            Some(Provider::GitLab)
        )
        .is_err());
    }

    #[test]
    fn rejects_other_hosts_and_embedded_credentials() {
        for remote in [
            "https://github.com.evil.test/a/b",
            "https://token@github.com/a/b",
            "http://github.com/a/b",
            "git@gitlab.com:../other",
            "https://gitlab.com/a",
        ] {
            assert!(ForgeRemote::parse(remote).is_err(), "{remote}");
        }
    }

    #[test]
    fn normalizes_provider_items() {
        let github = json!({"number": 12, "title": "Improve UI", "state": "open", "html_url": "https://github.com/a/b/pull/12", "head": {"ref": "feature"}, "base": {"ref": "main"}});
        let gitlab = json!({"iid": 4, "title": "Improve UI", "state": "opened", "web_url": "https://gitlab.com/a/b/-/merge_requests/4", "source_branch": "feature", "target_branch": "main"});
        assert_eq!(
            item(&github, Provider::GitHub, "reviews").unwrap().detail,
            "feature → main"
        );
        assert_eq!(
            item(&gitlab, Provider::GitLab, "reviews").unwrap().number,
            4
        );
    }

    #[test]
    fn explains_provider_validation_errors() {
        let message = provider_message(
            &json!({"message": {"source_branch": ["does not exist"], "target_branch": ["is missing"]}}),
        );
        assert!(message.contains("source_branch: does not exist"));
        assert!(message.contains("target_branch: is missing"));
    }

    #[test]
    fn normalizes_github_and_gitlab_repository_choices() {
        let github_account = account_remote(Provider::GitHub);
        let gitlab_account = account_remote(Provider::GitLab);
        let github = json!({"full_name": "team/project", "clone_url": "https://github.com/team/project.git", "ssh_url": "git@github.com:team/project.git", "private": true});
        let gitlab = json!({"path_with_namespace": "group/sub/project", "http_url_to_repo": "https://gitlab.com/group/sub/project.git", "ssh_url_to_repo": "git@gitlab.com:group/sub/project.git", "visibility": "private"});
        assert_eq!(
            repository(&github, &github_account).unwrap().name,
            "team/project"
        );
        assert!(repository(&github, &github_account).unwrap().private);
        assert_eq!(
            repository(&gitlab, &gitlab_account).unwrap().name,
            "group/sub/project"
        );
        assert!(repository(&gitlab, &gitlab_account).unwrap().private);
        let malicious = json!({"full_name": "team/project", "clone_url": "https://github.com.evil.test/team/project.git", "ssh_url": "git@github.com:team/project.git"});
        assert!(repository(&malicious, &github_account).is_none());
        let enterprise = ForgeRemote::parse_with_provider(
            "https://code.example.com/team/project.git",
            Some(Provider::GitHub),
        )
        .unwrap();
        let wrong_server = json!({"full_name": "team/project", "clone_url": "https://elsewhere.example.com/team/project.git", "ssh_url": "git@elsewhere.example.com:team/project.git"});
        assert!(repository(&wrong_server, &enterprise).is_none());
        let same_server = json!({"full_name": "team/project", "clone_url": "https://code.example.com/team/project.git", "ssh_url": "git@code.example.com:team/project.git"});
        assert!(repository(&same_server, &enterprise).is_some());
    }

    #[test]
    fn askpass_returns_only_requested_credential() {
        assert_eq!(
            askpass_answer(
                "Username for 'https://github.com':",
                "github.com",
                "user",
                "secret"
            ),
            "user"
        );
        assert_eq!(
            askpass_answer(
                "Password for 'https://gitlab.com':",
                "gitlab.com",
                "user",
                "secret"
            ),
            "secret"
        );
        assert_eq!(
            askpass_answer(
                "Password for 'https://github.com.evil.test':",
                "github.com",
                "user",
                "secret"
            ),
            ""
        );
        assert_eq!(
            askpass_answer("Other prompt", "github.com", "user", "secret"),
            ""
        );
        assert_eq!(
            askpass_answer(
                "Password for 'https://code.example.com/team/tool.git':",
                "code.example.com",
                "user",
                "secret"
            ),
            "secret"
        );
        assert_eq!(
            askpass_answer(
                "Password for 'https://code.example.com.evil.test/team/tool.git':",
                "code.example.com",
                "user",
                "secret"
            ),
            ""
        );
    }

    fn mock_server<T>(
        response: &str,
        action: impl FnOnce(&str) -> Result<T, String>,
    ) -> (Result<T, String>, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let response = response.to_owned();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut received = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                received.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&received);
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|value| value.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        break;
                    }
                }
            }
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8(received).unwrap()
        });
        let result = action(&base);
        (result, server.join().unwrap())
    }

    fn mock_request(
        provider: Provider,
        method: &'static str,
        payload: Option<Value>,
        response: &str,
    ) -> (Result<Value, String>, String) {
        mock_server(response, |base| {
            let remote = ForgeRemote {
                provider,
                host: match provider {
                    Provider::GitHub => "github.com",
                    Provider::GitLab => "gitlab.com",
                }
                .into(),
                project: "group/tool".into(),
            };
            let mut request = request(&remote, "secret-token", method, "/example");
            request.payload = payload;
            send_at(request, base)
        })
    }

    #[test]
    fn github_request_sends_bearer_auth_and_json_body() {
        let payload = json!({"title": "A \"quoted\" title", "body": "one\ntwo"});
        let (result, received) = mock_request(
            Provider::GitHub, "POST", Some(payload.clone()),
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}",
        );
        assert_eq!(result.unwrap()["ok"], true);
        assert!(received.starts_with("POST /example HTTP/1.1"));
        assert!(received.contains("Authorization: Bearer secret-token"));
        assert!(received.contains("X-GitHub-Api-Version: 2026-03-10"));
        assert!(received.ends_with(&payload.to_string()));
    }

    #[test]
    fn creates_personal_repositories_with_provider_visibility_fields() {
        for provider in [Provider::GitHub, Provider::GitLab] {
            let (response, endpoint) = match provider {
                Provider::GitHub => (
                    json!({"full_name":"owner/tool","clone_url":"https://github.com/owner/tool.git","ssh_url":"git@github.com:owner/tool.git","private":true}),
                    "/user/repos",
                ),
                Provider::GitLab => (
                    json!({"path_with_namespace":"owner/tool","http_url_to_repo":"https://gitlab.com/owner/tool.git","ssh_url_to_repo":"git@gitlab.com:owner/tool.git","visibility":"private"}),
                    "/projects",
                ),
            };
            let body = response.to_string();
            let http = format!(
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let (result, received) = mock_server(&http, |base| {
                let remote = account_remote(provider);
                let request = new_repository_request(
                    &remote,
                    "secret-token",
                    "tool",
                    "A useful repository",
                    true,
                    None,
                    None,
                )?;
                let result = send_at(request, base)?;
                repository(&result, &remote).ok_or("Invalid repository response".into())
            });
            assert_eq!(result.unwrap().name, "owner/tool");
            assert!(received.starts_with(&format!("POST {endpoint} HTTP/1.1")));
            assert!(received.contains("A useful repository"));
            if provider == Provider::GitHub {
                assert!(received.contains("\"private\":true"));
            } else {
                assert!(received.contains("\"visibility\":\"private\""));
            }
        }
        let remote = account_remote(Provider::GitHub);
        assert!(new_repository_request(&remote, "token", "../bad", "", true, None, None).is_err());
        assert!(
            new_repository_request(&remote, "token", "valid", "bad\ntext", true, None, None)
                .is_err()
        );
    }

    #[test]
    fn creates_repository_request_for_self_hosted_gitlab() {
        let body = json!({
            "path_with_namespace": "person/tool",
            "http_url_to_repo": "https://code.example.com/person/tool.git",
            "ssh_url_to_repo": "git@code.example.com:person/tool.git",
            "visibility": "public"
        })
        .to_string();
        let http = format!(
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (result, received) = mock_server(&http, |base| {
            let remote = ForgeRemote::parse_with_provider(
                "https://code.example.com/person/existing.git",
                Some(Provider::GitLab),
            )?;
            let request = new_repository_request(&remote, "token", "tool", "", false, None, None)?;
            let result = send_at(request, base)?;
            repository(&result, &remote).ok_or("Invalid repository response".into())
        });
        assert_eq!(result.unwrap().name, "person/tool");
        assert!(received.starts_with("POST /projects HTTP/1.1"));
        assert!(received.contains("\"visibility\":\"public\""));
    }

    #[test]
    fn creates_repositories_in_organizations_and_groups() {
        let github = account_remote(Provider::GitHub);
        let organization = checked_namespace(Provider::GitHub, "acme-team")
            .unwrap()
            .unwrap();
        let request = new_repository_request(
            &github,
            "token",
            "tool",
            "",
            true,
            Some(&organization),
            None,
        )
        .unwrap();
        assert_eq!(request.endpoint, "/orgs/acme-team/repos");
        assert!(checked_namespace(Provider::GitHub, "team/subgroup").is_err());
        assert!(checked_namespace(Provider::GitHub, "-bad").is_err());

        let gitlab = account_remote(Provider::GitLab);
        let group = checked_namespace(Provider::GitLab, "team/subgroup")
            .unwrap()
            .unwrap();
        let body = json!({"id": 42, "kind": "group", "full_path": group}).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (resolved, received) = mock_server(&response, |base| {
            gitlab_namespace_id_at(&gitlab, "token", &group, base)
        });
        assert_eq!(resolved.unwrap(), 42);
        assert!(received.starts_with("GET /namespaces/team%2Fsubgroup HTTP/1.1"));
        let request =
            new_repository_request(&gitlab, "token", "tool", "", false, Some(&group), Some(42))
                .unwrap();
        assert_eq!(request.payload.unwrap()["namespace_id"], 42);
        assert!(checked_namespace(Provider::GitLab, "../other").is_err());
        let wrong = json!({"id": 42, "kind": "user", "full_path": group}).to_string();
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{wrong}", wrong.len());
        let (resolved, _) = mock_server(&response, |base| {
            gitlab_namespace_id_at(&gitlab, "token", &group, base)
        });
        assert!(resolved.unwrap_err().contains("exact group namespace"));
    }

    #[test]
    fn self_hosted_github_request_uses_its_host_without_cloud_version_header() {
        let (result, received) = mock_server(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            |base| {
                let remote = ForgeRemote::parse_with_provider(
                    "https://code.example.com/team/tool.git",
                    Some(Provider::GitHub),
                )?;
                send_at(request(&remote, "secret-token", "GET", "/user"), base)
            },
        );
        result.unwrap();
        assert!(received.starts_with("GET /user HTTP/1.1"));
        assert!(received.contains("Authorization: Bearer secret-token"));
        assert!(!received.contains("X-GitHub-Api-Version"));
    }

    #[test]
    fn gitlab_request_sends_private_token_and_clear_error() {
        let (result, received) = mock_request(
            Provider::GitLab, "GET", None,
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: 21\r\n\r\n{\"message\":\"invalid\"}",
        );
        let error = result.unwrap_err();
        assert!(error.contains("invalid or expired"));
        assert!(!error.contains("secret-token"));
        assert!(received.contains("PRIVATE-TOKEN: secret-token"));
    }

    #[test]
    fn github_issue_close_uses_issue_endpoint() {
        let (result, received) = mock_server(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            |base| {
                let remote = ForgeRemote {
                    provider: Provider::GitHub,
                    host: "github.com".into(),
                    project: "team/tool".into(),
                };
                update_issue_state_at(&remote, "secret-token", 42, true, base)
            },
        );
        result.unwrap();
        assert!(received.starts_with("PATCH /repos/team/tool/issues/42 HTTP/1.1"));
        assert!(received.ends_with("{\"state\":\"closed\"}"));
    }

    #[test]
    fn gitlab_issue_reopen_uses_project_endpoint() {
        let (result, received) = mock_server(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            |base| {
                let remote = ForgeRemote {
                    provider: Provider::GitLab,
                    host: "gitlab.com".into(),
                    project: "group/team/tool".into(),
                };
                update_issue_state_at(&remote, "secret-token", 37, false, base)
            },
        );
        result.unwrap();
        assert!(received.starts_with("PUT /projects/group%2Fteam%2Ftool/issues/37 HTTP/1.1"));
        assert!(received.ends_with("{\"state_event\":\"reopen\"}"));
    }

    #[test]
    fn github_pull_request_discussion_uses_issue_comments_endpoint() {
        let body = r#"[{"id":7,"body":"Looks good","user":{"login":"alice"},"created_at":"2026-10-01T10:00:00Z"}]"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (result, received) = mock_server(&response, |base| {
            let remote = ForgeRemote {
                provider: Provider::GitHub,
                host: "github.com".into(),
                project: "team/tool".into(),
            };
            comments_at(&remote, "secret-token", "reviews", 42, 1, base)
        });
        let comments = result.unwrap();
        assert_eq!(comments.items[0].author, "alice");
        assert_eq!(comments.items[0].body, "Looks good");
        assert!(received
            .starts_with("GET /repos/team/tool/issues/42/comments?per_page=100&page=1 HTTP/1.1"));
    }

    #[test]
    fn gitlab_merge_request_comment_posts_to_notes() {
        let (result, received) = mock_server(
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            |base| {
                let remote = ForgeRemote {
                    provider: Provider::GitLab,
                    host: "gitlab.com".into(),
                    project: "group/tool".into(),
                };
                comment_create_at(
                    &remote,
                    "secret-token",
                    "reviews",
                    12,
                    "Please test this",
                    base,
                )
            },
        );
        result.unwrap();
        assert!(
            received.starts_with("POST /projects/group%2Ftool/merge_requests/12/notes HTTP/1.1")
        );
        assert!(received.ends_with("{\"body\":\"Please test this\"}"));
    }

    #[test]
    fn github_review_threads_group_replies_and_use_the_review_endpoint() {
        let body = r#"[{"id":11,"path":"src/main.rs","line":8,"side":"RIGHT","body":"Change this","user":{"login":"alice"}},{"id":12,"in_reply_to_id":11,"path":"src/main.rs","body":"Done","user":{"login":"bob"}}]"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let remote = ForgeRemote {
            provider: Provider::GitHub,
            host: "github.com".into(),
            project: "team/tool".into(),
        };
        let (result, received) = mock_server(&response, |base| {
            review_threads_at(&remote, "secret-token", 42, 1, base)
        });
        let threads = result.unwrap();
        assert_eq!(threads.items.len(), 1);
        assert_eq!(threads.items[0].line, Some(8));
        assert_eq!(threads.items[0].comments.len(), 2);
        assert!(received
            .starts_with("GET /repos/team/tool/pulls/42/comments?per_page=100&page=1 HTTP/1.1"));
        let (result, received) = mock_server(
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            |base| review_reply_at(&remote, "secret-token", 42, "11", "Thanks", base),
        );
        result.unwrap();
        assert!(received.starts_with("POST /repos/team/tool/pulls/42/comments/11/replies HTTP/1.1"));
        assert!(received.ends_with("{\"body\":\"Thanks\"}"));
    }

    #[test]
    fn github_review_threads_get_resolution_state_and_graphql_thread_id() {
        let remote = ForgeRemote {
            provider: Provider::GitHub,
            host: "github.com".into(),
            project: "team/tool".into(),
        };
        let mut items = vec![ForgeReviewThread {
            id: "11".into(),
            provider_id: None,
            path: "src/main.rs".into(),
            line: Some(8),
            side: "RIGHT".into(),
            resolved: false,
            comments: Vec::new(),
        }];
        let body = r#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[{"id":"PRRT_abc","isResolved":true,"comments":{"nodes":[{"fullDatabaseId":"11"}]}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (result, received) = mock_server(&response, |base| {
            github_review_status_at(&remote, "secret-token", 42, &mut items, base)
        });
        result.unwrap();
        assert_eq!(items[0].provider_id.as_deref(), Some("PRRT_abc"));
        assert!(items[0].resolved);
        assert!(received.starts_with("POST /graphql HTTP/1.1"));
        assert!(received.contains("reviewThreads"));

        for (resolved, mutation) in [
            (true, "resolveReviewThread"),
            (false, "unresolveReviewThread"),
        ] {
            let body = json!({"data": {(mutation): {"thread": {"id": "PRRT_abc", "isResolved": resolved}}}}).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let (result, received) = mock_server(&response, |base| {
                review_resolve_at(&remote, "secret-token", 42, "PRRT_abc", resolved, base)
            });
            result.unwrap();
            assert!(received.contains(mutation));
            assert!(received.contains("PRRT_abc"));
        }
    }

    #[test]
    fn gitlab_review_threads_read_diff_notes_and_reply_to_discussions() {
        let body = r#"[{"id":"thread-abc","notes":[{"id":21,"body":"Check this","author":{"username":"alice"},"position":{"new_path":"src/lib.rs","new_line":19},"resolved":false},{"id":22,"body":"Done","author":{"username":"bob"}}]}]"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let remote = ForgeRemote {
            provider: Provider::GitLab,
            host: "gitlab.com".into(),
            project: "group/tool".into(),
        };
        let (result, received) = mock_server(&response, |base| {
            review_threads_at(&remote, "secret-token", 7, 1, base)
        });
        let threads = result.unwrap();
        assert_eq!(threads.items[0].path, "src/lib.rs");
        assert_eq!(threads.items[0].line, Some(19));
        assert_eq!(threads.items[0].comments.len(), 2);
        assert!(received.starts_with(
            "GET /projects/group%2Ftool/merge_requests/7/discussions?per_page=100&page=1 HTTP/1.1"
        ));
        let (result, received) = mock_server(
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            |base| review_reply_at(&remote, "secret-token", 7, "thread-abc", "Thanks", base),
        );
        result.unwrap();
        assert!(received.starts_with(
            "POST /projects/group%2Ftool/merge_requests/7/discussions/thread%2Dabc/notes HTTP/1.1"
        ));
        assert!(received.ends_with("{\"body\":\"Thanks\"}"));
        let (result, received) = mock_server(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            |base| review_resolve_at(&remote, "secret-token", 7, "thread-abc", true, base),
        );
        result.unwrap();
        assert!(received.starts_with(
            "PUT /projects/group%2Ftool/merge_requests/7/discussions/thread%2Dabc HTTP/1.1"
        ));
        assert!(received.ends_with("{\"resolved\":true}"));
    }

    #[test]
    fn github_lists_changed_files_and_creates_line_thread() {
        let remote = ForgeRemote {
            provider: Provider::GitHub,
            host: "github.com".into(),
            project: "team/tool".into(),
        };
        let files = r#"[{"filename":"src/new.rs","previous_filename":"src/old.rs"}]"#;
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{files}", files.len());
        let (result, received) = mock_server(&response, |base| {
            review_files_at(&remote, "token", 42, 1, base)
        });
        let file = result.unwrap().items.remove(0);
        assert_eq!(file.old_path, "src/old.rs");
        assert!(received
            .starts_with("GET /repos/team/tool/pulls/42/files?per_page=100&page=1 HTTP/1.1"));
        let (result, received) = mock_server(
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            |base| {
                review_create_at(
                    &remote,
                    "token",
                    42,
                    ReviewDraft {
                        file: &file,
                        line: 19,
                        side: "RIGHT",
                        other_line: None,
                        body: "Check this",
                    },
                    &json!({"head":{"sha":"abc123"}}),
                    base,
                )
            },
        );
        result.unwrap();
        assert!(received.starts_with("POST /repos/team/tool/pulls/42/comments HTTP/1.1"));
        assert!(received.contains("\"commit_id\":\"abc123\""));
        assert!(received.contains("\"line\":19"));
        assert!(received.contains("\"path\":\"src/new.rs\""));
    }

    #[test]
    fn gitlab_lists_changed_files_and_creates_diff_thread() {
        let remote = ForgeRemote {
            provider: Provider::GitLab,
            host: "gitlab.com".into(),
            project: "group/tool".into(),
        };
        let files = r#"[{"new_path":"src/new.rs","old_path":"src/old.rs"}]"#;
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{files}", files.len());
        let (result, received) = mock_server(&response, |base| {
            review_files_at(&remote, "token", 7, 1, base)
        });
        let file = result.unwrap().items.remove(0);
        assert_eq!(file.new_path, "src/new.rs");
        assert!(received.starts_with(
            "GET /projects/group%2Ftool/merge_requests/7/diffs?per_page=100&page=1 HTTP/1.1"
        ));
        let details =
            json!({"diff_refs":{"base_sha":"base", "start_sha":"start", "head_sha":"head"}});
        let (result, received) = mock_server(
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            |base| {
                review_create_at(
                    &remote,
                    "token",
                    7,
                    ReviewDraft {
                        file: &file,
                        line: 12,
                        side: "LEFT",
                        other_line: None,
                        body: "Check this",
                    },
                    &details,
                    base,
                )
            },
        );
        result.unwrap();
        assert!(received
            .starts_with("POST /projects/group%2Ftool/merge_requests/7/discussions HTTP/1.1"));
        assert!(received.contains("\"old_line\":12"));
        assert!(received.contains("\"base_sha\":\"base\""));
        assert!(received.contains("\"old_path\":\"src/old.rs\""));
        assert!(!received.contains("\"new_line\""));
        let (result, received) = mock_server(
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            |base| {
                review_create_at(
                    &remote,
                    "token",
                    7,
                    ReviewDraft {
                        file: &file,
                        line: 20,
                        side: "RIGHT",
                        other_line: Some(18),
                        body: "Context line",
                    },
                    &details,
                    base,
                )
            },
        );
        result.unwrap();
        assert!(received.contains("\"new_line\":20"));
        assert!(received.contains("\"old_line\":18"));
    }
}
