use crate::{bitbucket_server::Server, git};
use keyring::{Entry, Error as KeyringError};
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;
use url::Url;
use zeroize::Zeroizing;

const KEYCHAIN_SERVICE: &str = "app.weave.desktop";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    Bitbucket,
    Azure,
}

impl Provider {
    fn label(self) -> &'static str {
        match self {
            Self::Bitbucket => "Bitbucket Cloud",
            Self::Azure => "Azure DevOps",
        }
    }
}

#[derive(Debug)]
struct Remote {
    provider: Provider,
    /// bitbucket.org, dev.azure.com, or an Azure DevOps Server host.
    host: String,
    organization: String,
    project: String,
    repository: String,
}

impl Remote {
    fn from_repo(path: &str) -> Result<Self, String> {
        let origin = git::run(path, &["config", "--get", "remote.origin.url"]).map_err(|_| {
            "This repository has no origin remote. Add a Bitbucket or Azure DevOps origin first."
        })?;
        Self::parse(origin.trim())
    }

    fn parse(origin: &str) -> Result<Self, String> {
        let (host, path) = if let Some(rest) = origin.strip_prefix("git@") {
            let (host, path) = rest
                .split_once(':')
                .ok_or("The SSH origin needs a server and repository path.")?;
            (host.to_string(), path.to_string())
        } else {
            let url = Url::parse(origin).map_err(|_| "The origin URL is invalid.")?;
            if !matches!(url.scheme(), "https" | "ssh")
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.port().is_some()
                || (url.scheme() == "https" && !url.username().is_empty())
            {
                return Err("Use a clean HTTPS or SSH origin URL without credentials, ports, or query text.".into());
            }
            (
                url.host_str().unwrap_or_default().to_string(),
                url.path().trim_start_matches('/').to_string(),
            )
        };
        let path = path.trim_end_matches(".git");
        let parts: Vec<&str> = path.split('/').collect();
        match (host.as_str(), parts.as_slice()) {
            ("bitbucket.org", [workspace, repository])
                if valid_segment(workspace) && valid_segment(repository) =>
            {
                Ok(Self {
                    provider: Provider::Bitbucket,
                    host: "bitbucket.org".into(),
                    organization: (*workspace).into(),
                    project: String::new(),
                    repository: (*repository).into(),
                })
            }
            ("dev.azure.com", [organization, project, "_git", repository])
            | ("ssh.dev.azure.com", ["v3", organization, project, repository])
                if valid_segment(organization) =>
            {
                let project = decode_azure_name(project)?;
                let repository = decode_azure_name(repository)?;
                Ok(Self {
                    provider: Provider::Azure,
                    host: "dev.azure.com".into(),
                    organization: (*organization).into(),
                    project,
                    repository,
                })
            }
            // Azure DevOps Server: https://host/collection[/more]/project/_git/repo
            (host, [collection @ .., project, "_git", repository])
                if !collection.is_empty()
                    && origin.starts_with("https://")
                    && !matches!(host, "bitbucket.org" | "dev.azure.com")
                    && collection.iter().all(|part| valid_segment(part)) =>
            {
                Ok(Self {
                    provider: Provider::Azure,
                    host: host.into(),
                    organization: collection.join("/"),
                    project: decode_azure_name(project)?,
                    repository: decode_azure_name(repository)?,
                })
            }
            _ => Err(
                "Use a Bitbucket Cloud, Azure DevOps or Azure DevOps Server origin URL for this integration."
                    .into(),
            ),
        }
    }

    fn keychain_entry(&self) -> Result<Entry, String> {
        Entry::new(
            KEYCHAIN_SERVICE,
            &format!("external:{}:{}", self.provider.label(), self.organization),
        )
        .map_err(|error| format!("Could not open the system keychain: {error}"))
    }

    fn credential(&self) -> Result<Credential, String> {
        let saved = self
            .keychain_entry()?
            .get_password()
            .map_err(|error| match error {
                KeyringError::NoEntry => format!(
                    "Connect {} before using its online tools.",
                    self.provider.label()
                ),
                _ => format!("Could not read the system keychain: {error}"),
            })?;
        let saved: Credential = serde_json::from_str(&saved)
            .map_err(|_| "The saved account is invalid. Disconnect and connect it again.")?;
        if !crate::account_oauth::is_oauth(&saved.token) {
            return Ok(saved);
        }
        let entry = self.keychain_entry()?;
        let token = crate::account_oauth::current_token(&saved.token, |next| {
            let renewed = Credential {
                email: saved.email.clone(),
                token: next.clone(),
            };
            let text = serde_json::to_string(&renewed).map_err(|error| error.to_string())?;
            entry
                .set_password(&text)
                .map_err(|error| format!("Could not save the renewed sign-in: {error}"))
        })?;
        Ok(Credential {
            email: OAUTH.into(),
            token,
        })
    }

    fn api_base(&self) -> String {
        match self.provider {
            Provider::Bitbucket => format!(
                "https://api.bitbucket.org/2.0/repositories/{}/{}",
                self.organization, self.repository
            ),
            Provider::Azure => format!(
                "{}/{}/_apis/git/repositories/{}",
                self.collection_base(),
                encode_segment(&self.project),
                encode_segment(&self.repository)
            ),
        }
    }

    /// Azure DevOps organization or Server collection URL.
    fn collection_base(&self) -> String {
        format!("https://{}/{}", self.host, self.organization)
    }

    fn endpoints(&self) -> Endpoints {
        match self.provider {
            Provider::Bitbucket => Endpoints {
                repo: self.api_base(),
                account: format!(
                    "https://api.bitbucket.org/2.0/repositories/{}",
                    self.organization
                ),
                collection: String::new(),
            },
            Provider::Azure => Endpoints {
                repo: self.api_base(),
                account: format!(
                    "{}/{}/_apis",
                    self.collection_base(),
                    encode_segment(&self.project)
                ),
                collection: self.collection_base(),
            },
        }
    }

    /// Azure DevOps Server 2020 and newer support API 6.0.
    fn api_version(&self) -> &'static str {
        if self.host == "dev.azure.com" {
            "7.1"
        } else {
            "6.0"
        }
    }

    fn web_url(&self) -> String {
        match self.provider {
            Provider::Bitbucket => format!(
                "https://bitbucket.org/{}/{}",
                self.organization, self.repository
            ),
            Provider::Azure => format!(
                "{}/{}/_git/{}",
                self.collection_base(),
                encode_segment(&self.project),
                encode_segment(&self.repository)
            ),
        }
    }
}

pub(crate) fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn decode_azure_name(value: &str) -> Result<String, String> {
    let decoded = percent_decode_str(value).decode_utf8().map_err(|_| {
        "The Azure DevOps origin has invalid text in its project or repository name."
    })?;
    if decoded.is_empty()
        || decoded.len() > 255
        || matches!(decoded.as_ref(), "." | "..")
        || decoded
            .chars()
            .any(|character| character.is_control() || "/\\?#".contains(character))
    {
        return Err("The Azure DevOps origin has an invalid project or repository name.".into());
    }
    Ok(decoded.into_owned())
}

fn encode_segment(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// API base URLs; tests point them at a local server.
struct Endpoints {
    repo: String,
    account: String,
    collection: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Credential {
    pub(crate) email: String,
    pub(crate) token: Zeroizing<String>,
}

pub(crate) fn client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("Could not prepare the provider connection: {error}"))
}

/// Marks a credential whose token is an OAuth access token.
const OAUTH: &str = "oauth";

fn request(client: &Client, method: Method, url: &str, credential: &Credential) -> RequestBuilder {
    let request = client
        .request(method, url)
        .header("Accept", "application/json");
    if credential.email == OAUTH {
        request.bearer_auth(credential.token.as_str())
    } else {
        request.basic_auth(&credential.email, Some(credential.token.as_str()))
    }
}

pub(crate) fn send(request: RequestBuilder, provider: &str) -> Result<Value, String> {
    let mut response = request
        .send()
        .map_err(|error| format!("Could not reach {provider}: {error}"))?;
    let status = response.status();
    let mut limited = (&mut response).take(2 * 1024 * 1024 + 1);
    let mut body = String::new();
    limited
        .read_to_string(&mut body)
        .map_err(|error| format!("Could not read {provider}'s response: {error}"))?;
    if body.len() > 2 * 1024 * 1024 {
        return Err(format!(
            "{provider} returned too much data. Narrow the request and try again."
        ));
    }
    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .pointer("/error/message")
                    .or_else(|| value.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| {
                status
                    .canonical_reason()
                    .unwrap_or("request failed")
                    .to_string()
            });
        return Err(format!("{provider} returned {}: {message}. Check the account token and its repository permissions.", status.as_u16()));
    }
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&body).map_err(|_| format!("{provider} returned invalid JSON."))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalInfo {
    pub(crate) provider: String,
    pub(crate) organization: String,
    pub(crate) repository: String,
    pub(crate) connected: bool,
    pub(crate) url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalItem {
    pub(crate) number: i64,
    pub(crate) title: String,
    pub(crate) state: String,
    pub(crate) url: String,
    pub(crate) detail: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalPage {
    pub(crate) items: Vec<ExternalItem>,
    pub(crate) has_more: bool,
}

pub(crate) fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn provider_link(value: &str, remote: &Remote) -> String {
    Url::parse(value)
        .ok()
        .filter(|url| {
            url.scheme() == "https"
                && url.host_str() == Some(remote.host.as_str())
                && url.username().is_empty()
                && url.password().is_none()
        })
        .map(|url| url.to_string())
        .unwrap_or_default()
}

fn list_at(
    remote: &Remote,
    credential: &Credential,
    kind: &str,
    state: &str,
    page: u32,
    api: &Endpoints,
) -> Result<ExternalPage, String> {
    let base = api.repo.as_str();
    if page == 0 || page > 1000 {
        return Err("Choose a page between 1 and 1000.".into());
    }
    if !matches!(kind, "reviews" | "ci") {
        return Err("Choose pull requests or build runs.".into());
    }
    if page > 30 && remote.provider == Provider::Azure && kind == "ci" {
        return Err(
            "Azure build history goes back 900 runs. Open Azure DevOps for older builds.".into(),
        );
    }
    let (bitbucket_state, azure_state) = match state {
        "open" | "" => ("&state=OPEN", "active"),
        "merged" => ("&state=MERGED", "completed"),
        "declined" => ("&state=DECLINED", "abandoned"),
        "all" => ("&state=OPEN&state=MERGED&state=DECLINED", "all"),
        _ => return Err("Choose open, merged, declined or all pull requests.".into()),
    };
    let version = remote.api_version();
    let client = client()?;
    let url = match (remote.provider, kind) {
        (Provider::Bitbucket, "reviews") => {
            format!("{base}/pullrequests?pagelen=30&page={page}{bitbucket_state}")
        }
        (Provider::Bitbucket, "ci") => {
            format!("{base}/pipelines?pagelen=30&page={page}&sort=-created_on")
        }
        (Provider::Azure, "reviews") => format!(
            "{base}/pullrequests?searchCriteria.status={azure_state}&%24top=30&%24skip={}&api-version={version}",
            (page - 1) * 30
        ),
        (Provider::Azure, "ci") => {
            let repo = send(
                request(
                    &client,
                    Method::GET,
                    &format!("{base}?api-version={version}"),
                    credential,
                ),
                remote.provider.label(),
            )?;
            let id = field(&repo, "id");
            if id.is_empty() {
                return Err(
                    "Azure DevOps did not return a repository ID for its build list.".into(),
                );
            }
            // The builds API pages with a header token, so ask for every run up
            // to this page and keep the last 30.
            format!(
                "{}/{}/_apis/build/builds?repositoryId={}&repositoryType=TfsGit&%24top={}&api-version={version}",
                api.collection,
                encode_segment(&remote.project),
                encode_segment(id),
                page * 30
            )
        }
        _ => unreachable!(),
    };
    let result = send(
        request(&client, Method::GET, &url, credential),
        remote.provider.label(),
    )?;
    let values = result
        .get("values")
        .or_else(|| result.get("value"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "{} returned a list in an unexpected format.",
                remote.provider.label()
            )
        })?;
    let skip = if remote.provider == Provider::Azure && kind == "ci" {
        ((page - 1) * 30) as usize
    } else {
        0
    };
    let items = values
        .iter()
        .skip(skip)
        .map(|value| match (remote.provider, kind) {
            (Provider::Bitbucket, "reviews") => ExternalItem {
                number: value["id"].as_i64().unwrap_or_default(),
                title: field(value, "title").into(),
                state: field(value, "state").into(),
                url: provider_link(
                    value
                        .pointer("/links/html/href")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                    remote,
                ),
                detail: value
                    .pointer("/author/display_name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
            },
            (Provider::Bitbucket, "ci") => ExternalItem {
                number: value["build_number"].as_i64().unwrap_or_default(),
                title: format!(
                    "Pipeline #{}",
                    value["build_number"].as_i64().unwrap_or_default()
                ),
                state: value
                    .pointer("/state/result/name")
                    .or_else(|| value.pointer("/state/name"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                url: provider_link(
                    value
                        .pointer("/links/html/href")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                    remote,
                ),
                detail: value
                    .pointer("/target/ref_name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
            },
            (Provider::Azure, "reviews") => ExternalItem {
                number: value["pullRequestId"].as_i64().unwrap_or_default(),
                title: field(value, "title").into(),
                state: field(value, "status").into(),
                url: format!(
                    "{}/pullrequest/{}",
                    remote.web_url(),
                    value["pullRequestId"].as_i64().unwrap_or_default()
                ),
                detail: value
                    .pointer("/createdBy/displayName")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
            },
            (Provider::Azure, "ci") => ExternalItem {
                number: value["id"].as_i64().unwrap_or_default(),
                title: format!("Build #{}", field(value, "buildNumber")),
                state: value
                    .get("result")
                    .or_else(|| value.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                url: provider_link(
                    value
                        .pointer("/_links/web/href")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                    remote,
                ),
                detail: value
                    .pointer("/sourceBranch")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
            },
            _ => unreachable!(),
        })
        .collect();
    let has_more = if remote.provider == Provider::Bitbucket {
        result.get("next").and_then(Value::as_str).is_some()
    } else if kind == "ci" {
        values.len() == (page * 30) as usize
    } else {
        values.len() == 30
    };
    Ok(ExternalPage { items, has_more })
}

#[tauri::command]
pub fn external_info(path: String) -> Result<ExternalInfo, String> {
    if let Some(server) = Server::from_repo(&path) {
        return server.info();
    }
    let remote = Remote::from_repo(&path)?;
    let connected = match remote.keychain_entry()?.get_password() {
        Ok(_) => true,
        Err(KeyringError::NoEntry) => false,
        Err(error) => return Err(format!("Could not read the system keychain: {error}")),
    };
    Ok(ExternalInfo {
        provider: remote.provider.label().into(),
        organization: remote.organization.clone(),
        repository: remote.repository.clone(),
        connected,
        url: remote.web_url(),
    })
}

#[tauri::command]
pub async fn external_connect(path: String, email: String, token: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(server) = Server::from_repo(&path) {
            return server.connect(&email, &token);
        }
        let remote = Remote::from_repo(&path)?;
        let email = email.trim().to_string();
        let token = Zeroizing::new(token.trim().to_string());
        if token.is_empty() || (remote.provider == Provider::Bitbucket && !email.contains('@')) {
            return Err(
                "Enter a token and, for Bitbucket Cloud, your Atlassian account email.".into(),
            );
        }
        let credential = Credential { email, token };
        let base = remote.api_base();
        let url = if remote.provider == Provider::Azure {
            format!("{base}?api-version={}", remote.api_version())
        } else {
            base
        };
        send(
            request(&client()?, Method::GET, &url, &credential),
            remote.provider.label(),
        )?;
        let saved = serde_json::to_string(&credential)
            .map_err(|error| format!("Could not prepare account credentials: {error}"))?;
        remote
            .keychain_entry()?
            .set_password(&saved)
            .map_err(|error| format!("Could not save the token in the system keychain: {error}"))
    })
    .await
    .map_err(|error| format!("Account connection stopped unexpectedly: {error}"))?
}

fn save_grant(remote: &Remote, grant: &crate::account_oauth::Grant) -> Result<(), String> {
    let credential = Credential {
        email: String::new(),
        token: grant.encode(),
    };
    let probe = Credential {
        email: OAUTH.into(),
        token: Zeroizing::new(grant.access.clone()),
    };
    let base = remote.api_base();
    let url = if remote.provider == Provider::Azure {
        format!("{base}?api-version={}", remote.api_version())
    } else {
        base
    };
    send(
        request(&client()?, Method::GET, &url, &probe),
        remote.provider.label(),
    )?;
    let saved = serde_json::to_string(&credential).map_err(|error| error.to_string())?;
    remote
        .keychain_entry()?
        .set_password(&saved)
        .map_err(|error| format!("Could not save the sign-in in the system keychain: {error}"))
}

/// Bitbucket Cloud sign-in through an OAuth consumer with the callback URL
/// http://localhost:53682/.
#[tauri::command]
pub async fn external_oauth_bitbucket(
    path: String,
    client_id: String,
    client_secret: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = Remote::from_repo(&path)?;
        if remote.provider != Provider::Bitbucket {
            return Err("This repository is not on Bitbucket Cloud.".into());
        }
        let (client_id, secret) = (client_id.trim(), client_secret.trim());
        crate::account_oauth::valid_client(client_id, Some(secret))?;
        let mut authorize = reqwest::Url::parse("https://bitbucket.org/site/oauth2/authorize")
            .map_err(|error| error.to_string())?;
        authorize
            .query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("response_type", "code");
        let grant = crate::account_oauth::browser_sign_in(
            authorize.as_str(),
            crate::account_oauth::bitbucket_grant(client_id, secret),
        )?;
        save_grant(&remote, &grant)
    })
    .await
    .map_err(|error| format!("Sign-in stopped unexpectedly: {error}"))?
}

/// Starts Microsoft sign-in for Azure DevOps (device code).
#[tauri::command]
pub async fn external_oauth_azure_start(
    client_id: String,
) -> Result<crate::forge_oauth::DeviceStart, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::forge_oauth::start_at(
            crate::forge_oauth::Service::Azure,
            client_id.trim(),
            crate::forge_oauth::Service::Azure.base(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// True once the Microsoft sign-in has finished and the account is saved.
#[tauri::command]
pub async fn external_oauth_azure_poll(
    path: String,
    id: String,
    client_id: String,
) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = Remote::from_repo(&path)?;
        if remote.provider != Provider::Azure || remote.host != "dev.azure.com" {
            return Err("Microsoft sign-in is for Azure DevOps Services (dev.azure.com). Use a personal access token for Azure DevOps Server.".into());
        }
        let Some((_, tokens)) = crate::forge_oauth::poll_at(&id, crate::forge_oauth::Service::Azure.base())? else {
            return Ok(false);
        };
        save_grant(&remote, &crate::account_oauth::microsoft_grant(client_id.trim(), &tokens))?;
        Ok(true)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn external_disconnect(path: String) -> Result<(), String> {
    if let Some(server) = Server::from_repo(&path) {
        return server.disconnect();
    }
    let remote = Remote::from_repo(&path)?;
    match remote.keychain_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(format!(
            "Could not remove the token from the system keychain: {error}"
        )),
    }
}

#[tauri::command]
pub async fn external_list(
    path: String,
    kind: String,
    page: u32,
    state: Option<String>,
) -> Result<ExternalPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(server) = Server::from_repo(&path) {
            return server.list(&kind, state.as_deref().unwrap_or("open"), page);
        }
        let remote = Remote::from_repo(&path)?;
        let credential = remote.credential()?;
        list_at(
            &remote,
            &credential,
            &kind,
            state.as_deref().unwrap_or("open"),
            page,
            &remote.endpoints(),
        )
    })
    .await
    .map_err(|error| format!("Provider request stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn external_create_review(
    path: String,
    title: String,
    description: String,
    source: String,
    target: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if title.trim().is_empty() || source == target {
            return Err("Enter a title and two different branch names.".into());
        }
        for branch in [&source, &target] {
            git::run(&path, &["check-ref-format", "--branch", branch])
                .map_err(|_| format!("{branch} is not a valid Git branch name."))?;
        }
        if let Some(server) = Server::from_repo(&path) {
            return server.create_review(&title, &description, &source, &target);
        }
        let remote = Remote::from_repo(&path)?;
        let credential = remote.credential()?;
        create_review_at(
            &remote,
            &credential,
            &title,
            &description,
            &source,
            &target,
            &remote.api_base(),
        )
    })
    .await
    .map_err(|error| format!("Creating the pull request stopped unexpectedly: {error}"))?
}

fn create_review_at(
    remote: &Remote,
    credential: &Credential,
    title: &str,
    description: &str,
    source: &str,
    target: &str,
    base: &str,
) -> Result<(), String> {
    let (url, body) = match remote.provider {
        Provider::Bitbucket => (
            format!("{base}/pullrequests"),
            json!({"title":title.trim(),"description":description,"source":{"branch":{"name":source}},"destination":{"branch":{"name":target}}}),
        ),
        Provider::Azure => (
            format!("{base}/pullrequests?api-version={}", remote.api_version()),
            json!({"sourceRefName":format!("refs/heads/{source}"),"targetRefName":format!("refs/heads/{target}"),"title":title.trim(),"description":description}),
        ),
    };
    send(
        request(&client()?, Method::POST, &url, credential).json(&body),
        remote.provider.label(),
    )?;
    Ok(())
}

#[tauri::command]
pub async fn external_merge(path: String, number: i64, confirmed: bool) -> Result<(), String> {
    if !confirmed {
        return Err("Confirm the pull request merge before continuing.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        if number <= 0 {
            return Err("Choose a valid pull request.".into());
        }
        if let Some(server) = Server::from_repo(&path) {
            return server.merge(number);
        }
        let remote = Remote::from_repo(&path)?;
        let credential = remote.credential()?;
        merge_at(&remote, &credential, number, &remote.api_base())
    })
    .await
    .map_err(|error| format!("Merging the pull request stopped unexpectedly: {error}"))?
}

fn merge_at(
    remote: &Remote,
    credential: &Credential,
    number: i64,
    base: &str,
) -> Result<(), String> {
    let client = client()?;
    match remote.provider {
        Provider::Bitbucket => {
            let url = format!("{base}/pullrequests/{number}/merge");
            send(
                request(&client, Method::POST, &url, credential),
                remote.provider.label(),
            )?;
        }
        Provider::Azure => {
            let url = format!(
                "{base}/pullrequests/{number}?api-version={}",
                remote.api_version()
            );
            let current = send(
                request(&client, Method::GET, &url, credential),
                remote.provider.label(),
            )?;
            let commit = current.pointer("/lastMergeSourceCommit/commitId").and_then(Value::as_str)
                .ok_or("Azure DevOps did not return the pull request source commit. Refresh and try again.")?;
            send(
                request(&client, Method::PATCH, &url, credential).json(
                    &json!({"status":"completed","lastMergeSourceCommit":{"commitId":commit}}),
                ),
                remote.provider.label(),
            )?;
        }
    }
    Ok(())
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalComment {
    /// Comment id, used as the parent of a reply.
    pub(crate) id: i64,
    /// Azure DevOps thread id; 0 for providers without threads.
    pub(crate) thread: i64,
    pub(crate) author: String,
    pub(crate) body: String,
    pub(crate) created: String,
    /// "path:line" for comments on a changed line.
    pub(crate) location: String,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalRepository {
    pub(crate) name: String,
    pub(crate) clone_url: String,
    pub(crate) web_url: String,
    pub(crate) private: bool,
}

fn check_number(number: i64) -> Result<(), String> {
    if number <= 0 {
        return Err("Choose a valid pull request.".into());
    }
    Ok(())
}

fn approve_at(
    remote: &Remote,
    credential: &Credential,
    number: i64,
    api: &Endpoints,
) -> Result<(), String> {
    check_number(number)?;
    let client = client()?;
    let label = remote.provider.label();
    match remote.provider {
        Provider::Bitbucket => {
            send(
                request(
                    &client,
                    Method::POST,
                    &format!("{}/pullrequests/{number}/approve", api.repo),
                    credential,
                ),
                label,
            )?;
        }
        Provider::Azure => {
            let version = remote.api_version();
            let me = send(
                request(
                    &client,
                    Method::GET,
                    &format!(
                        "{}/_apis/connectionData?api-version={version}",
                        api.collection
                    ),
                    credential,
                ),
                label,
            )?;
            let id = me
                .pointer("/authenticatedUser/id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or("Azure DevOps did not say which account the token belongs to.")?;
            let url = format!(
                "{}/pullrequests/{number}/reviewers/{}?api-version={version}",
                api.repo,
                encode_segment(id)
            );
            send(
                request(&client, Method::PUT, &url, credential).json(&json!({"vote": 10})),
                label,
            )?;
        }
    }
    Ok(())
}

fn comments_at(
    remote: &Remote,
    credential: &Credential,
    number: i64,
    api: &Endpoints,
) -> Result<Vec<ExternalComment>, String> {
    check_number(number)?;
    let client = client()?;
    let label = remote.provider.label();
    let mut comments = Vec::new();
    match remote.provider {
        Provider::Bitbucket => {
            let result = send(
                request(
                    &client,
                    Method::GET,
                    &format!("{}/pullrequests/{number}/comments?pagelen=100", api.repo),
                    credential,
                ),
                label,
            )?;
            for value in result["values"].as_array().into_iter().flatten() {
                if value["deleted"].as_bool() == Some(true) {
                    continue;
                }
                let location = match (
                    value.pointer("/inline/path").and_then(Value::as_str),
                    value.pointer("/inline/to").and_then(Value::as_i64),
                ) {
                    (Some(path), Some(line)) => format!("{path}:{line}"),
                    (Some(path), None) => path.into(),
                    _ => String::new(),
                };
                comments.push(ExternalComment {
                    id: value["id"].as_i64().unwrap_or_default(),
                    thread: 0,
                    author: value
                        .pointer("/user/display_name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                    body: value
                        .pointer("/content/raw")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                    created: field(value, "created_on").into(),
                    location,
                });
            }
        }
        Provider::Azure => {
            let url = format!(
                "{}/pullRequests/{number}/threads?api-version={}",
                api.repo,
                remote.api_version()
            );
            let result = send(request(&client, Method::GET, &url, credential), label)?;
            for thread in result["value"].as_array().into_iter().flatten() {
                if thread["isDeleted"].as_bool() == Some(true) {
                    continue;
                }
                let location = match (
                    thread
                        .pointer("/threadContext/filePath")
                        .and_then(Value::as_str),
                    thread
                        .pointer("/threadContext/rightFileStart/line")
                        .and_then(Value::as_i64),
                ) {
                    (Some(path), Some(line)) => format!("{}:{line}", path.trim_start_matches('/')),
                    (Some(path), None) => path.trim_start_matches('/').into(),
                    _ => String::new(),
                };
                for comment in thread["comments"].as_array().into_iter().flatten() {
                    if comment["commentType"] == "system"
                        || comment["isDeleted"].as_bool() == Some(true)
                    {
                        continue;
                    }
                    comments.push(ExternalComment {
                        id: comment["id"].as_i64().unwrap_or_default(),
                        thread: thread["id"].as_i64().unwrap_or_default(),
                        author: comment
                            .pointer("/author/displayName")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                        body: field(comment, "content").into(),
                        created: field(comment, "publishedDate").into(),
                        location: location.clone(),
                    });
                }
            }
        }
    }
    Ok(comments)
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyTarget {
    pub(crate) id: i64,
    pub(crate) thread: i64,
}

fn comment_at(
    remote: &Remote,
    credential: &Credential,
    number: i64,
    body: &str,
    reply_to: Option<ReplyTarget>,
    api: &Endpoints,
) -> Result<(), String> {
    check_number(number)?;
    let body = body.trim();
    if body.is_empty() || body.len() > 32_000 {
        return Err("Write a comment of up to 32,000 characters.".into());
    }
    let client = client()?;
    let (url, payload) = match (remote.provider, reply_to) {
        (Provider::Bitbucket, None) => (
            format!("{}/pullrequests/{number}/comments", api.repo),
            json!({"content": {"raw": body}}),
        ),
        (Provider::Bitbucket, Some(target)) => (
            format!("{}/pullrequests/{number}/comments", api.repo),
            json!({"content": {"raw": body}, "parent": {"id": target.id}}),
        ),
        (Provider::Azure, Some(target)) => (
            format!(
                "{}/pullRequests/{number}/threads/{}/comments?api-version={}",
                api.repo,
                target.thread,
                remote.api_version()
            ),
            json!({"parentCommentId": target.id, "content": body, "commentType": 1}),
        ),
        (Provider::Azure, None) => (
            format!(
                "{}/pullRequests/{number}/threads?api-version={}",
                api.repo,
                remote.api_version()
            ),
            json!({"comments": [{"parentCommentId": 0, "content": body, "commentType": 1}], "status": 1}),
        ),
    };
    send(
        request(&client, Method::POST, &url, credential).json(&payload),
        remote.provider.label(),
    )?;
    Ok(())
}

pub(crate) fn clean_clone_url(value: &str) -> String {
    Url::parse(value)
        .map(|mut url| {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        })
        .unwrap_or_default()
}

fn repository_from(remote: &Remote, value: &Value) -> ExternalRepository {
    match remote.provider {
        Provider::Bitbucket => ExternalRepository {
            name: field(value, "name").into(),
            clone_url: value["links"]["clone"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|link| link["name"] == "https")
                .map(|link| clean_clone_url(field(link, "href")))
                .unwrap_or_default(),
            web_url: provider_link(
                value
                    .pointer("/links/html/href")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                remote,
            ),
            private: value["is_private"].as_bool().unwrap_or(true),
        },
        Provider::Azure => ExternalRepository {
            name: field(value, "name").into(),
            clone_url: clean_clone_url(field(value, "remoteUrl")),
            web_url: provider_link(field(value, "webUrl"), remote),
            private: value.pointer("/project/visibility").and_then(Value::as_str) != Some("public"),
        },
    }
}

fn repositories_at(
    remote: &Remote,
    credential: &Credential,
    api: &Endpoints,
) -> Result<Vec<ExternalRepository>, String> {
    let url = match remote.provider {
        Provider::Bitbucket => format!("{}?pagelen=100&sort=-updated_on", api.account),
        Provider::Azure => format!(
            "{}/git/repositories?api-version={}",
            api.account,
            remote.api_version()
        ),
    };
    let result = send(
        request(&client()?, Method::GET, &url, credential),
        remote.provider.label(),
    )?;
    let values = result
        .get("values")
        .or_else(|| result.get("value"))
        .and_then(Value::as_array);
    let mut repositories: Vec<_> = values
        .into_iter()
        .flatten()
        .map(|value| repository_from(remote, value))
        .collect();
    repositories.sort_by_key(|repo| repo.name.to_lowercase());
    Ok(repositories)
}

fn create_repository_at(
    remote: &Remote,
    credential: &Credential,
    name: &str,
    private: bool,
    api: &Endpoints,
) -> Result<ExternalRepository, String> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 100
        || name.starts_with('.')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(
            "Use letters, numbers, dots, dashes or underscores for the repository name.".into(),
        );
    }
    let client = client()?;
    let label = remote.provider.label();
    let created = match remote.provider {
        Provider::Bitbucket => {
            let url = format!("{}/{}", api.account, name.to_lowercase());
            send(
                request(&client, Method::POST, &url, credential)
                    .json(&json!({"scm": "git", "is_private": private})),
                label,
            )?
        }
        Provider::Azure => {
            if !private {
                return Err("Azure DevOps repositories follow their project's visibility. Change it in the project settings.".into());
            }
            let version = remote.api_version();
            let project = send(
                request(
                    &client,
                    Method::GET,
                    &format!(
                        "{}/_apis/projects/{}?api-version={version}",
                        api.collection,
                        encode_segment(&remote.project)
                    ),
                    credential,
                ),
                label,
            )?;
            let id = field(&project, "id");
            if id.is_empty() {
                return Err("Azure DevOps did not return the project ID.".into());
            }
            let url = format!("{}/git/repositories?api-version={version}", api.account);
            send(
                request(&client, Method::POST, &url, credential)
                    .json(&json!({"name": name, "project": {"id": id}})),
                label,
            )?
        }
    };
    Ok(repository_from(remote, &created))
}

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| format!("Provider request stopped unexpectedly: {error}"))?
}

async fn with_remote<T: Send + 'static>(
    path: String,
    job: impl FnOnce(&Remote, &Credential, &Endpoints) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let remote = Remote::from_repo(&path)?;
        let credential = remote.credential()?;
        job(&remote, &credential, &remote.endpoints())
    })
    .await
    .map_err(|error| format!("Provider request stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn external_approve(path: String, number: i64) -> Result<(), String> {
    if let Some(server) = Server::from_repo(&path) {
        check_number(number)?;
        return blocking(move || server.approve(number)).await;
    }
    with_remote(path, move |remote, credential, api| {
        approve_at(remote, credential, number, api)
    })
    .await
}

#[tauri::command]
pub async fn external_comments(path: String, number: i64) -> Result<Vec<ExternalComment>, String> {
    if let Some(server) = Server::from_repo(&path) {
        check_number(number)?;
        return blocking(move || server.comments(number)).await;
    }
    with_remote(path, move |remote, credential, api| {
        comments_at(remote, credential, number, api)
    })
    .await
}

#[tauri::command]
pub async fn external_comment(
    path: String,
    number: i64,
    body: String,
    reply_to: Option<ReplyTarget>,
) -> Result<(), String> {
    if let Some(server) = Server::from_repo(&path) {
        check_number(number)?;
        return blocking(move || server.comment(number, &body, reply_to.map(|target| target.id)))
            .await;
    }
    with_remote(path, move |remote, credential, api| {
        comment_at(remote, credential, number, &body, reply_to, api)
    })
    .await
}

#[tauri::command]
pub async fn external_repositories(path: String) -> Result<Vec<ExternalRepository>, String> {
    if let Some(server) = Server::from_repo(&path) {
        return blocking(move || server.repositories()).await;
    }
    with_remote(path, |remote, credential, api| {
        repositories_at(remote, credential, api)
    })
    .await
}

#[tauri::command]
pub async fn external_create_repository(
    path: String,
    name: String,
    private: bool,
) -> Result<ExternalRepository, String> {
    if let Some(server) = Server::from_repo(&path) {
        return blocking(move || server.create_repository(&name, private)).await;
    }
    with_remote(path, move |remote, credential, api| {
        create_repository_at(remote, credential, &name, private, api)
    })
    .await
}

/// Credentials for HTTPS Git operations against a connected Bitbucket Cloud
/// or Azure DevOps remote.
pub(crate) fn git_auth_for_url(url: &str) -> Result<Option<crate::forge::GitAuth>, String> {
    let Ok(remote) = Remote::parse(url) else {
        return Server::git_auth(url);
    };
    if !url.starts_with("https://") {
        return Ok(None);
    }
    let credential = match remote.keychain_entry()?.get_password() {
        Ok(_) => remote.credential()?,
        Err(KeyringError::NoEntry) => return Ok(None),
        Err(error) => return Err(format!("Could not read the system keychain: {error}")),
    };
    Ok(Some(crate::forge::GitAuth {
        username: match (remote.provider, credential.email == OAUTH) {
            (Provider::Bitbucket, true) => "x-token-auth",
            (Provider::Bitbucket, false) => "x-bitbucket-api-token-auth",
            (Provider::Azure, _) => "pat",
        }
        .to_string(),
        host: remote.host,
        token: credential.token,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn mock_server(
        body: &'static str,
        expected_path: &'static str,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0u8; 8192];
            let read = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..read]);
            assert!(request.starts_with(expected_path), "{request}");
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: basic "));
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        });
        (url, handle)
    }

    #[test]
    fn parses_provider_remotes_and_rejects_unsafe_urls() {
        let bitbucket = Remote::parse("git@bitbucket.org:team/example.git").unwrap();
        assert_eq!(bitbucket.provider, Provider::Bitbucket);
        assert_eq!(bitbucket.repository, "example");
        let azure = Remote::parse("https://dev.azure.com/team/work/_git/example").unwrap();
        assert_eq!(azure.project, "work");
        let spaced =
            Remote::parse("https://dev.azure.com/team/My%20Project/_git/Repo%20Name").unwrap();
        assert_eq!(spaced.repository, "Repo Name");
        assert_eq!(
            spaced.api_base(),
            "https://dev.azure.com/team/My%20Project/_apis/git/repositories/Repo%20Name"
        );
        assert_eq!(
            Remote::parse("git@ssh.dev.azure.com:v3/team/work/example")
                .unwrap()
                .provider,
            Provider::Azure
        );
        assert!(Remote::parse("https://user:secret@bitbucket.org/team/example").is_err());
        assert!(Remote::parse("https://bitbucket.org/team/../example").is_err());
        assert!(Remote::parse("https://elsewhere.test/team/example").is_err());
        assert!(Remote::parse("https://dev.azure.com/team/My%2FProject/_git/repo").is_err());
        let server =
            Remote::parse("https://tfs.example.test/tfs/DefaultCollection/Work/_git/App").unwrap();
        assert_eq!(server.provider, Provider::Azure);
        assert_eq!(server.api_version(), "6.0");
        assert_eq!(
            server.api_base(),
            "https://tfs.example.test/tfs/DefaultCollection/Work/_apis/git/repositories/App"
        );
        assert!(Remote::parse("git@tfs.example.test:tfs/Work/_git/App").is_err());
    }

    #[test]
    fn validates_project_segments() {
        assert!(valid_segment("main"));
        assert!(!valid_segment("../bad"));
        assert!(!valid_segment(""));
    }

    #[test]
    fn reads_bitbucket_pull_requests_with_basic_auth() {
        let remote = Remote::parse("https://bitbucket.org/team/example.git").unwrap();
        let credential = Credential {
            email: "a@example.com".into(),
            token: Zeroizing::new("secret".into()),
        };
        let (base, server) = mock_server(
            r#"{"values":[{"id":7,"title":"Review me","state":"OPEN","links":{"html":{"href":"https://bitbucket.org/team/example/pull-requests/7"}}}],"next":"more"}"#,
            "GET /pullrequests?pagelen=30&page=1&state=OPEN HTTP/1.1",
        );
        let page = list_at(
            &remote,
            &credential,
            "reviews",
            "open",
            1,
            &endpoints(&base),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(page.items[0].title, "Review me");
        assert!(page.has_more);
    }

    #[test]
    fn reads_azure_pull_requests_with_basic_auth() {
        let remote = Remote::parse("https://dev.azure.com/team/work/_git/example").unwrap();
        let credential = Credential {
            email: String::new(),
            token: Zeroizing::new("secret".into()),
        };
        let (base, server) = mock_server(
            r#"{"value":[{"pullRequestId":8,"title":"Ship","status":"active","createdBy":{"displayName":"Ada"}}]}"#,
            "GET /pullrequests?searchCriteria.status=active&%24top=30&%24skip=0&api-version=7.1 HTTP/1.1",
        );
        let page = list_at(
            &remote,
            &credential,
            "reviews",
            "open",
            1,
            &endpoints(&base),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(page.items[0].number, 8);
        assert_eq!(
            page.items[0].url,
            "https://dev.azure.com/team/work/_git/example/pullrequest/8"
        );
    }

    #[test]
    fn creates_bitbucket_review_with_branch_payload() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut bytes = [0u8; 4096];
            loop {
                let count = stream.read(&mut bytes).unwrap();
                request.extend_from_slice(&bytes[..count]);
                let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
            let received = String::from_utf8(request).unwrap();
            assert!(received.starts_with("POST /pullrequests HTTP/1.1"));
            assert!(received.contains("\"name\":\"feature/new\""));
            assert!(received.contains("\"name\":\"main\""));
            write!(stream, "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}").unwrap();
        });
        let remote = Remote::parse("https://bitbucket.org/team/example").unwrap();
        let credential = Credential {
            email: "a@example.com".into(),
            token: Zeroizing::new("secret".into()),
        };
        create_review_at(
            &remote,
            &credential,
            "Title",
            "Description",
            "feature/new",
            "main",
            &base,
        )
        .unwrap();
        server.join().unwrap();
    }

    #[test]
    fn azure_merge_uses_server_source_commit() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            for (method, body) in [
                ("GET", r#"{"lastMergeSourceCommit":{"commitId":"abc123"}}"#),
                ("PATCH", "{}"),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                let mut bytes = [0u8; 4096];
                loop {
                    let count = stream.read(&mut bytes).unwrap();
                    request.extend_from_slice(&bytes[..count]);
                    let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                let received = String::from_utf8(request).unwrap();
                assert!(received.starts_with(&format!(
                    "{method} /pullrequests/9?api-version=7.1 HTTP/1.1"
                )));
                if method == "PATCH" {
                    assert!(received.contains("\"commitId\":\"abc123\""));
                }
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let remote = Remote::parse("https://dev.azure.com/team/work/_git/example").unwrap();
        let credential = Credential {
            email: String::new(),
            token: Zeroizing::new("secret".into()),
        };
        merge_at(&remote, &credential, 9, &base).unwrap();
        server.join().unwrap();
    }

    /// Serves one canned response per request and returns what was received.
    fn serve(responses: Vec<&'static str>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut received = Vec::new();
            for body in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                let mut bytes = [0u8; 4096];
                loop {
                    let count = stream.read(&mut bytes).unwrap();
                    request.extend_from_slice(&bytes[..count]);
                    let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
                received.push(String::from_utf8(request).unwrap());
            }
            received
        });
        (base, handle)
    }

    fn endpoints(base: &str) -> Endpoints {
        Endpoints {
            repo: base.into(),
            account: base.into(),
            collection: base.into(),
        }
    }

    fn token() -> Credential {
        Credential {
            email: "a@example.com".into(),
            token: Zeroizing::new("secret".into()),
        }
    }

    #[test]
    fn approves_pull_requests() {
        let bitbucket = Remote::parse("https://bitbucket.org/team/example").unwrap();
        let (base, server) = serve(vec!["{}"]);
        approve_at(&bitbucket, &token(), 4, &endpoints(&base)).unwrap();
        assert!(server.join().unwrap()[0].starts_with("POST /pullrequests/4/approve HTTP/1.1"));

        let azure = Remote::parse("https://dev.azure.com/team/work/_git/example").unwrap();
        let (base, server) = serve(vec![r#"{"authenticatedUser":{"id":"user-1"}}"#, "{}"]);
        approve_at(&azure, &token(), 5, &endpoints(&base)).unwrap();
        let requests = server.join().unwrap();
        assert!(requests[0].starts_with("GET /_apis/connectionData?api-version=7.1 HTTP/1.1"));
        assert!(requests[1]
            .starts_with("PUT /pullrequests/5/reviewers/user%2D1?api-version=7.1 HTTP/1.1"));
        assert!(requests[1].ends_with(r#"{"vote":10}"#));
        assert!(approve_at(&azure, &token(), 0, &endpoints(&base)).is_err());
    }

    #[test]
    fn reads_and_posts_comments() {
        let bitbucket = Remote::parse("https://bitbucket.org/team/example").unwrap();
        let (base, server) = serve(vec![
            r#"{"values":[{"content":{"raw":"Looks good"},"user":{"display_name":"Ada"},"created_on":"2026-01-01","inline":{"path":"src/a.rs","to":12}},{"deleted":true,"content":{"raw":"gone"}}]}"#,
            "{}",
        ]);
        let comments = comments_at(&bitbucket, &token(), 3, &endpoints(&base)).unwrap();
        assert_eq!(
            comments,
            vec![ExternalComment {
                id: 0,
                thread: 0,
                author: "Ada".into(),
                body: "Looks good".into(),
                created: "2026-01-01".into(),
                location: "src/a.rs:12".into()
            }]
        );
        comment_at(&bitbucket, &token(), 3, "Thanks!", None, &endpoints(&base)).unwrap();
        let requests = server.join().unwrap();
        assert!(requests[1].ends_with(r#"{"content":{"raw":"Thanks!"}}"#));
        assert!(comment_at(&bitbucket, &token(), 3, "  ", None, &endpoints(&base)).is_err());

        let azure = Remote::parse("https://dev.azure.com/team/work/_git/example").unwrap();
        let (base, server) = serve(vec![
            r#"{"value":[{"threadContext":{"filePath":"/lib.rs","rightFileStart":{"line":4}},"comments":[{"content":"Why?","author":{"displayName":"Lin"},"publishedDate":"2026-02-02","commentType":"text"},{"content":"Policy","commentType":"system"}]}]}"#,
        ]);
        let comments = comments_at(&azure, &token(), 9, &endpoints(&base)).unwrap();
        server.join().unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].location, "lib.rs:4");
    }

    #[test]
    fn lists_and_creates_repositories() {
        let bitbucket = Remote::parse("https://bitbucket.org/team/example").unwrap();
        let (base, server) = serve(vec![
            r#"{"values":[{"name":"zeta","is_private":false,"links":{"clone":[{"name":"ssh","href":"git@bitbucket.org:team/zeta.git"},{"name":"https","href":"https://ada@bitbucket.org/team/zeta.git"}],"html":{"href":"https://bitbucket.org/team/zeta"}}},{"name":"Alpha","is_private":true,"links":{"clone":[]}}]}"#,
            r#"{"name":"New-Repo","is_private":true,"links":{"clone":[{"name":"https","href":"https://bitbucket.org/team/new-repo.git"}]}}"#,
        ]);
        let repos = repositories_at(&bitbucket, &token(), &endpoints(&base)).unwrap();
        assert_eq!(
            repos
                .iter()
                .map(|repo| repo.name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "zeta"]
        );
        assert_eq!(repos[1].clone_url, "https://bitbucket.org/team/zeta.git");
        assert_eq!(repos[1].web_url, "https://bitbucket.org/team/zeta");
        let created =
            create_repository_at(&bitbucket, &token(), "New-Repo", true, &endpoints(&base))
                .unwrap();
        assert_eq!(created.clone_url, "https://bitbucket.org/team/new-repo.git");
        let requests = server.join().unwrap();
        assert!(requests[1].starts_with("POST /new-repo HTTP/1.1"));
        assert!(
            create_repository_at(&bitbucket, &token(), "bad name", true, &endpoints(&base))
                .is_err()
        );
    }

    #[test]
    fn pages_azure_build_history() {
        let azure = Remote::parse("https://dev.azure.com/team/work/_git/example").unwrap();
        let builds: String = (0..60)
            .map(|n| format!(r#"{{"id":{n},"buildNumber":"{n}","status":"completed"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let builds: &'static str = Box::leak(format!(r#"{{"value":[{builds}]}}"#).into_boxed_str());
        let (base, server) = serve(vec![r#"{"id":"repo-1"}"#, builds]);
        let page = list_at(&azure, &token(), "ci", "open", 2, &endpoints(&base)).unwrap();
        server.join().unwrap();
        assert_eq!(page.items.len(), 30);
        assert_eq!(page.items[0].number, 30);
        assert!(page.has_more);
    }

    #[test]
    fn replies_to_existing_comments() {
        let bitbucket = Remote::parse("https://bitbucket.org/team/example").unwrap();
        let azure = Remote::parse("https://dev.azure.com/team/work/_git/example").unwrap();
        let (base, server) = serve(vec!["{}", "{}"]);
        let target = ReplyTarget { id: 41, thread: 7 };
        comment_at(
            &bitbucket,
            &token(),
            3,
            "Agreed",
            Some(target),
            &endpoints(&base),
        )
        .unwrap();
        comment_at(&azure, &token(), 3, "Done", Some(target), &endpoints(&base)).unwrap();
        let requests = server.join().unwrap();
        assert!(requests[0].ends_with(r#"{"content":{"raw":"Agreed"},"parent":{"id":41}}"#));
        assert!(requests[1]
            .starts_with("POST /pullRequests/3/threads/7/comments?api-version=7.1 HTTP/1.1"));
        assert!(requests[1].contains(r#""parentCommentId":41"#));
    }
}
