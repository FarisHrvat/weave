//! Dropbox, Google Drive and OneDrive. Each provider is used through an app
//! the user registers with it (Weave cannot ship one), signed in with OAuth
//! and PKCE in the browser. Tokens stay in the system keychain.

use crate::{
    files::{FileEntry, FileListing},
    profiles::{self, Profile},
    AppState,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use keyring::{Entry, Error as KeyringError};
use reqwest::{
    blocking::{Body, Client, RequestBuilder, Response},
    header::{CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE},
    StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    net::TcpListener,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};
use tempfile::NamedTempFile;

pub(crate) const REDIRECT_URI: &str = "http://localhost:53682/";
const REDIRECT_PORT: u16 = 53682;
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Provider {
    Dropbox,
    GoogleDrive,
    OneDrive,
}

impl Provider {
    pub(crate) fn from_protocol(protocol: &str) -> Option<Self> {
        match protocol {
            "dropbox" => Some(Self::Dropbox),
            "gdrive" => Some(Self::GoogleDrive),
            "onedrive" => Some(Self::OneDrive),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Dropbox => "dropbox",
            Self::GoogleDrive => "gdrive",
            Self::OneDrive => "onedrive",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Dropbox => "Dropbox",
            Self::GoogleDrive => "Google Drive",
            Self::OneDrive => "OneDrive",
        }
    }

    pub(crate) fn host(self) -> &'static str {
        match self {
            Self::Dropbox => "dropbox.com",
            Self::GoogleDrive => "drive.google.com",
            Self::OneDrive => "onedrive.live.com",
        }
    }
}

pub(crate) fn is_cloud(protocol: &str) -> bool {
    Provider::from_protocol(protocol).is_some()
}

#[derive(Clone)]
pub(crate) struct Endpoints {
    auth: String,
    token: String,
    api: String,
    content: String,
}

impl Endpoints {
    fn real(provider: Provider) -> Self {
        let (auth, token, api, content) = match provider {
            Provider::Dropbox => (
                "https://www.dropbox.com/oauth2/authorize",
                "https://api.dropboxapi.com/oauth2/token",
                "https://api.dropboxapi.com/2",
                "https://content.dropboxapi.com/2",
            ),
            Provider::GoogleDrive => (
                "https://accounts.google.com/o/oauth2/v2/auth",
                "https://oauth2.googleapis.com/token",
                "https://www.googleapis.com/drive/v3",
                "https://www.googleapis.com/upload/drive/v3",
            ),
            Provider::OneDrive => (
                "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
                "https://login.microsoftonline.com/common/oauth2/v2.0/token",
                "https://graph.microsoft.com/v1.0",
                "https://graph.microsoft.com/v1.0",
            ),
        };
        Self {
            auth: auth.into(),
            token: token.into(),
            api: api.into(),
            content: content.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn at(base: &str) -> Self {
        Self {
            auth: format!("{base}/authorize"),
            token: format!("{base}/token"),
            api: format!("{base}/api"),
            content: format!("{base}/content"),
        }
    }
}

/// The app the user registered with the provider.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRegistration {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Tokens {
    access: String,
    refresh: Option<String>,
    expires_at: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

fn keychain(account: &str) -> Result<Entry, String> {
    Entry::new("app.weave.desktop", account)
        .map_err(|error| format!("Could not open the system keychain: {error}"))
}

fn read_json<T: serde::de::DeserializeOwned>(account: &str) -> Result<Option<T>, String> {
    match keychain(account)?.get_password() {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| "A saved cloud sign-in is damaged. Sign in again.".to_string()),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(format!("Could not read the system keychain: {error}")),
    }
}

fn write_json<T: Serialize>(account: &str, value: &T) -> Result<(), String> {
    let text = serde_json::to_string(value).map_err(|error| error.to_string())?;
    keychain(account)?
        .set_password(&text)
        .map_err(|error| format!("Could not save to the system keychain: {error}"))
}

fn tokens_account(profile_id: &str) -> String {
    format!("cloud-tokens:{profile_id}")
}

fn app_account(provider: Provider) -> String {
    format!("cloud-app:{}", provider.key())
}

fn parse_time(value: Option<&str>) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(value?)
        .ok()
        .and_then(|time| u64::try_from(time.timestamp()).ok())
}

pub(crate) fn normalize(path: &str) -> String {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    format!("/{}", parts.join("/"))
}

pub(crate) fn split(path: &str) -> (String, String) {
    let path = normalize(path);
    match path.rsplit_once('/') {
        Some(("", name)) => ("/".into(), name.into()),
        Some((parent, name)) => (parent.into(), name.into()),
        None => ("/".into(), path),
    }
}

fn child(parent: &str, name: &str) -> String {
    normalize(&format!("{parent}/{name}"))
}

pub(crate) fn listing(path: &str, entries: Vec<FileEntry>) -> FileListing {
    let path = normalize(path);
    let parent = (path != "/").then(|| split(&path).0);
    let mut entries = entries;
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    FileListing {
        path,
        parent,
        entries,
    }
}

/// Dropbox-API-Arg must be ASCII; other characters are sent as \u escapes.
fn header_json(value: &Value) -> String {
    value
        .to_string()
        .chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_string()
            } else {
                c.encode_utf16(&mut [0; 2])
                    .iter()
                    .map(|unit| format!("\\u{unit:04x}"))
                    .collect()
            }
        })
        .collect()
}

fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|part| {
            percent_encoding::utf8_percent_encode(part, percent_encoding::NON_ALPHANUMERIC)
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn drive_quote(name: &str) -> String {
    format!("'{}'", name.replace('\\', "\\\\").replace('\'', "\\'"))
}

pub struct CloudUpload {
    file: NamedTempFile,
    path: String,
    position: u64,
}

pub struct CloudClient {
    provider: Provider,
    endpoints: Endpoints,
    http: Client,
    app: AppRegistration,
    tokens: Mutex<Tokens>,
    profile_id: Option<String>,
    /// Google Drive addresses files by ID; paths are resolved and remembered.
    ids: Mutex<HashMap<String, (String, String)>>,
}

impl CloudClient {
    pub fn connect(profile: &Profile) -> Result<Self, String> {
        let provider = Provider::from_protocol(&profile.protocol)
            .ok_or("This is not a cloud drive connection.")?;
        let app = read_json::<AppRegistration>(&app_account(provider))?.ok_or_else(|| {
            format!(
                "Add your {} app's client ID in the connection settings, then sign in.",
                provider.name()
            )
        })?;
        let tokens = read_json::<Tokens>(&tokens_account(&profile.id))?.ok_or_else(|| {
            format!(
                "Sign in to {} in the connection settings first.",
                provider.name()
            )
        })?;
        Self::with(
            provider,
            Endpoints::real(provider),
            app,
            tokens,
            Some(profile.id.clone()),
        )
    }

    fn with(
        provider: Provider,
        endpoints: Endpoints,
        app: AppRegistration,
        tokens: Tokens,
        profile_id: Option<String>,
    ) -> Result<Self, String> {
        let http = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|error| format!("Could not prepare the connection: {error}"))?;
        Ok(Self {
            provider,
            endpoints,
            http,
            app,
            tokens: Mutex::new(tokens),
            profile_id,
            ids: Mutex::new(HashMap::new()),
        })
    }

    fn refresh(&self, tokens: &mut Tokens) -> Result<(), String> {
        let refresh = tokens.refresh.clone().ok_or_else(|| {
            format!(
                "The {} sign-in expired. Sign in again in the connection settings.",
                self.provider.name()
            )
        })?;
        let mut form = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh),
            ("client_id", self.app.client_id.clone()),
        ];
        if let Some(secret) = &self.app.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        let response = self
            .http
            .post(&self.endpoints.token)
            .form(&form)
            .send()
            .map_err(|error| format!("Could not reach {}: {error}", self.provider.name()))?;
        if !response.status().is_success() {
            return Err(format!(
                "{} no longer accepts this sign-in. Sign in again in the connection settings.",
                self.provider.name()
            ));
        }
        let value: Value = response
            .json()
            .map_err(|_| "The sign-in server sent an unreadable reply.")?;
        *tokens = token_reply(&value, tokens.refresh.clone())?;
        if let Some(id) = &self.profile_id {
            write_json(&tokens_account(id), tokens)?;
        }
        Ok(())
    }

    fn access(&self, force_refresh: bool) -> Result<String, String> {
        let mut tokens = self
            .tokens
            .lock()
            .map_err(|_| "The cloud connection is busy.")?;
        if force_refresh || tokens.expires_at <= now() + 60 {
            self.refresh(&mut tokens)?;
        }
        Ok(tokens.access.clone())
    }

    /// Sends a request with the access token, renewing it once if refused.
    fn call(
        &self,
        action: &str,
        build: impl Fn(&str) -> RequestBuilder,
    ) -> Result<Response, String> {
        let mut response = build(&self.access(false)?)
            .send()
            .map_err(|error| format!("Could not {action}: {error}"))?;
        if response.status() == StatusCode::UNAUTHORIZED
            && self
                .tokens
                .lock()
                .map(|tokens| tokens.refresh.is_some())
                .unwrap_or(false)
        {
            response = build(&self.access(true)?)
                .send()
                .map_err(|error| format!("Could not {action}: {error}"))?;
        }
        self.checked(response, action)
    }

    fn checked(&self, response: Response, action: &str) -> Result<Response, String> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().unwrap_or_default();
        let detail = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| {
                value["error_summary"]
                    .as_str()
                    .or(value["error"]["message"].as_str())
                    .or(value["error_description"].as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let name = self.provider.name();
        let hint = match status.as_u16() {
            401 => format!("Sign in to {name} again in the connection settings."),
            403 => format!(
                "{name} refused access. Check the app's permissions and your storage quota."
            ),
            404 => "It does not exist.".into(),
            409 if detail.contains("not_found") => "It does not exist.".into(),
            409 if detail.contains("conflict") => "Something with that name already exists.".into(),
            429 => format!("{name} is limiting requests. Try again in a minute."),
            500..=599 => format!("{name} is having trouble. Try again shortly."),
            _ if !detail.is_empty() => detail.clone(),
            _ => "Check the connection settings.".into(),
        };
        Err(format!(
            "Could not {action}: HTTP {}. {hint}",
            status.as_u16()
        ))
    }

    fn json(&self, action: &str, build: impl Fn(&str) -> RequestBuilder) -> Result<Value, String> {
        self.call(action, build)?
            .json()
            .map_err(|_| format!("Could not {action}: the reply was unreadable."))
    }

    fn dropbox_path(path: &str) -> String {
        let path = normalize(path);
        if path == "/" {
            String::new()
        } else {
            path
        }
    }

    fn dropbox(&self, endpoint: &str, body: Value, action: &str) -> Result<Value, String> {
        let url = format!("{}/{endpoint}", self.endpoints.api);
        self.json(action, |token| {
            self.http.post(&url).bearer_auth(token).json(&body)
        })
    }

    fn onedrive_item(&self, path: &str, suffix: &str) -> String {
        let path = normalize(path);
        if path == "/" {
            format!("{}/me/drive/root{suffix}", self.endpoints.api)
        } else {
            format!(
                "{}/me/drive/root:{}:{suffix}",
                self.endpoints.api,
                encode_path(&path)
            )
        }
    }

    /// Google Drive ID and type for a path.
    fn drive_item(&self, path: &str) -> Result<(String, String), String> {
        let path = normalize(path);
        if path == "/" {
            return Ok(("root".into(), FOLDER_MIME.into()));
        }
        if let Some(found) = self.ids.lock().ok().and_then(|ids| ids.get(&path).cloned()) {
            return Ok(found);
        }
        let (parent, name) = split(&path);
        let (parent_id, _) = self.drive_item(&parent)?;
        let query = format!(
            "{} in parents and name = {} and trashed = false",
            drive_quote(&parent_id),
            drive_quote(&name)
        );
        let url = format!("{}/files", self.endpoints.api);
        let value = self.json("find the file in Google Drive", |token| {
            self.http.get(&url).bearer_auth(token).query(&[
                ("q", query.as_str()),
                ("fields", "files(id,mimeType)"),
                ("pageSize", "10"),
            ])
        })?;
        let file = value["files"]
            .as_array()
            .and_then(|files| files.first())
            .ok_or_else(|| format!("{path} does not exist."))?;
        let found = (
            file["id"].as_str().unwrap_or_default().to_string(),
            file["mimeType"].as_str().unwrap_or_default().to_string(),
        );
        if let Ok(mut ids) = self.ids.lock() {
            ids.insert(path, found.clone());
        }
        Ok(found)
    }

    fn forget(&self, path: &str) {
        let path = normalize(path);
        if let Ok(mut ids) = self.ids.lock() {
            ids.retain(|known, _| known != &path && !known.starts_with(&format!("{path}/")));
        }
    }

    fn drive_entry(&self, parent: &str, file: &Value) -> FileEntry {
        let name = file["name"].as_str().unwrap_or_default().to_string();
        let path = child(parent, &name);
        let mime = file["mimeType"].as_str().unwrap_or_default();
        if let Ok(mut ids) = self.ids.lock() {
            ids.insert(
                path.clone(),
                (
                    file["id"].as_str().unwrap_or_default().to_string(),
                    mime.to_string(),
                ),
            );
        }
        FileEntry {
            name,
            path,
            is_dir: mime == FOLDER_MIME,
            is_symlink: false,
            size: file["size"]
                .as_str()
                .and_then(|size| size.parse().ok())
                .unwrap_or(0),
            modified: parse_time(file["modifiedTime"].as_str()),
            readonly: false,
        }
    }

    pub fn list(&self, path: &str) -> Result<FileListing, String> {
        let path = normalize(path);
        let mut entries = Vec::new();
        match self.provider {
            Provider::Dropbox => {
                let mut page = self.dropbox(
                    "files/list_folder",
                    json!({"path": Self::dropbox_path(&path), "limit": 2000}),
                    "list the Dropbox folder",
                )?;
                loop {
                    for entry in page["entries"].as_array().into_iter().flatten() {
                        let tag = entry[".tag"].as_str().unwrap_or_default();
                        if tag != "file" && tag != "folder" {
                            continue;
                        }
                        let name = entry["name"].as_str().unwrap_or_default().to_string();
                        entries.push(FileEntry {
                            path: child(&path, &name),
                            name,
                            is_dir: tag == "folder",
                            is_symlink: false,
                            size: entry["size"].as_u64().unwrap_or(0),
                            modified: parse_time(entry["server_modified"].as_str()),
                            readonly: false,
                        });
                    }
                    if page["has_more"] != true || entries.len() > 100_000 {
                        break;
                    }
                    page = self.dropbox(
                        "files/list_folder/continue",
                        json!({"cursor": page["cursor"]}),
                        "list the Dropbox folder",
                    )?;
                }
            }
            Provider::OneDrive => {
                let mut url = format!("{}?$top=1000", self.onedrive_item(&path, "/children"));
                loop {
                    let page = self.json("list the OneDrive folder", |token| {
                        self.http.get(&url).bearer_auth(token)
                    })?;
                    for item in page["value"].as_array().into_iter().flatten() {
                        let name = item["name"].as_str().unwrap_or_default().to_string();
                        entries.push(FileEntry {
                            path: child(&path, &name),
                            name,
                            is_dir: item.get("folder").is_some(),
                            is_symlink: false,
                            size: item["size"].as_u64().unwrap_or(0),
                            modified: parse_time(item["lastModifiedDateTime"].as_str()),
                            readonly: false,
                        });
                    }
                    match page["@odata.nextLink"].as_str() {
                        Some(next) if entries.len() < 100_000 => url = next.to_string(),
                        _ => break,
                    }
                }
            }
            Provider::GoogleDrive => {
                let (id, mime) = self.drive_item(&path)?;
                if mime != FOLDER_MIME {
                    return Err(format!("{path} is a file, not a folder."));
                }
                let query = format!("{} in parents and trashed = false", drive_quote(&id));
                let url = format!("{}/files", self.endpoints.api);
                let mut page_token = String::new();
                loop {
                    let page = self.json("list the Google Drive folder", |token| {
                        let mut request = self.http.get(&url).bearer_auth(token).query(&[
                            ("q", query.as_str()),
                            (
                                "fields",
                                "nextPageToken,files(id,name,mimeType,size,modifiedTime)",
                            ),
                            ("pageSize", "1000"),
                        ]);
                        if !page_token.is_empty() {
                            request = request.query(&[("pageToken", page_token.as_str())]);
                        }
                        request
                    })?;
                    for file in page["files"].as_array().into_iter().flatten() {
                        entries.push(self.drive_entry(&path, file));
                    }
                    match page["nextPageToken"].as_str() {
                        Some(next) if entries.len() < 100_000 => page_token = next.to_string(),
                        _ => break,
                    }
                }
            }
        }
        Ok(listing(&path, entries))
    }

    pub fn stat(&self, path: &str) -> Result<FileEntry, String> {
        let path = normalize(path);
        if path == "/" {
            return Ok(FileEntry {
                name: "/".into(),
                path,
                is_dir: true,
                is_symlink: false,
                size: 0,
                modified: None,
                readonly: false,
            });
        }
        let (parent, name) = split(&path);
        match self.provider {
            Provider::Dropbox => {
                let entry = self.dropbox(
                    "files/get_metadata",
                    json!({"path": path}),
                    &format!("open {path}"),
                )?;
                Ok(FileEntry {
                    name,
                    path,
                    is_dir: entry[".tag"] == "folder",
                    is_symlink: false,
                    size: entry["size"].as_u64().unwrap_or(0),
                    modified: parse_time(entry["server_modified"].as_str()),
                    readonly: false,
                })
            }
            Provider::OneDrive => {
                let url = self.onedrive_item(&path, "");
                let item = self.json(&format!("open {path}"), |token| {
                    self.http.get(&url).bearer_auth(token)
                })?;
                Ok(FileEntry {
                    name,
                    path,
                    is_dir: item.get("folder").is_some(),
                    is_symlink: false,
                    size: item["size"].as_u64().unwrap_or(0),
                    modified: parse_time(item["lastModifiedDateTime"].as_str()),
                    readonly: false,
                })
            }
            Provider::GoogleDrive => {
                let (id, _) = self.drive_item(&path)?;
                let url = format!("{}/files/{id}", self.endpoints.api);
                let file = self.json(&format!("open {path}"), |token| {
                    self.http
                        .get(&url)
                        .bearer_auth(token)
                        .query(&[("fields", "id,name,mimeType,size,modifiedTime")])
                })?;
                Ok(self.drive_entry(&parent, &file))
            }
        }
    }

    pub fn mkdir(&self, path: &str) -> Result<(), String> {
        let path = normalize(path);
        let (parent, name) = split(&path);
        match self.provider {
            Provider::Dropbox => self
                .dropbox(
                    "files/create_folder_v2",
                    json!({"path": path, "autorename": false}),
                    &format!("create {path}"),
                )
                .map(drop),
            Provider::OneDrive => {
                let url = self.onedrive_item(&parent, "/children");
                let body = json!({"name": name, "folder": {}, "@microsoft.graph.conflictBehavior": "fail"});
                self.call(&format!("create {path}"), |token| {
                    self.http.post(&url).bearer_auth(token).json(&body)
                })
                .map(drop)
            }
            Provider::GoogleDrive => {
                if self.drive_item(&path).is_ok() {
                    return Err(format!("{path} already exists."));
                }
                let (parent_id, _) = self.drive_item(&parent)?;
                let url = format!("{}/files", self.endpoints.api);
                let body = json!({"name": name, "mimeType": FOLDER_MIME, "parents": [parent_id]});
                self.call(&format!("create {path}"), |token| {
                    self.http.post(&url).bearer_auth(token).json(&body)
                })
                .map(drop)
            }
        }
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        let (from, to) = (normalize(from), normalize(to));
        let (to_parent, to_name) = split(&to);
        let action = format!("move {from} to {to}");
        match self.provider {
            Provider::Dropbox => self
                .dropbox(
                    "files/move_v2",
                    json!({"from_path": from, "to_path": to, "autorename": false}),
                    &action,
                )
                .map(drop),
            Provider::OneDrive => {
                let url = self.onedrive_item(&from, "");
                let parent = if to_parent == "/" {
                    String::new()
                } else {
                    to_parent.clone()
                };
                let body = json!({"name": to_name, "parentReference": {"path": format!("/drive/root:{parent}")}});
                self.call(&action, |token| {
                    self.http.patch(&url).bearer_auth(token).json(&body)
                })
                .map(drop)
            }
            Provider::GoogleDrive => {
                if self.drive_item(&to).is_ok() {
                    return Err(format!("{to} already exists."));
                }
                let (id, _) = self.drive_item(&from)?;
                let (old_parent, _) = self.drive_item(&split(&from).0)?;
                let (new_parent, _) = self.drive_item(&to_parent)?;
                let url = format!("{}/files/{id}", self.endpoints.api);
                let body = json!({"name": to_name});
                self.call(&action, |token| {
                    let request = self.http.patch(&url).bearer_auth(token).json(&body);
                    if old_parent == new_parent {
                        request
                    } else {
                        request.query(&[
                            ("addParents", new_parent.as_str()),
                            ("removeParents", old_parent.as_str()),
                        ])
                    }
                })?;
                self.forget(&from);
                Ok(())
            }
        }
    }

    /// Deletes a file or folder. Google Drive items go to the Drive trash.
    pub fn delete(&self, path: &str, recursive: bool) -> Result<(), String> {
        let path = normalize(path);
        if path == "/" {
            return Err("The drive's root folder cannot be deleted.".into());
        }
        if !recursive && self.stat(&path)?.is_dir && !self.list(&path)?.entries.is_empty() {
            return Err(format!(
                "{path} is not empty. Choose recursive deletion to remove its contents."
            ));
        }
        let action = format!("delete {path}");
        match self.provider {
            Provider::Dropbox => self
                .dropbox("files/delete_v2", json!({"path": path}), &action)
                .map(drop),
            Provider::OneDrive => {
                let url = self.onedrive_item(&path, "");
                self.call(&action, |token| self.http.delete(&url).bearer_auth(token))
                    .map(drop)
            }
            Provider::GoogleDrive => {
                let (id, _) = self.drive_item(&path)?;
                let url = format!("{}/files/{id}", self.endpoints.api);
                self.call(&action, |token| {
                    self.http
                        .patch(&url)
                        .bearer_auth(token)
                        .json(&json!({"trashed": true}))
                })?;
                self.forget(&path);
                Ok(())
            }
        }
    }

    pub fn open_read(&self, path: &str, offset: u64) -> Result<Response, String> {
        let path = normalize(path);
        let action = format!("download {path}");
        let range = |request: RequestBuilder| {
            if offset > 0 {
                request.header(RANGE, format!("bytes={offset}-"))
            } else {
                request
            }
        };
        let response = match self.provider {
            Provider::Dropbox => {
                let url = format!("{}/files/download", self.endpoints.content);
                let arg = header_json(&json!({"path": path}));
                self.call(&action, |token| {
                    range(
                        self.http
                            .post(&url)
                            .bearer_auth(token)
                            .header("Dropbox-API-Arg", &arg),
                    )
                })?
            }
            Provider::OneDrive => {
                let url = self.onedrive_item(&path, "/content");
                self.call(&action, |token| {
                    range(self.http.get(&url).bearer_auth(token))
                })?
            }
            Provider::GoogleDrive => {
                let (id, mime) = self.drive_item(&path)?;
                if mime.starts_with("application/vnd.google-apps.") {
                    return Err(format!("{path} is a Google Docs, Sheets or Slides file. It has no file to download; export it from Google Drive in the browser."));
                }
                let url = format!("{}/files/{id}", self.endpoints.api);
                self.call(&action, |token| {
                    range(
                        self.http
                            .get(&url)
                            .bearer_auth(token)
                            .query(&[("alt", "media")]),
                    )
                })?
            }
        };
        if offset > 0 && response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(format!(
                "{} did not resume the download. Remove the partial copy and retry.",
                self.provider.name()
            ));
        }
        Ok(response)
    }

    pub fn open_write(
        &self,
        path: &str,
        truncate: bool,
        offset: u64,
    ) -> Result<CloudUpload, String> {
        let path = normalize(path);
        if path == "/" {
            return Err("Choose a file name inside the drive.".into());
        }
        let mut file = NamedTempFile::new()
            .map_err(|error| format!("Could not prepare the upload: {error}"))?;
        if !truncate && offset > 0 {
            let mut response = self.open_read(&path, 0)?;
            let copied = std::io::copy(&mut response, &mut file)
                .map_err(|error| format!("Could not resume the upload: {error}"))?;
            if copied != offset {
                return Err("The partial file in the drive changed. Remove it and retry.".into());
            }
        }
        Ok(CloudUpload {
            file,
            path,
            position: offset,
        })
    }

    pub fn write_chunk(upload: &mut CloudUpload, offset: u64, data: &[u8]) -> Result<(), String> {
        if upload.position != offset {
            return Err("The upload position changed unexpectedly. Retry the transfer.".into());
        }
        upload
            .file
            .write_all(data)
            .map_err(|error| format!("Could not prepare the upload: {error}"))?;
        upload.position += data.len() as u64;
        Ok(())
    }

    pub fn write_small(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let mut upload = self.open_write(path, true, 0)?;
        Self::write_chunk(&mut upload, 0, bytes)?;
        self.finish_write(upload)
    }

    fn part(upload: &CloudUpload, offset: u64, length: u64) -> Result<Body, String> {
        let mut file = File::open(upload.file.path())
            .map_err(|error| format!("Could not read the upload: {error}"))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| format!("Could not read the upload: {error}"))?;
        Ok(Body::sized(file.take(length), length))
    }

    pub fn finish_write(&self, upload: CloudUpload) -> Result<(), String> {
        let size = upload.position;
        let path = upload.path.clone();
        let action = format!("upload {path}");
        File::open(upload.file.path())
            .map_err(|error| format!("Could not read the upload: {error}"))?;
        match self.provider {
            Provider::Dropbox => {
                const CHUNK: u64 = 64 * 1024 * 1024;
                let commit = json!({"path": path, "mode": "overwrite", "mute": true});
                let post = |endpoint: &str, arg: Value, offset: u64, length: u64| {
                    let url = format!("{}/{endpoint}", self.endpoints.content);
                    let arg = header_json(&arg);
                    self.call(&action, |token| {
                        self.http
                            .post(&url)
                            .bearer_auth(token)
                            .header("Dropbox-API-Arg", &arg)
                            .header(CONTENT_TYPE, "application/octet-stream")
                            .header(CONTENT_LENGTH, length)
                            .body(
                                Self::part(&upload, offset, length)
                                    .unwrap_or_else(|_| Body::from(Vec::new())),
                            )
                            .timeout(Duration::from_secs(3600))
                    })
                };
                if size <= CHUNK {
                    return post("files/upload", commit, 0, size).map(drop);
                }
                let started: Value = post(
                    "files/upload_session/start",
                    json!({"close": false}),
                    0,
                    CHUNK,
                )?
                .json()
                .map_err(|_| "Dropbox sent an unreadable upload reply.")?;
                let session = started["session_id"]
                    .as_str()
                    .ok_or("Dropbox did not start the upload.")?
                    .to_string();
                let mut offset = CHUNK;
                while size - offset > CHUNK {
                    post(
                        "files/upload_session/append_v2",
                        json!({"cursor": {"session_id": session, "offset": offset}, "close": false}),
                        offset,
                        CHUNK,
                    )?;
                    offset += CHUNK;
                }
                post(
                    "files/upload_session/finish",
                    json!({"cursor": {"session_id": session, "offset": offset}, "commit": commit}),
                    offset,
                    size - offset,
                )
                .map(drop)
            }
            Provider::OneDrive => {
                const SMALL: u64 = 4 * 1024 * 1024;
                const CHUNK: u64 = 32 * 320 * 1024;
                if size <= SMALL {
                    let url = self.onedrive_item(&path, "/content");
                    return self
                        .call(&action, |token| {
                            self.http
                                .put(&url)
                                .bearer_auth(token)
                                .header(CONTENT_LENGTH, size)
                                .body(
                                    Self::part(&upload, 0, size)
                                        .unwrap_or_else(|_| Body::from(Vec::new())),
                                )
                        })
                        .map(drop);
                }
                let url = self.onedrive_item(&path, "/createUploadSession");
                let body = json!({"item": {"@microsoft.graph.conflictBehavior": "replace"}});
                let session = self.json(&action, |token| {
                    self.http.post(&url).bearer_auth(token).json(&body)
                })?;
                let upload_url = session["uploadUrl"]
                    .as_str()
                    .ok_or("OneDrive did not start the upload.")?
                    .to_string();
                let mut offset = 0;
                while offset < size {
                    let length = CHUNK.min(size - offset);
                    // The upload URL carries its own authorization.
                    let response = self
                        .http
                        .put(&upload_url)
                        .header(CONTENT_LENGTH, length)
                        .header(
                            CONTENT_RANGE,
                            format!("bytes {offset}-{}/{size}", offset + length - 1),
                        )
                        .body(Self::part(&upload, offset, length)?)
                        .timeout(Duration::from_secs(3600))
                        .send()
                        .map_err(|error| format!("Could not {action}: {error}"))?;
                    self.checked(response, &action)?;
                    offset += length;
                }
                Ok(())
            }
            Provider::GoogleDrive => {
                let (parent, name) = split(&path);
                let existing = self.drive_item(&path).ok();
                let location = match &existing {
                    Some((id, _)) => {
                        let url = format!("{}/files/{id}", self.endpoints.content);
                        self.call(&action, |token| {
                            self.http
                                .patch(&url)
                                .bearer_auth(token)
                                .query(&[("uploadType", "resumable")])
                                .json(&json!({}))
                        })?
                    }
                    None => {
                        let (parent_id, _) = self.drive_item(&parent)?;
                        let url = format!("{}/files", self.endpoints.content);
                        let body = json!({"name": name, "parents": [parent_id]});
                        self.call(&action, |token| {
                            self.http
                                .post(&url)
                                .bearer_auth(token)
                                .query(&[("uploadType", "resumable")])
                                .json(&body)
                        })?
                    }
                }
                .headers()
                .get("Location")
                .and_then(|value| value.to_str().ok())
                .ok_or("Google Drive did not start the upload.")?
                .to_string();
                let response = self
                    .http
                    .put(&location)
                    .bearer_auth(self.access(false)?)
                    .header(CONTENT_LENGTH, size)
                    .body(Self::part(&upload, 0, size)?)
                    .timeout(Duration::from_secs(3600))
                    .send()
                    .map_err(|error| format!("Could not {action}: {error}"))?;
                let file: Value = self
                    .checked(response, &action)?
                    .json()
                    .unwrap_or(Value::Null);
                if let (Some(id), Ok(mut ids)) = (file["id"].as_str(), self.ids.lock()) {
                    ids.insert(
                        path,
                        (
                            id.to_string(),
                            file["mimeType"].as_str().unwrap_or_default().to_string(),
                        ),
                    );
                }
                Ok(())
            }
        }
    }

    fn account(&self) -> Result<String, String> {
        let value = match self.provider {
            Provider::Dropbox => {
                let url = format!("{}/users/get_current_account", self.endpoints.api);
                self.json("read the Dropbox account", |token| {
                    self.http
                        .post(&url)
                        .bearer_auth(token)
                        .header(CONTENT_TYPE, "application/json")
                        .body("null")
                })?["email"]
                    .clone()
            }
            Provider::OneDrive => {
                let url = format!("{}/me", self.endpoints.api);
                let me = self.json("read the Microsoft account", |token| {
                    self.http.get(&url).bearer_auth(token)
                })?;
                if me["mail"].is_string() {
                    me["mail"].clone()
                } else {
                    me["userPrincipalName"].clone()
                }
            }
            Provider::GoogleDrive => {
                let url = format!("{}/about", self.endpoints.api);
                self.json("read the Google account", |token| {
                    self.http
                        .get(&url)
                        .bearer_auth(token)
                        .query(&[("fields", "user(emailAddress)")])
                })?["user"]["emailAddress"]
                    .clone()
            }
        };
        Ok(value.as_str().unwrap_or("signed-in").to_string())
    }
}

fn token_reply(value: &Value, previous_refresh: Option<String>) -> Result<Tokens, String> {
    let access = value["access_token"]
        .as_str()
        .ok_or("The sign-in server did not return an access token.")?;
    Ok(Tokens {
        access: access.to_string(),
        refresh: value["refresh_token"]
            .as_str()
            .map(str::to_string)
            .or(previous_refresh),
        expires_at: now() + value["expires_in"].as_u64().unwrap_or(3600),
    })
}

fn authorize_url(
    provider: Provider,
    endpoints: &Endpoints,
    app: &AppRegistration,
    challenge: &str,
    state: &str,
) -> String {
    let mut url = reqwest::Url::parse(&endpoints.auth).expect("valid authorization URL");
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &app.client_id)
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state);
        match provider {
            Provider::Dropbox => {
                query.append_pair("token_access_type", "offline");
            }
            Provider::GoogleDrive => {
                query
                    .append_pair("scope", "https://www.googleapis.com/auth/drive")
                    .append_pair("access_type", "offline")
                    .append_pair("prompt", "consent");
            }
            Provider::OneDrive => {
                query
                    .append_pair("scope", "offline_access Files.ReadWrite.All User.Read")
                    .append_pair("response_mode", "query");
            }
        }
    }
    url.into()
}

/// Waits for the browser to come back to the loopback address with a code.
pub(crate) fn wait_for_code(
    listener: &TcpListener,
    expected_state: &str,
    timeout: Duration,
) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let started = Instant::now();
    loop {
        let (mut stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if started.elapsed() > timeout {
                    return Err("Sign-in timed out. Start it again and finish it in the browser within five minutes.".into());
                }
                std::thread::sleep(Duration::from_millis(150));
                continue;
            }
            Err(error) => return Err(format!("Sign-in stopped: {error}")),
        };
        stream.set_nonblocking(false).ok();
        stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
        let mut buffer = [0_u8; 8192];
        let count = stream.read(&mut buffer).unwrap_or(0);
        let request = String::from_utf8_lossy(&buffer[..count]).to_string();
        let target = request.split_whitespace().nth(1).unwrap_or("/");
        let url = reqwest::Url::parse(&format!("http://localhost{target}"))
            .map_err(|_| "The browser sent an unreadable sign-in reply.")?;
        let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
        if params.is_empty() {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            continue;
        }
        let outcome = if let Some(error) = params.get("error") {
            Err(if error == "access_denied" {
                "Sign-in was cancelled in the browser.".to_string()
            } else {
                format!("The provider refused the sign-in: {error}")
            })
        } else if params.get("state").map(String::as_str) != Some(expected_state) {
            Err(
                "The sign-in reply did not match this request. Start the sign-in again."
                    .to_string(),
            )
        } else {
            params
                .get("code")
                .cloned()
                .ok_or_else(|| "The sign-in reply had no code.".to_string())
        };
        let page = if outcome.is_ok() {
            "You are signed in. You can close this tab and return to Weave."
        } else {
            "Sign-in did not finish. Return to Weave for details."
        };
        let body = format!("<!doctype html><meta charset=utf-8><title>Weave</title><body style=\"font:15px -apple-system;padding:40px\">{page}</body>");
        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        return outcome;
    }
}

fn exchange_code(
    endpoints: &Endpoints,
    app: &AppRegistration,
    code: &str,
    verifier: &str,
) -> Result<Tokens, String> {
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", REDIRECT_URI.to_string()),
        ("client_id", app.client_id.clone()),
        ("code_verifier", verifier.to_string()),
    ];
    if let Some(secret) = &app.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let response = Client::new()
        .post(&endpoints.token)
        .form(&form)
        .timeout(Duration::from_secs(30))
        .send()
        .map_err(|error| format!("Could not finish signing in: {error}"))?;
    let status = response.status();
    let value: Value = response.json().unwrap_or(Value::Null);
    if !status.is_success() {
        let detail = value["error_description"]
            .as_str()
            .or(value["error"].as_str())
            .unwrap_or("no details");
        return Err(format!("The provider refused the sign-in: {detail}. Check the client ID, secret and redirect URI {REDIRECT_URI}."));
    }
    token_reply(&value, None)
}

fn pkce() -> (String, String) {
    let verifier = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudApp {
    client_id: String,
    has_secret: bool,
    redirect_uri: &'static str,
}

#[tauri::command]
pub fn cloud_app_get(provider: String) -> Result<CloudApp, String> {
    let provider =
        Provider::from_protocol(&provider).ok_or("Choose Dropbox, Google Drive or OneDrive.")?;
    let app = read_json::<AppRegistration>(&app_account(provider))?.unwrap_or_default();
    Ok(CloudApp {
        client_id: app.client_id,
        has_secret: app.client_secret.is_some(),
        redirect_uri: REDIRECT_URI,
    })
}

#[tauri::command]
pub fn cloud_app_set(
    provider: String,
    client_id: String,
    client_secret: Option<String>,
) -> Result<(), String> {
    let provider =
        Provider::from_protocol(&provider).ok_or("Choose Dropbox, Google Drive or OneDrive.")?;
    let client_id = client_id.trim().to_string();
    if client_id.is_empty() || client_id.len() > 300 || client_id.chars().any(char::is_whitespace) {
        return Err(
            "Enter the client ID (app key) from your app registration, without spaces.".into(),
        );
    }
    let existing = read_json::<AppRegistration>(&app_account(provider))?.unwrap_or_default();
    let client_secret = match client_secret.map(|secret| secret.trim().to_string()) {
        Some(secret) if !secret.is_empty() => Some(secret),
        Some(_) => None,
        None => existing
            .client_secret
            .filter(|_| existing.client_id == client_id),
    };
    write_json(
        &app_account(provider),
        &AppRegistration {
            client_id,
            client_secret,
        },
    )
}

#[tauri::command]
pub fn cloud_signed_in(profile_id: String) -> Result<bool, String> {
    Ok(read_json::<Tokens>(&tokens_account(&profile_id))?.is_some())
}

#[tauri::command]
pub fn cloud_sign_out(profile_id: String) -> Result<(), String> {
    match keychain(&tokens_account(&profile_id))?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(format!("Could not remove the sign-in: {error}")),
    }
}

/// Removes a deleted host's sign-in from the Keychain.
pub(crate) fn forget_sign_in(profile_id: &str) {
    let _ = cloud_sign_out(profile_id.to_string());
}

/// Opens the provider's sign-in page and waits for the browser to return.
/// Returns the account's email address, which becomes the profile's user.
#[tauri::command]
pub async fn cloud_sign_in(app: AppHandle, profile_id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let profile = profiles::get_profile(&state, &profile_id)?;
        let provider = Provider::from_protocol(&profile.protocol).ok_or("This is not a cloud drive connection.")?;
        let registration = read_json::<AppRegistration>(&app_account(provider))?
            .ok_or_else(|| format!("Save your {} app's client ID first.", provider.name()))?;
        let endpoints = Endpoints::real(provider);
        let listener = TcpListener::bind(("127.0.0.1", REDIRECT_PORT))
            .map_err(|_| format!("Port {REDIRECT_PORT} is in use, maybe by another sign-in. Finish or close it, then try again."))?;
        let (verifier, challenge) = pkce();
        let request_state = uuid::Uuid::new_v4().simple().to_string();
        let url = authorize_url(provider, &endpoints, &registration, &challenge, &request_state);
        crate::platform::open_url(&url)?;
        let code = wait_for_code(&listener, &request_state, Duration::from_secs(300))?;
        let tokens = exchange_code(&endpoints, &registration, &code, &verifier)?;
        write_json(&tokens_account(&profile_id), &tokens)?;
        let client = CloudClient::with(provider, endpoints, registration, tokens, Some(profile_id.clone()))?;
        let email = client.account()?;
        let db = state.db.lock().map_err(|_| "Connections are busy. Try again.")?;
        db.execute("UPDATE profiles SET username=?2 WHERE id=?1", [&profile_id, &email])
            .map_err(|error| format!("Signed in, but could not save the account name: {error}"))?;
        Ok(email)
    })
    .await
    .map_err(|error| format!("Sign-in stopped unexpectedly: {error}"))?
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{net::TcpStream, sync::Arc, thread};

    pub(crate) type Handler = Arc<
        dyn Fn(&str, &str, &HashMap<String, String>, &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>)
            + Send
            + Sync,
    >;

    fn handle(mut stream: TcpStream, handler: Handler) {
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut pending = Vec::new();
        loop {
            let end = loop {
                if let Some(end) = pending.windows(4).position(|window| window == b"\r\n\r\n") {
                    break end;
                }
                let mut buffer = [0_u8; 65536];
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => return,
                    Ok(count) => pending.extend_from_slice(&buffer[..count]),
                }
            };
            let head = String::from_utf8_lossy(&pending[..end]).to_string();
            pending.drain(..end + 4);
            let mut lines = head.lines();
            let mut first = lines.next().unwrap_or("").split_whitespace();
            let (method, target) = (
                first.next().unwrap_or("").to_string(),
                first.next().unwrap_or("").to_string(),
            );
            let headers: HashMap<String, String> = lines
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
                .collect();
            let length: usize = headers
                .get("content-length")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            while pending.len() < length {
                let mut buffer = [0_u8; 65536];
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => return,
                    Ok(count) => pending.extend_from_slice(&buffer[..count]),
                }
            }
            let body: Vec<u8> = pending.drain(..length).collect();
            let (status, extra, reply) = handler(&method, &target, &headers, &body);
            let mut response =
                format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", reply.len());
            for (name, value) in extra {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            if stream
                .write_all(response.as_bytes())
                .and_then(|_| stream.write_all(&reply))
                .is_err()
            {
                return;
            }
        }
    }

    pub(crate) fn serve(handler: Handler) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let handler = handler.clone();
                thread::spawn(move || handle(stream, handler));
            }
        });
        base
    }

    fn client(provider: Provider, base: &str, access: &str) -> CloudClient {
        CloudClient::with(
            provider,
            Endpoints::at(base),
            AppRegistration {
                client_id: "app".into(),
                client_secret: None,
            },
            Tokens {
                access: access.into(),
                refresh: Some("refresh-1".into()),
                expires_at: now() + 3600,
            },
            None,
        )
        .unwrap()
    }

    fn reply(value: Value) -> (u16, Vec<(String, String)>, Vec<u8>) {
        (
            200,
            vec![("Content-Type".into(), "application/json".into())],
            value.to_string().into_bytes(),
        )
    }

    fn path_of(target: &str) -> String {
        let path = target.split('?').next().unwrap_or("");
        percent_encoding::percent_decode_str(path)
            .decode_utf8_lossy()
            .to_string()
    }

    #[test]
    fn works_with_dropbox_and_renews_an_expired_token() {
        let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::from([(
            "/Docs/é.txt".to_string(),
            b"hello world".to_vec(),
        )])));
        let store = files.clone();
        let base = serve(Arc::new(move |method, target, headers, body| {
            let path = path_of(target);
            if path == "/token" {
                assert!(String::from_utf8_lossy(body).contains("refresh_token=refresh-1"));
                return reply(json!({"access_token": "fresh", "expires_in": 14400}));
            }
            if headers.get("authorization").map(String::as_str) != Some("Bearer fresh") {
                return (401, vec![], b"{}".to_vec());
            }
            let mut files = store.lock().unwrap();
            let arg: Value = headers
                .get("dropbox-api-arg")
                .map(|text| serde_json::from_str(text).unwrap())
                .unwrap_or_else(|| serde_json::from_slice(body).unwrap_or(Value::Null));
            match (method, path.as_str()) {
                ("POST", "/api/files/list_folder") => {
                    let folder = arg["path"].as_str().unwrap().to_string();
                    let entries: Vec<Value> = files
                        .iter()
                        .filter(|(key, _)| split(key).0 == normalize(&folder))
                        .map(|(key, data)| json!({".tag": "file", "name": split(key).1, "size": data.len(), "server_modified": "2026-01-02T03:04:05Z"}))
                        .chain((folder.is_empty()).then(|| json!({".tag": "folder", "name": "Docs"})))
                        .collect();
                    reply(json!({"entries": entries, "has_more": false, "cursor": "c"}))
                }
                ("POST", "/content/files/upload") => {
                    assert!(headers["dropbox-api-arg"].is_ascii());
                    files.insert(arg["path"].as_str().unwrap().into(), body.to_vec());
                    reply(json!({}))
                }
                ("POST", "/content/files/download") => {
                    match files.get(arg["path"].as_str().unwrap()) {
                        Some(data) => match headers.get("range") {
                            Some(range) => {
                                let start: usize = range
                                    .trim_start_matches("bytes=")
                                    .trim_end_matches('-')
                                    .parse()
                                    .unwrap();
                                (206, vec![], data[start..].to_vec())
                            }
                            None => (200, vec![], data.clone()),
                        },
                        None => (
                            409,
                            vec![],
                            br#"{"error_summary":"path/not_found/"}"#.to_vec(),
                        ),
                    }
                }
                ("POST", "/api/files/move_v2") => {
                    let data = files.remove(arg["from_path"].as_str().unwrap()).unwrap();
                    files.insert(arg["to_path"].as_str().unwrap().into(), data);
                    reply(json!({}))
                }
                ("POST", "/api/files/get_metadata") => {
                    match files.get(arg["path"].as_str().unwrap()) {
                        Some(data) => reply(json!({".tag": "file", "size": data.len()})),
                        None => (
                            409,
                            vec![],
                            br#"{"error_summary":"path/not_found/.."}"#.to_vec(),
                        ),
                    }
                }
                ("POST", "/api/files/delete_v2") => {
                    files.remove(arg["path"].as_str().unwrap());
                    reply(json!({}))
                }
                _ => (404, vec![], vec![]),
            }
        }));
        let client = client(Provider::Dropbox, &base, "expired");
        let root = client.list("/").unwrap();
        assert_eq!(root.entries[0].name, "Docs");
        assert!(root.entries[0].is_dir);
        let docs = client.list("/Docs").unwrap();
        assert_eq!(
            (docs.entries[0].name.as_str(), docs.entries[0].size),
            ("é.txt", 11)
        );
        assert_eq!(docs.parent.as_deref(), Some("/"));
        let mut read = String::new();
        client
            .open_read("/Docs/é.txt", 6)
            .unwrap()
            .read_to_string(&mut read)
            .unwrap();
        assert_eq!(read, "world");
        client.write_small("/Docs/new ü.txt", b"uploaded").unwrap();
        client.rename("/Docs/new ü.txt", "/moved.txt").unwrap();
        assert_eq!(files.lock().unwrap()["/moved.txt"], b"uploaded");
        client.delete("/moved.txt", false).unwrap();
        let missing = client.stat("/moved.txt").unwrap_err();
        assert!(missing.contains("does not exist"), "{missing}");
    }

    #[test]
    fn works_with_onedrive_including_upload_sessions() {
        let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let store = files.clone();
        let base = Arc::new(Mutex::new(String::new()));
        let own_base = base.clone();
        let address = serve(Arc::new(move |method, target, headers, body| {
            let path = path_of(target);
            let mut files = store.lock().unwrap();
            if path.starts_with("/session/") {
                assert!(headers.get("authorization").is_none());
                let name = path.trim_start_matches("/session");
                let range = headers["content-range"].clone();
                let entry = files.entry(name.to_string()).or_default();
                assert!(range.starts_with(&format!("bytes {}-", entry.len())));
                entry.extend_from_slice(body);
                return (
                    if range.ends_with(&format!("/{}", 5 * 1024 * 1024))
                        && entry.len() == 5 * 1024 * 1024
                    {
                        201
                    } else {
                        202
                    },
                    vec![],
                    b"{}".to_vec(),
                );
            }
            assert_eq!(
                headers.get("authorization").map(String::as_str),
                Some("Bearer token")
            );
            let item = path.trim_start_matches("/api/me/drive/root");
            match (method, item) {
                ("GET", "/children") => reply(
                    json!({"value": [{"name": "Photos", "folder": {}}, {"name": "a.txt", "size": 3, "file": {}, "lastModifiedDateTime": "2026-02-01T00:00:00Z"}]}),
                ),
                ("PUT", item) if item.ends_with(":/content") => {
                    files.insert(
                        item.trim_start_matches(':')
                            .trim_end_matches(":/content")
                            .to_string(),
                        body.to_vec(),
                    );
                    reply(json!({}))
                }
                ("POST", item) if item.ends_with(":/createUploadSession") => {
                    let name = item
                        .trim_start_matches(':')
                        .trim_end_matches(":/createUploadSession");
                    reply(
                        json!({"uploadUrl": format!("{}/session{name}", own_base.lock().unwrap())}),
                    )
                }
                ("POST", ":/Photos:/children") => {
                    let body: Value = serde_json::from_slice(body).unwrap();
                    assert_eq!(body["@microsoft.graph.conflictBehavior"], "fail");
                    reply(json!({}))
                }
                ("PATCH", ":/a.txt:") => {
                    let body: Value = serde_json::from_slice(body).unwrap();
                    assert_eq!(body["parentReference"]["path"], "/drive/root:/Photos");
                    reply(json!({}))
                }
                _ => (
                    404,
                    vec![],
                    br#"{"error":{"message":"itemNotFound"}}"#.to_vec(),
                ),
            }
        }));
        *base.lock().unwrap() = address.clone();
        let client = client(Provider::OneDrive, &address, "token");
        let root = client.list("/").unwrap();
        assert_eq!(
            root.entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["Photos", "a.txt"]
        );
        assert_eq!(root.entries[1].modified, Some(1_769_904_000));
        client.write_small("/small.txt", b"tiny").unwrap();
        let big: Vec<u8> = (0..5 * 1024 * 1024)
            .map(|index| (index % 7) as u8)
            .collect();
        client.write_small("/big.bin", &big).unwrap();
        assert_eq!(files.lock().unwrap()["/small.txt"], b"tiny");
        assert_eq!(files.lock().unwrap()["/big.bin"], big);
        client.mkdir("/Photos/2026").unwrap();
        client.rename("/a.txt", "/Photos/a.txt").unwrap();
        assert!(client
            .stat("/missing")
            .unwrap_err()
            .contains("does not exist"));
    }

    #[test]
    fn works_with_google_drive_by_resolving_paths_to_ids() {
        let created: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let log = created.clone();
        let base = Arc::new(Mutex::new(String::new()));
        let own_base = base.clone();
        let address = serve(Arc::new(move |method, target, _headers, body| {
            let path = path_of(target);
            let query: HashMap<String, String> = reqwest::Url::parse(&format!("http://x{target}"))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
            match (method, path.as_str()) {
                ("GET", "/api/files") => {
                    let q = query["q"].clone();
                    if q == "'root' in parents and trashed = false" {
                        reply(
                            json!({"files": [{"id": "f1", "name": "Work", "mimeType": FOLDER_MIME}, {"id": "d1", "name": "Plan", "mimeType": "application/vnd.google-apps.document"}]}),
                        )
                    } else if q == "'root' in parents and name = 'Work' and trashed = false" {
                        reply(json!({"files": [{"id": "f1", "mimeType": FOLDER_MIME}]}))
                    } else if q == "'f1' in parents and name = 'it\\'s.txt' and trashed = false" {
                        reply(json!({"files": [{"id": "t1", "mimeType": "text/plain"}]}))
                    } else if q.contains("name = ") {
                        reply(json!({"files": []}))
                    } else if q == "'f1' in parents and trashed = false"
                        && !query.contains_key("pageToken")
                    {
                        reply(
                            json!({"files": [{"id": "t1", "name": "it's.txt", "mimeType": "text/plain", "size": "5"}], "nextPageToken": "p2"}),
                        )
                    } else {
                        reply(
                            json!({"files": [{"id": "t2", "name": "second.txt", "mimeType": "text/plain", "size": "1"}]}),
                        )
                    }
                }
                ("GET", "/api/files/t1") => {
                    assert_eq!(query["alt"], "media");
                    (200, vec![], b"hello".to_vec())
                }
                ("POST", "/content/files") => {
                    assert_eq!(query["uploadType"], "resumable");
                    log.lock()
                        .unwrap()
                        .push(serde_json::from_slice(body).unwrap());
                    (
                        200,
                        vec![(
                            "Location".into(),
                            format!("{}/upload-1", own_base.lock().unwrap()),
                        )],
                        vec![],
                    )
                }
                ("PUT", "/upload-1") => reply(
                    json!({"id": "n1", "mimeType": "text/plain", "body": String::from_utf8_lossy(body)}),
                ),
                ("PATCH", "/api/files/t1") => {
                    log.lock().unwrap().push(json!({"patch": serde_json::from_slice::<Value>(body).unwrap(), "query": query}));
                    reply(json!({}))
                }
                _ => (404, vec![], vec![]),
            }
        }));
        *base.lock().unwrap() = address.clone();
        let client = client(Provider::GoogleDrive, &address, "token");
        let root = client.list("/").unwrap();
        assert!(root.entries[0].is_dir && root.entries[0].name == "Work");
        let work = client.list("/Work").unwrap();
        assert_eq!(work.entries.len(), 2);
        let mut text = String::new();
        client
            .open_read("/Work/it's.txt", 0)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "hello");
        assert!(client
            .open_read("/Plan", 0)
            .unwrap_err()
            .contains("Google Docs"));
        client.write_small("/Work/new.txt", b"fresh").unwrap();
        assert_eq!(
            created.lock().unwrap()[0],
            json!({"name": "new.txt", "parents": ["f1"]})
        );
        client.rename("/Work/it's.txt", "/renamed.txt").unwrap();
        let patched = created.lock().unwrap()[1].clone();
        assert_eq!(patched["patch"]["name"], "renamed.txt");
        assert_eq!(
            (
                patched["query"]["addParents"].as_str(),
                patched["query"]["removeParents"].as_str()
            ),
            (Some("root"), Some("f1"))
        );
    }

    #[test]
    fn builds_sign_in_links_and_reads_the_browser_reply() {
        let app = AppRegistration {
            client_id: "my app".into(),
            client_secret: None,
        };
        let url = authorize_url(
            Provider::GoogleDrive,
            &Endpoints::real(Provider::GoogleDrive),
            &app,
            "challenge",
            "state-1",
        );
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(
            url.contains("client_id=my+app")
                && url.contains("code_challenge_method=S256")
                && url.contains("access_type=offline")
        );
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A53682%2F"));
        let (verifier, challenge) = pkce();
        assert!(verifier.len() >= 43 && challenge.len() == 43);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream
                .write_all(b"GET /?code=abc&state=state-1 HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .unwrap();
            let mut page = String::new();
            stream.read_to_string(&mut page).unwrap();
            page
        });
        assert_eq!(
            wait_for_code(&listener, "state-1", Duration::from_secs(5)).unwrap(),
            "abc"
        );
        assert!(browser.join().unwrap().contains("You are signed in"));
        let browser = thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream
                .write_all(b"GET /?error=access_denied&state=state-2 HTTP/1.1\r\n\r\n")
                .unwrap();
        });
        assert!(wait_for_code(&listener, "state-2", Duration::from_secs(5))
            .unwrap_err()
            .contains("cancelled"));
        browser.join().unwrap();
        assert_eq!(
            header_json(&json!({"path": "/é"})),
            "{\"path\":\"/\\u00e9\"}"
        );
    }
}
