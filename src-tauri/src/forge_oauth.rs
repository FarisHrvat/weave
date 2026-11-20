//! Browser sign-in for GitHub and GitLab with the OAuth device flow
//! (RFC 8628). The user registers an OAuth app with device flow enabled and
//! enters its client ID; no client secret is needed. GitLab access tokens
//! expire, so their refresh token is kept and used before each request.

use keyring::{Entry, Error as KeyringError};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Service {
    GitHub,
    GitLab,
    /// Microsoft Entra sign-in for Azure DevOps.
    Azure,
}

impl Service {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "GitHub" => Ok(Self::GitHub),
            "GitLab" => Ok(Self::GitLab),
            "Azure DevOps" => Ok(Self::Azure),
            _ => Err("Choose GitHub, GitLab or Azure DevOps.".into()),
        }
    }

    pub(crate) fn base(self) -> &'static str {
        match self {
            Self::GitHub => "https://github.com",
            Self::GitLab => "https://gitlab.com",
            Self::Azure => "https://login.microsoftonline.com/organizations/oauth2/v2.0",
        }
    }

    fn device_path(self) -> &'static str {
        match self {
            Self::GitHub => "/login/device/code",
            Self::GitLab => "/oauth/authorize_device",
            Self::Azure => "/devicecode",
        }
    }

    fn token_path(self) -> &'static str {
        match self {
            Self::GitHub => "/login/oauth/access_token",
            Self::GitLab => "/oauth/token",
            Self::Azure => "/token",
        }
    }

    fn scope(self) -> &'static str {
        match self {
            Self::GitHub => "repo read:org workflow",
            Self::GitLab => "api read_user write_repository",
            // Azure DevOps' resource ID, with a refresh token.
            Self::Azure => "499b84ac-1321-427f-aa17-267ca6975798/.default offline_access",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStart {
    id: String,
    user_code: String,
    verification_uri: String,
    interval: u64,
    expires_in: u64,
}

struct Pending {
    service: Service,
    client_id: String,
    device_code: Zeroizing<String>,
    expires: Instant,
}

fn pending() -> &'static Mutex<HashMap<String, Pending>> {
    static PENDING: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}

fn client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("Could not prepare the sign-in request: {error}"))
}

pub(crate) fn post_form(url: &str, form: &[(&str, &str)]) -> Result<Value, String> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    let response = client()?
        .post(url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .map_err(|error| format!("Could not reach the sign-in service: {error}"))?;
    let status = response.status();
    let value: Value = response.json().unwrap_or(Value::Null);
    if !status.is_success() && value.get("error").is_none() {
        return Err(format!(
            "The sign-in service returned HTTP {status}. Check the client ID."
        ));
    }
    Ok(value)
}

fn valid_client_id(client_id: &str) -> bool {
    !client_id.is_empty()
        && client_id.len() <= 200
        && client_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

pub(crate) fn start_at(
    service: Service,
    client_id: &str,
    base: &str,
) -> Result<DeviceStart, String> {
    if !valid_client_id(client_id) {
        return Err("Enter the client ID of your OAuth app.".into());
    }
    let value = post_form(
        &format!("{base}{}", service.device_path()),
        &[("client_id", client_id), ("scope", service.scope())],
    )?;
    if let Some(error) = value["error"].as_str() {
        return Err(match error {
            "device_flow_disabled" => {
                "Device flow is turned off for this OAuth app. Enable it in the app's settings."
                    .into()
            }
            "invalid_client" | "unauthorized_client" => {
                "The client ID was not accepted. Check it and that the app allows device sign-in."
                    .into()
            }
            other => format!("The sign-in service refused the request: {other}"),
        });
    }
    let text = |key: &str| value[key].as_str().map(str::to_string);
    let device_code =
        text("device_code").ok_or("The sign-in service did not return a device code.")?;
    let expires_in = value["expires_in"].as_u64().unwrap_or(900);
    let id = uuid::Uuid::new_v4().to_string();
    pending()
        .lock()
        .map_err(|_| "Sign-in is busy. Try again.")?
        .insert(
            id.clone(),
            Pending {
                service,
                client_id: client_id.into(),
                device_code: Zeroizing::new(device_code),
                expires: Instant::now() + Duration::from_secs(expires_in),
            },
        );
    Ok(DeviceStart {
        id,
        user_code: text("user_code").unwrap_or_default(),
        verification_uri: text("verification_uri_complete")
            .or_else(|| text("verification_uri"))
            .ok_or("The sign-in service did not return a verification page.")?,
        interval: value["interval"].as_u64().unwrap_or(5).max(1),
        expires_in,
    })
}

pub(crate) struct Tokens {
    pub access: Zeroizing<String>,
    pub refresh: Option<Zeroizing<String>>,
    pub expires_in: Option<u64>,
}

fn tokens_from(value: &Value) -> Option<Tokens> {
    Some(Tokens {
        access: Zeroizing::new(value["access_token"].as_str()?.to_string()),
        refresh: value["refresh_token"]
            .as_str()
            .map(|token| Zeroizing::new(token.to_string())),
        expires_in: value["expires_in"].as_u64(),
    })
}

/// `Ok(None)` while the user has not finished signing in.
pub(crate) fn poll_at(id: &str, base: &str) -> Result<Option<(Service, Tokens)>, String> {
    let (service, client_id, device_code) = {
        let map = pending()
            .lock()
            .map_err(|_| "Sign-in is busy. Try again.")?;
        let item = map.get(id).ok_or("This sign-in has ended. Start again.")?;
        if Instant::now() > item.expires {
            return Err("The code expired before sign-in finished. Start again.".into());
        }
        (
            item.service,
            item.client_id.clone(),
            item.device_code.clone(),
        )
    };
    let value = post_form(
        &format!("{base}{}", service.token_path()),
        &[
            ("client_id", &client_id),
            ("device_code", &device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ],
    )?;
    let finish = || {
        if let Ok(mut map) = pending().lock() {
            map.remove(id);
        }
    };
    match value["error"].as_str() {
        Some("authorization_pending" | "slow_down") => Ok(None),
        Some("access_denied") => {
            finish();
            Err("Sign-in was cancelled in the browser.".into())
        }
        Some("expired_token") => {
            finish();
            Err("The code expired before sign-in finished. Start again.".into())
        }
        Some(other) => {
            finish();
            Err(format!("Sign-in failed: {other}"))
        }
        None => {
            finish();
            tokens_from(&value)
                .map(|tokens| Some((service, tokens)))
                .ok_or_else(|| "The sign-in service did not return a token.".into())
        }
    }
}

#[derive(Serialize, Deserialize)]
struct RefreshState {
    client_id: String,
    refresh_token: String,
    expires_at: i64,
}

fn refresh_entry(host: &str) -> Result<Entry, String> {
    Entry::new("app.weave.desktop", &format!("forge:oauth-refresh:{host}"))
        .map_err(|error| format!("Could not open the system keychain: {error}"))
}

pub(crate) fn remember_refresh(host: &str, client_id: &str, tokens: &Tokens) -> Result<(), String> {
    let entry = refresh_entry(host)?;
    match (&tokens.refresh, tokens.expires_in) {
        (Some(refresh), Some(expires_in)) => {
            let state = RefreshState {
                client_id: client_id.into(),
                refresh_token: refresh.to_string(),
                expires_at: chrono::Utc::now().timestamp() + expires_in as i64,
            };
            entry
                .set_password(&serde_json::to_string(&state).map_err(|error| error.to_string())?)
                .map_err(|error| {
                    format!("Could not save the sign-in in the system keychain: {error}")
                })
        }
        _ => forget_refresh(host),
    }
}

pub(crate) fn forget_refresh(host: &str) -> Result<(), String> {
    match refresh_entry(host)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(format!("Could not update the system keychain: {error}")),
    }
}

/// Renews an expiring OAuth access token stored in `account`.
pub(crate) fn refresh_if_needed(host: &str, account: &Entry) -> Result<(), String> {
    let saved = match refresh_entry(host)?.get_password() {
        Ok(saved) => saved,
        Err(KeyringError::NoEntry) => return Ok(()),
        Err(error) => return Err(format!("Could not read the system keychain: {error}")),
    };
    let Ok(state) = serde_json::from_str::<RefreshState>(&saved) else {
        return Ok(());
    };
    if state.expires_at - chrono::Utc::now().timestamp() > 120 {
        return Ok(());
    }
    let value = post_form(
        &format!("https://{host}/oauth/token"),
        &[
            ("client_id", &state.client_id),
            ("refresh_token", &state.refresh_token),
            ("grant_type", "refresh_token"),
        ],
    )?;
    let tokens = tokens_from(&value)
        .ok_or("Your GitLab sign-in has expired. Sign in again from the account settings.")?;
    account
        .set_password(&tokens.access)
        .map_err(|error| format!("Could not save the renewed sign-in: {error}"))?;
    remember_refresh(host, &state.client_id, &tokens)
}

#[tauri::command]
pub async fn forge_oauth_start(provider: String, client_id: String) -> Result<DeviceStart, String> {
    let service = Service::parse(&provider)?;
    tauri::async_runtime::spawn_blocking(move || {
        start_at(service, client_id.trim(), service.base())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Returns true once the account is connected.
#[tauri::command]
pub async fn forge_oauth_poll(id: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let service = pending()
            .lock()
            .map_err(|_| "Sign-in is busy. Try again.")?
            .get(&id)
            .map(|item| (item.service, item.client_id.clone()));
        let Some((service, client_id)) = service else {
            return Err("This sign-in has ended. Start again.".into());
        };
        let Some((_, tokens)) = poll_at(&id, service.base())? else {
            return Ok(false);
        };
        let name = match service {
            Service::GitHub => "GitHub",
            Service::GitLab => "GitLab",
            Service::Azure => return Err("Finish Azure DevOps sign-in from its own panel.".into()),
        };
        crate::forge::connect_provider_token(name, tokens.access.to_string())?;
        remember_refresh(
            service.base().trim_start_matches("https://"),
            &client_id,
            &tokens,
        )?;
        Ok(true)
    })
    .await
    .map_err(|error| error.to_string())?
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
                let mut bytes = [0_u8; 4096];
                let mut request = Vec::new();
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
    fn completes_the_github_device_flow() {
        let (base, server) = serve(vec![
            r#"{"device_code":"dev123","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#,
            r#"{"error":"authorization_pending"}"#,
            r#"{"access_token":"gho_token","token_type":"bearer","scope":"repo"}"#,
        ]);
        let start = start_at(Service::GitHub, "Iv1.abc123", &base).unwrap();
        assert_eq!(start.user_code, "ABCD-1234");
        assert_eq!(start.interval, 5);
        assert!(poll_at(&start.id, &base).unwrap().is_none());
        let (service, tokens) = poll_at(&start.id, &base).unwrap().unwrap();
        assert_eq!(service, Service::GitHub);
        assert_eq!(tokens.access.as_str(), "gho_token");
        assert!(poll_at(&start.id, &base).err().unwrap().contains("ended"));
        let requests = server.join().unwrap();
        assert!(requests[0].starts_with("POST /login/device/code HTTP/1.1"));
        assert!(requests[0].ends_with("client_id=Iv1.abc123&scope=repo+read%3Aorg+workflow"));
        assert!(requests[2]
            .contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"));
    }

    #[test]
    fn explains_refusals_and_cancelled_sign_ins() {
        let (base, server) = serve(vec![
            r#"{"error":"device_flow_disabled"}"#,
            r#"{"device_code":"d","user_code":"U","verification_uri_complete":"https://gitlab.com/oauth/device?user_code=U","expires_in":300}"#,
            r#"{"error":"access_denied"}"#,
        ]);
        assert!(start_at(Service::GitHub, "abc", &base)
            .unwrap_err()
            .contains("Enable it"));
        let start = start_at(Service::GitLab, "abc", &base).unwrap();
        assert_eq!(
            start.verification_uri,
            "https://gitlab.com/oauth/device?user_code=U"
        );
        assert_eq!(
            poll_at(&start.id, &base).err().unwrap(),
            "Sign-in was cancelled in the browser."
        );
        server.join().unwrap();
        assert!(start_at(Service::GitHub, "bad id!", &base).is_err());
    }
}
