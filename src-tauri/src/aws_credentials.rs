//! S3 connections can take their keys from an AWS CLI profile instead of a
//! saved secret. Static keys come from ~/.aws/credentials; temporary keys
//! come from a `credential_process` helper or `aws configure
//! export-credentials` (SSO and assumed roles) and are refreshed shortly
//! before they expire.

use crate::{remote_access::find_tool, AppState};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    process::Command,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tauri::State;
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct AwsCredentials {
    pub access_key: String,
    pub secret: Zeroizing<String>,
    pub token: Option<Zeroizing<String>>,
    /// Unix seconds; `None` for keys that do not expire.
    pub expires: Option<i64>,
}

fn cache() -> &'static Mutex<HashMap<String, AwsCredentials>> {
    static CACHE: OnceLock<Mutex<HashMap<String, AwsCredentials>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn setting_key(profile_id: &str) -> String {
    format!("s3.aws_profile:{profile_id}")
}

fn aws_file(variable: &str, name: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".aws").join(name)))
}

/// Sections of an AWS INI file. Config files name profiles `[profile x]`.
fn sections(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut result: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') || line.starts_with(';') || line.is_empty() {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let name = name.trim();
            let name = name
                .strip_prefix("profile ")
                .unwrap_or(name)
                .trim()
                .to_string();
            result.entry(name.clone()).or_default();
            current = Some(name);
        } else if let (Some(name), Some((key, value))) = (&current, line.split_once('=')) {
            result
                .entry(name.clone())
                .or_default()
                .insert(key.trim().to_lowercase(), value.trim().to_string());
        }
    }
    result
}

fn read_sections(variable: &str, name: &str) -> HashMap<String, HashMap<String, String>> {
    aws_file(variable, name)
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|text| sections(&text))
        .unwrap_or_default()
}

pub fn profile_names() -> Vec<String> {
    let mut names: Vec<String> = read_sections("AWS_SHARED_CREDENTIALS_FILE", "credentials")
        .into_keys()
        .chain(read_sections("AWS_CONFIG_FILE", "config").into_keys())
        .filter(|name| !name.starts_with("sso-session ") && !name.starts_with("services "))
        .collect();
    names.sort();
    names.dedup();
    names
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ProcessOutput {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    expiration: Option<String>,
}

fn from_process_output(stdout: &[u8]) -> Result<AwsCredentials, String> {
    let output: ProcessOutput = serde_json::from_slice(stdout)
        .map_err(|_| "The AWS credential helper returned output Weave could not read.")?;
    let expires = output
        .expiration
        .as_deref()
        .map(|value| {
            chrono::DateTime::parse_from_rfc3339(value)
                .map(|time| time.timestamp())
                .map_err(|_| {
                    format!("The AWS credential helper returned an unreadable expiry time: {value}")
                })
        })
        .transpose()?;
    Ok(AwsCredentials {
        access_key: output.access_key_id,
        secret: Zeroizing::new(output.secret_access_key),
        token: output
            .session_token
            .filter(|token| !token.is_empty())
            .map(Zeroizing::new),
        expires,
    })
}

fn run_helper(mut command: Command, description: &str) -> Result<AwsCredentials, String> {
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start {description}: {error}"))?;
    let started = std::time::Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            break;
        }
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            return Err(format!("{description} did not finish within a minute."));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        return Err(
            if detail.contains("Token has expired") || detail.contains("sso login") {
                "Your AWS SSO session has expired. Run `aws sso login` in Terminal, then try again."
                    .into()
            } else {
                format!("{description} failed: {}", detail.trim())
            },
        );
    }
    from_process_output(&output.stdout)
}

pub fn resolve(profile: &str) -> Result<AwsCredentials, String> {
    let credentials = read_sections("AWS_SHARED_CREDENTIALS_FILE", "credentials");
    if let Some(values) = credentials.get(profile) {
        if let (Some(key), Some(secret)) = (
            values.get("aws_access_key_id"),
            values.get("aws_secret_access_key"),
        ) {
            return Ok(AwsCredentials {
                access_key: key.clone(),
                secret: Zeroizing::new(secret.clone()),
                token: values
                    .get("aws_session_token")
                    .map(|token| Zeroizing::new(token.clone())),
                expires: None,
            });
        }
    }
    let config = read_sections("AWS_CONFIG_FILE", "config");
    let settings = config.get(profile).or_else(|| credentials.get(profile));
    if let Some(process) = settings.and_then(|values| values.get("credential_process")) {
        let command = if cfg!(windows) {
            let mut command = crate::platform::command("cmd");
            command.arg("/C").arg(process);
            command
        } else {
            let mut command = crate::platform::command("/bin/sh");
            command.arg("-c").arg(process);
            command
        };
        return run_helper(command, "the AWS credential_process helper");
    }
    if settings.is_none() {
        return Err(format!(
            "There is no AWS profile named “{profile}” in ~/.aws/config or ~/.aws/credentials."
        ));
    }
    let aws = find_tool("aws").ok_or(
        "This AWS profile uses SSO or an assumed role, which needs the AWS CLI. Install it from aws.amazon.com/cli, then try again.",
    )?;
    let mut command = crate::platform::command(aws);
    command.args([
        "configure",
        "export-credentials",
        "--format",
        "process",
        "--profile",
        profile,
    ]);
    run_helper(command, "the AWS CLI")
}

fn linked_profile(state: &AppState, profile_id: &str) -> Option<String> {
    let db = state.db.lock().ok()?;
    db.query_row(
        "SELECT value FROM settings WHERE key=?1",
        [setting_key(profile_id)],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// Fresh credentials for an S3 profile linked to an AWS profile, or `None`
/// when the connection uses its own saved keys.
pub fn credentials_for(
    state: &AppState,
    profile_id: &str,
) -> Result<Option<AwsCredentials>, String> {
    let Some(aws_profile) = linked_profile(state, profile_id) else {
        return Ok(None);
    };
    let now = chrono::Utc::now().timestamp();
    let mut cache = cache()
        .lock()
        .map_err(|_| "AWS credentials are busy. Try again.")?;
    if let Some(cached) = cache.get(profile_id) {
        if cached.expires.is_none_or(|expires| expires - now > 300) {
            return Ok(Some(cached.clone()));
        }
    }
    let fresh = resolve(&aws_profile)?;
    cache.insert(profile_id.to_string(), fresh.clone());
    Ok(Some(fresh))
}

/// The secret and token last handed out for a linked profile.
pub fn cached(profile_id: &str) -> Option<AwsCredentials> {
    cache().lock().ok()?.get(profile_id).cloned()
}

#[tauri::command]
pub fn s3_aws_profiles() -> Vec<String> {
    profile_names()
}

#[tauri::command]
pub fn s3_aws_link_get(state: State<'_, AppState>, profile_id: String) -> Option<String> {
    linked_profile(&state, &profile_id)
}

#[tauri::command]
pub async fn s3_aws_link(
    state: State<'_, AppState>,
    profile_id: String,
    aws_profile: Option<String>,
) -> Result<(), String> {
    if let Some(name) = aws_profile.clone() {
        tauri::async_runtime::spawn_blocking(move || resolve(&name))
            .await
            .map_err(|error| error.to_string())??;
    }
    if let Ok(mut cache) = cache().lock() {
        cache.remove(&profile_id);
    }
    let db = state
        .db
        .lock()
        .map_err(|_| "Settings are busy. Try again.")?;
    match aws_profile {
        Some(name) => db.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [setting_key(&profile_id), name],
        ),
        None => db.execute("DELETE FROM settings WHERE key=?1", [setting_key(&profile_id)]),
    }
    .map_err(|error| format!("Could not save the AWS profile choice: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_profiles_from_credentials_and_config_files() {
        let parsed = sections(
            "[default]\naws_access_key_id = AKIA1\naws_secret_access_key = s1\n\n# comment\n[profile work]\nregion = eu-west-1\ncredential_process = /bin/echo hi\n",
        );
        assert_eq!(parsed["default"]["aws_access_key_id"], "AKIA1");
        assert_eq!(parsed["work"]["credential_process"], "/bin/echo hi");
    }

    #[test]
    fn reads_credential_process_output_with_expiry() {
        let credentials = from_process_output(
            br#"{"Version":1,"AccessKeyId":"ASIA1","SecretAccessKey":"sec","SessionToken":"tok","Expiration":"2030-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(credentials.access_key, "ASIA1");
        assert_eq!(
            credentials.token.as_deref().map(String::as_str),
            Some("tok")
        );
        assert_eq!(credentials.expires, Some(1_893_456_000));
        assert!(from_process_output(b"not json").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn resolves_static_keys_and_helpers_and_refreshes_expiring_ones() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = dir.path().join("credentials");
        let config = dir.path().join("config");
        let counter = dir.path().join("count");
        fs::write(
            &credentials,
            "[static]\naws_access_key_id=AKIA2\naws_secret_access_key=secret2\n",
        )
        .unwrap();
        fs::write(
            &config,
            format!(
                "[profile helper]\ncredential_process = /bin/sh -c 'echo x >> {}; printf \"{{\\\"Version\\\":1,\\\"AccessKeyId\\\":\\\"ASIA9\\\",\\\"SecretAccessKey\\\":\\\"s9\\\",\\\"SessionToken\\\":\\\"t9\\\",\\\"Expiration\\\":\\\"2000-01-01T00:00:00Z\\\"}}\"'\n",
                counter.display()
            ),
        )
        .unwrap();
        std::env::set_var("AWS_SHARED_CREDENTIALS_FILE", &credentials);
        std::env::set_var("AWS_CONFIG_FILE", &config);

        assert_eq!(profile_names(), ["helper", "static"]);
        assert_eq!(resolve("static").unwrap().access_key, "AKIA2");
        assert!(resolve("missing").err().unwrap().contains("no AWS profile"));

        let state = AppState::for_tests();
        state
            .db
            .lock()
            .unwrap()
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        assert!(credentials_for(&state, "p").unwrap().is_none());
        state
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO settings VALUES (?1, 'helper')",
                [setting_key("p")],
            )
            .unwrap();
        let first = credentials_for(&state, "p").unwrap().unwrap();
        assert_eq!(first.access_key, "ASIA9");
        // The helper's keys are already expired, so every request refreshes them.
        credentials_for(&state, "p").unwrap();
        assert_eq!(fs::read_to_string(&counter).unwrap().lines().count(), 2);
        assert_eq!(cached("p").unwrap().secret.as_str(), "s9");

        std::env::remove_var("AWS_SHARED_CREDENTIALS_FILE");
        std::env::remove_var("AWS_CONFIG_FILE");
    }
}
