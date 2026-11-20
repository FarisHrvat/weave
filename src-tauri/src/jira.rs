use crate::AppState;
use keyring::{Entry, Error as KeyringError};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::Method;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{io::Read, time::Duration};
use tauri::State;
use zeroize::Zeroizing;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct JiraConfig {
    site: String,
    project: String,
    email: String,
    /// Set after browser sign-in: the Atlassian site ID for api.atlassian.com.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cloud_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraInfo {
    site: String,
    project: String,
    email: String,
    connected: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraIssue {
    key: String,
    summary: String,
    status: String,
    issue_type: String,
    url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraPage {
    issues: Vec<JiraIssue>,
    next_page_token: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraTransition {
    id: String,
    name: String,
}

/// Jira Cloud sites end in .atlassian.net; anything else is Data Center.
fn is_cloud(config: &JiraConfig) -> bool {
    config.site.ends_with(".atlassian.net")
}

fn validate_config(config: &JiraConfig) -> Result<(), String> {
    if is_cloud(config) {
        let name = config.site.strip_suffix(".atlassian.net").unwrap_or("");
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || name.starts_with('-')
            || name.ends_with('-')
        {
            return Err("Enter your Jira Cloud site hostname, such as team.atlassian.net.".into());
        }
    } else {
        let valid = url::Url::parse(&format!("https://{}", config.site)).is_ok_and(|url| {
            url.host_str()
                .is_some_and(|host| host.contains('.') || host == "localhost")
                && url.username().is_empty()
                && url.query().is_none()
                && url.fragment().is_none()
        }) && !config.site.contains("..")
            && config
                .site
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-:/_".contains(&byte));
        if !valid {
            return Err(
                "Enter your Jira address, such as team.atlassian.net or jira.example.com.".into(),
            );
        }
    }
    if config.project.is_empty()
        || config.project.len() > 30
        || !config
            .project
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err("Enter a Jira project key using uppercase letters and numbers.".into());
    }
    if is_cloud(config)
        && config.cloud_id.is_none()
        && (!config.email.contains('@') || config.email.chars().any(char::is_whitespace))
    {
        return Err("Enter the email address for your Atlassian account.".into());
    }
    Ok(())
}

fn entry(config: &JiraConfig) -> Result<Entry, String> {
    Entry::new(
        "app.weave.desktop",
        &format!("jira:{}:{}", config.site, config.email),
    )
    .map_err(|error| format!("Could not open the system keychain: {error}"))
}

fn saved_config(state: &AppState) -> Result<Option<JiraConfig>, String> {
    let db = state.db.lock().map_err(|_| "Settings database is busy")?;
    let saved: Option<String> = db
        .query_row(
            "SELECT value FROM settings WHERE key='jira:connection'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("Could not read Jira settings: {error}"))?;
    saved
        .map(|saved| {
            serde_json::from_str(&saved)
                .map_err(|_| "Saved Jira settings are invalid. Reconnect Jira.".into())
        })
        .transpose()
}

fn credentials(state: &AppState) -> Result<(JiraConfig, Zeroizing<String>), String> {
    let config = saved_config(state)?.ok_or("Connect Jira before opening issues.")?;
    let saved = entry(&config)?
        .get_password()
        .map_err(|error| match error {
            KeyringError::NoEntry => {
                "The Jira API token is missing from the system keychain. Reconnect Jira.".into()
            }
            _ => format!("Could not read the Jira token from the system keychain: {error}"),
        })?;
    let token = crate::account_oauth::current_token(&saved, |next| {
        entry(&config)?
            .set_password(next)
            .map_err(|error| format!("Could not save the renewed Jira sign-in: {error}"))
    })?;
    Ok((config, token))
}

fn client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("Could not prepare the Jira connection: {error}"))
}

fn request(
    client: &Client,
    method: Method,
    url: &str,
    config: &JiraConfig,
    token: &str,
) -> RequestBuilder {
    let request = client
        .request(method, url)
        .header("Accept", "application/json");
    if is_cloud(config) && config.cloud_id.is_none() {
        request.basic_auth(&config.email, Some(token))
    } else {
        request.bearer_auth(token)
    }
}

fn send(request: RequestBuilder) -> Result<Value, String> {
    let mut response = request
        .send()
        .map_err(|error| format!("Could not reach Jira Cloud: {error}"))?;
    let status = response.status();
    let mut body = String::new();
    (&mut response)
        .take(2 * 1024 * 1024 + 1)
        .read_to_string(&mut body)
        .map_err(|error| format!("Could not read Jira's response: {error}"))?;
    if body.len() > 2 * 1024 * 1024 {
        return Err("Jira returned too much data. Narrow the request and try again.".into());
    }
    if !status.is_success() {
        let reason = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("errorMessages")
                    .and_then(Value::as_array)
                    .and_then(|messages| messages.first())
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| {
                status
                    .canonical_reason()
                    .unwrap_or("request failed")
                    .to_string()
            });
        return Err(format!(
            "Jira returned {}: {reason}. Check the project key, token, and permissions.",
            status.as_u16()
        ));
    }
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&body).map_err(|_| "Jira returned invalid JSON.".into())
}

fn base(config: &JiraConfig) -> String {
    if let Some(id) = &config.cloud_id {
        format!("https://api.atlassian.com/ex/jira/{id}/rest/api/3")
    } else if is_cloud(config) {
        format!("https://{}/rest/api/3", config.site)
    } else {
        format!("https://{}/rest/api/2", config.site)
    }
}

fn parse_issue(value: &Value, config: &JiraConfig) -> Option<JiraIssue> {
    let key = value.get("key")?.as_str()?;
    Some(JiraIssue {
        key: key.into(),
        summary: value
            .pointer("/fields/summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        status: value
            .pointer("/fields/status/name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        issue_type: value
            .pointer("/fields/issuetype/name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        url: format!("https://{}/browse/{key}", config.site),
    })
}

fn search_at(
    config: &JiraConfig,
    token: &str,
    next_page_token: Option<String>,
    api_base: &str,
) -> Result<JiraPage, String> {
    let jql = format!("project = {} ORDER BY updated DESC", config.project);
    if !is_cloud(config) {
        // Data Center pages with startAt; the token carries the next offset.
        let start: u32 = next_page_token
            .as_deref()
            .unwrap_or("0")
            .parse()
            .map_err(|_| "Refresh the issue list.")?;
        let mut url =
            url::Url::parse(&format!("{api_base}/search")).map_err(|_| "Invalid Jira address.")?;
        url.query_pairs_mut()
            .append_pair("jql", &jql)
            .append_pair("startAt", &start.to_string())
            .append_pair("maxResults", "30")
            .append_pair("fields", "summary,status,issuetype");
        let result = send(request(
            &client()?,
            Method::GET,
            url.as_str(),
            config,
            token,
        ))?;
        let issues: Vec<JiraIssue> = result
            .get("issues")
            .and_then(Value::as_array)
            .ok_or("Jira returned an issue list in an unexpected format.")?
            .iter()
            .filter_map(|value| parse_issue(value, config))
            .collect();
        let total = result.get("total").and_then(Value::as_u64).unwrap_or(0);
        let next = start as u64 + issues.len() as u64;
        return Ok(JiraPage {
            next_page_token: (next < total && !issues.is_empty()).then(|| next.to_string()),
            issues,
        });
    }
    let mut body = json!({"jql": jql, "maxResults":30,
        "fields":["summary","status","issuetype"]});
    if let Some(next) = next_page_token {
        body["nextPageToken"] = Value::String(next);
    }
    let result = send(
        request(
            &client()?,
            Method::POST,
            &format!("{api_base}/search/jql"),
            config,
            token,
        )
        .json(&body),
    )?;
    let issues = result
        .get("issues")
        .and_then(Value::as_array)
        .ok_or("Jira returned an issue list in an unexpected format.")?
        .iter()
        .filter_map(|value| parse_issue(value, config))
        .collect();
    let next_page_token = result
        .get("nextPageToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(JiraPage {
        issues,
        next_page_token,
    })
}

#[tauri::command]
pub fn jira_info(state: State<'_, AppState>) -> Result<Option<JiraInfo>, String> {
    let Some(config) = saved_config(&state)? else {
        return Ok(None);
    };
    let connected = match entry(&config)?.get_password() {
        Ok(_) => true,
        Err(KeyringError::NoEntry) => false,
        Err(error) => {
            return Err(format!(
                "Could not read the Jira token from the system keychain: {error}"
            ))
        }
    };
    Ok(Some(JiraInfo {
        site: config.site,
        project: config.project,
        email: if config.cloud_id.is_some() {
            String::new()
        } else {
            config.email
        },
        connected,
    }))
}

#[tauri::command]
pub async fn jira_connect(
    state: State<'_, AppState>,
    site: String,
    project: String,
    email: String,
    token: String,
) -> Result<(), String> {
    let config = JiraConfig {
        site: site
            .trim()
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_string(),
        project: project.trim().to_ascii_uppercase(),
        email: email.trim().to_string(),
        cloud_id: None,
    };
    validate_config(&config)?;
    let token = Zeroizing::new(token.trim().to_string());
    if token.is_empty() {
        return Err("Enter a Jira API token or personal access token.".into());
    }
    let checked = config.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let url = format!("{}/project/{}", base(&checked), checked.project);
        send(request(&client()?, Method::GET, &url, &checked, &token))?;
        entry(&checked)?.set_password(&token).map_err(|error| {
            format!("Could not save the Jira token in the system keychain: {error}")
        })
    })
    .await
    .map_err(|error| format!("Jira connection stopped unexpectedly: {error}"))??;
    let saved = serde_json::to_string(&config)
        .map_err(|error| format!("Could not prepare Jira settings: {error}"))?;
    let db = state.db.lock().map_err(|_| "Settings database is busy")?;
    db.execute("INSERT INTO settings (key,value) VALUES ('jira:connection',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [saved])
        .map_err(|error| format!("The token was saved, but Jira settings could not be saved: {error}"))?;
    Ok(())
}

/// Jira Cloud sign-in through an Atlassian OAuth 2.0 (3LO) app whose
/// callback URL is http://localhost:53682/.
#[tauri::command]
pub async fn jira_oauth_connect(
    state: State<'_, AppState>,
    site: String,
    project: String,
    client_id: String,
    client_secret: String,
) -> Result<(), String> {
    let mut config = JiraConfig {
        site: site
            .trim()
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_string(),
        project: project.trim().to_ascii_uppercase(),
        email: "oauth".into(),
        cloud_id: Some(String::new()),
    };
    if !is_cloud(&config) {
        return Err("Browser sign-in is for Jira Cloud sites (name.atlassian.net). Use a personal access token for Jira Data Center.".into());
    }
    validate_config(&config)?;
    let (client_id, secret) = (
        client_id.trim().to_string(),
        client_secret.trim().to_string(),
    );
    crate::account_oauth::valid_client(&client_id, Some(&secret))?;
    let config = tauri::async_runtime::spawn_blocking(move || {
        let mut authorize = reqwest::Url::parse("https://auth.atlassian.com/authorize")
            .map_err(|error| error.to_string())?;
        authorize
            .query_pairs_mut()
            .append_pair("audience", "api.atlassian.com")
            .append_pair("client_id", &client_id)
            .append_pair(
                "scope",
                "read:jira-work write:jira-work read:jira-user offline_access",
            )
            .append_pair("response_type", "code")
            .append_pair("prompt", "consent");
        let grant = crate::account_oauth::browser_sign_in(
            authorize.as_str(),
            crate::account_oauth::atlassian_grant(&client_id, &secret),
        )?;
        let sites = send(
            client()?
                .get("https://api.atlassian.com/oauth/token/accessible-resources")
                .bearer_auth(&grant.access),
        )?;
        let wanted = format!("https://{}", config.site);
        config.cloud_id = sites
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| {
                item["url"].as_str().map(|url| url.trim_end_matches('/')) == Some(wanted.as_str())
            })
            .and_then(|item| item["id"].as_str().map(str::to_string));
        if config.cloud_id.is_none() {
            return Err(format!(
                "This sign-in has no access to {}. Approve that site on the consent page.",
                config.site
            ));
        }
        let url = format!("{}/project/{}", base(&config), config.project);
        send(request(
            &client()?,
            Method::GET,
            &url,
            &config,
            &grant.access,
        ))?;
        entry(&config)?
            .set_password(&grant.encode())
            .map_err(|error| {
                format!("Could not save the Jira sign-in in the system keychain: {error}")
            })?;
        Ok::<_, String>(config)
    })
    .await
    .map_err(|error| format!("Jira sign-in stopped unexpectedly: {error}"))??;
    let saved = serde_json::to_string(&config)
        .map_err(|error| format!("Could not prepare Jira settings: {error}"))?;
    let db = state.db.lock().map_err(|_| "Settings database is busy")?;
    db.execute("INSERT INTO settings (key,value) VALUES ('jira:connection',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [saved])
        .map_err(|error| format!("Signed in, but Jira settings could not be saved: {error}"))?;
    Ok(())
}

#[tauri::command]
pub fn jira_disconnect(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(config) = saved_config(&state)? {
        match entry(&config)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => {}
            Err(error) => {
                return Err(format!(
                    "Could not remove the Jira token from the system keychain: {error}"
                ))
            }
        }
    }
    state
        .db
        .lock()
        .map_err(|_| "Settings database is busy")?
        .execute("DELETE FROM settings WHERE key='jira:connection'", [])
        .map_err(|error| format!("Could not remove Jira settings: {error}"))?;
    Ok(())
}

#[tauri::command]
pub async fn jira_list(
    state: State<'_, AppState>,
    next_page_token: Option<String>,
) -> Result<JiraPage, String> {
    let (config, token) = credentials(&state)?;
    if next_page_token
        .as_ref()
        .is_some_and(|value| value.len() > 1024)
    {
        return Err("Jira's next-page token is invalid. Refresh the issue list.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        search_at(&config, &token, next_page_token, &base(&config))
    })
    .await
    .map_err(|error| format!("Loading Jira issues stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn jira_create(
    state: State<'_, AppState>,
    summary: String,
    description: String,
    issue_type: String,
) -> Result<JiraIssue, String> {
    let (config, token) = credentials(&state)?;
    if summary.trim().is_empty() || summary.len() > 255 {
        return Err("Enter an issue summary of at most 255 characters.".into());
    }
    if !["Task", "Bug", "Story"].contains(&issue_type.as_str()) {
        return Err("Choose Task, Bug, or Story for the issue type.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut body = json!({"fields":{"project":{"key":config.project},"summary":summary.trim(),"issuetype":{"name":issue_type}}});
        if !description.trim().is_empty() {
            body["fields"]["description"] = if is_cloud(&config) {
                json!({"type":"doc","version":1,
                    "content":[{"type":"paragraph","content":[{"type":"text","text":description}]}]})
            } else {
                json!(description)
            };
        }
        let result = send(request(&client()?, Method::POST, &format!("{}/issue", base(&config)), &config, &token).json(&body))?;
        let key = result.get("key").and_then(Value::as_str).ok_or("Jira created the issue but did not return its key. Refresh the list.")?;
        Ok(JiraIssue { key:key.into(), summary:summary.trim().into(), status:"Open".into(), issue_type,
            url:format!("https://{}/browse/{key}", config.site) })
    }).await.map_err(|error| format!("Creating the Jira issue stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn jira_transitions(
    state: State<'_, AppState>,
    key: String,
) -> Result<Vec<JiraTransition>, String> {
    let (config, token) = credentials(&state)?;
    if !valid_issue_key(&key, &config.project) {
        return Err("Choose an issue from the connected Jira project.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let result = send(request(
            &client()?,
            Method::GET,
            &format!("{}/issue/{key}/transitions", base(&config)),
            &config,
            &token,
        ))?;
        let transitions = result
            .get("transitions")
            .and_then(Value::as_array)
            .ok_or("Jira returned transitions in an unexpected format.")?;
        Ok(transitions
            .iter()
            .filter_map(|value| {
                Some(JiraTransition {
                    id: value.get("id")?.as_str()?.into(),
                    name: value.get("name")?.as_str()?.into(),
                })
            })
            .collect())
    })
    .await
    .map_err(|error| format!("Loading Jira transitions stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn jira_transition(
    state: State<'_, AppState>,
    key: String,
    transition_id: String,
) -> Result<(), String> {
    let (config, token) = credentials(&state)?;
    if !valid_issue_key(&key, &config.project)
        || !transition_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("Choose a valid issue and available transition.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        send(
            request(
                &client()?,
                Method::POST,
                &format!("{}/issue/{key}/transitions", base(&config)),
                &config,
                &token,
            )
            .json(&json!({"transition":{"id":transition_id}})),
        )?;
        Ok(())
    })
    .await
    .map_err(|error| format!("Changing the Jira issue stopped unexpectedly: {error}"))?
}

fn valid_issue_key(key: &str, project: &str) -> bool {
    key.strip_prefix(project)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn validates_jira_site_and_issue_key() {
        let config = JiraConfig {
            site: "team.atlassian.net".into(),
            project: "OPS".into(),
            email: "a@example.com".into(),
            cloud_id: None,
        };
        assert!(validate_config(&config).is_ok());
        assert!(valid_issue_key("OPS-42", "OPS"));
        assert!(!valid_issue_key("OTHER-42", "OPS"));
        assert!(!valid_issue_key("OPS-42/../", "OPS"));
        let data_center = JiraConfig {
            site: "jira.example.test/jira".into(),
            email: String::new(),
            ..config.clone()
        };
        assert!(validate_config(&data_center).is_ok());
        assert_eq!(
            base(&data_center),
            "https://jira.example.test/jira/rest/api/2"
        );
        assert!(validate_config(&JiraConfig {
            site: "user@jira.example.test".into(),
            ..data_center
        })
        .is_err());
    }

    #[test]
    fn searches_jira_project_with_auth_and_reads_pagination() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api_base = format!("http://{}/rest/api/3", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..count]);
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
            assert!(received.starts_with("POST /rest/api/3/search/jql HTTP/1.1"));
            assert!(received
                .to_ascii_lowercase()
                .contains("authorization: basic "));
            assert!(received.contains("project = OPS ORDER BY updated DESC"));
            assert!(!received.contains("nextPageToken"));
            let body = r#"{"issues":[{"key":"OPS-42","fields":{"summary":"Fix sync","status":{"name":"To Do"},"issuetype":{"name":"Bug"}}}],"nextPageToken":"cursor-2"}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        });
        let config = JiraConfig {
            site: "team.atlassian.net".into(),
            project: "OPS".into(),
            email: "a@example.com".into(),
            cloud_id: None,
        };
        let result = search_at(&config, "secret", None, &api_base).unwrap();
        server.join().unwrap();
        assert_eq!(result.issues[0].key, "OPS-42");
        assert_eq!(result.issues[0].status, "To Do");
        assert_eq!(result.next_page_token.as_deref(), Some("cursor-2"));
    }

    #[test]
    fn searches_data_center_with_a_bearer_token_and_start_offsets() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api_base = format!("http://{}/rest/api/2", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0u8; 4096];
            let count = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]).to_string();
            let body = r#"{"startAt":30,"total":61,"issues":[{"key":"OPS-7","fields":{"summary":"Fix","status":{"name":"Open"},"issuetype":{"name":"Bug"}}}]}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            request
        });
        let config = JiraConfig {
            site: "jira.example.test".into(),
            project: "OPS".into(),
            email: String::new(),
            cloud_id: None,
        };
        let page = search_at(&config, "pat-secret", Some("30".into()), &api_base).unwrap();
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /rest/api/2/search?jql=project+%3D+OPS+ORDER+BY+updated+DESC&startAt=30&maxResults=30"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer pat-secret"));
        assert_eq!(page.issues[0].url, "https://jira.example.test/browse/OPS-7");
        assert_eq!(page.next_page_token.as_deref(), Some("31"));
    }

    #[test]
    fn uses_the_atlassian_api_after_browser_sign_in() {
        let config = JiraConfig {
            site: "team.atlassian.net".into(),
            project: "WEB".into(),
            email: "oauth".into(),
            cloud_id: Some("abc-123".into()),
        };
        assert!(validate_config(&config).is_ok());
        assert_eq!(
            base(&config),
            "https://api.atlassian.com/ex/jira/abc-123/rest/api/3"
        );
        let built = request(
            &client().unwrap(),
            Method::GET,
            "https://example.test",
            &config,
            "access",
        )
        .build()
        .unwrap();
        assert_eq!(built.headers()["authorization"], "Bearer access");
        let saved = serde_json::to_string(&JiraConfig {
            cloud_id: None,
            ..config
        })
        .unwrap();
        assert!(!saved.contains("cloud"));
    }
}
