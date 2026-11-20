//! Writes commit messages for staged changes with Claude, using the user's
//! own Anthropic API key kept in the system keychain. Rust has no official
//! Anthropic SDK, so this calls the Messages API over HTTPS directly.

use crate::git::run;
use keyring::{Entry, Error as KeyringError};
use serde_json::{json, Value};
use std::time::Duration;
use zeroize::Zeroizing;

const API: &str = "https://api.anthropic.com";
const MODEL: &str = "claude-opus-5-5";
/// Staged diffs larger than this are refused rather than silently cut short.
const MAX_DIFF_BYTES: usize = 400_000;

fn entry() -> Result<Entry, String> {
    Entry::new("app.weave.desktop", "anthropic-api-key")
        .map_err(|error| format!("Could not open the system keychain: {error}"))
}

fn api_key() -> Result<Zeroizing<String>, String> {
    entry()?
        .get_password()
        .map(Zeroizing::new)
        .map_err(|error| match error {
            KeyringError::NoEntry => {
                "Add your Anthropic API key to write commit messages.".to_string()
            }
            _ => format!("Could not read the API key from the system keychain: {error}"),
        })
}

const INSTRUCTIONS: &str = "You write Git commit messages. Describe what the staged changes do and why, \
in the style of the repository's recent commits. Use a summary line of at most 72 characters in the \
imperative mood, then a blank line and a short body only when it adds useful context. Reply with the \
commit message only.";

fn prompt(diff: &str, recent: &str) -> String {
    format!("Recent commit subjects:\n{recent}\n\nStaged changes:\n{diff}")
}

const EXPLAIN: &str = "You explain Git commits to a developer reviewing history. In a few short \
paragraphs of plain prose, say what the commit changes, why it likely did so, and anything a reviewer \
should watch for. Do not repeat the diff line by line.";

const PULL_REQUEST: &str = "You write pull request descriptions. Reply with a title of at most 72 \
characters on the first line, a blank line, then a short Markdown description: a summary of the \
change and a bulleted list of the notable parts. Mention testing only when the changes show it.";

pub(crate) fn generate_at(
    base: &str,
    key: &str,
    diff: &str,
    recent: &str,
) -> Result<String, String> {
    ask(base, key, INSTRUCTIONS, &prompt(diff, recent))
}

/// Sends one request and returns the text of the reply.
fn ask(base: &str, key: &str, system: &str, content: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|error| format!("Could not prepare the request: {error}"))?;
    let body = json!({
        "model": MODEL,
        "max_tokens": 16000,
        "fallbacks": "default",
        "output_config": {"effort": "low"},
        "system": system,
        "messages": [{"role": "user", "content": content}],
    });
    let response = client
        .post(format!("{base}/v1/messages"))
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "server-side-fallback-2026-07-01")
        .json(&body)
        .send()
        .map_err(|error| format!("Could not reach the Anthropic API: {error}"))?;
    let status = response.status();
    let value: Value = response.json().unwrap_or(Value::Null);
    if !status.is_success() {
        let detail = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("no details");
        return Err(match status.as_u16() {
            401 => "The Anthropic API key was not accepted. Check it and save it again.".into(),
            429 => "The Anthropic API is rate limiting requests. Try again in a minute.".into(),
            500..=599 => "The Anthropic API is busy. Try again shortly.".into(),
            _ => format!("The Anthropic API returned {status}: {detail}"),
        });
    }
    if value["stop_reason"] == "refusal" {
        return Err(
            "The model declined to describe these changes. Write the message yourself.".into(),
        );
    }
    let text: String = value["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();
    if text.is_empty() {
        return Err("The model did not return a message. Try again.".into());
    }
    Ok(text)
}

#[tauri::command]
pub fn ai_key_status() -> bool {
    api_key().is_ok()
}

#[tauri::command]
pub fn ai_key_set(key: String) -> Result<(), String> {
    let key = Zeroizing::new(key.trim().to_string());
    if !key.starts_with("sk-ant-") || key.len() < 20 || key.chars().any(char::is_whitespace) {
        return Err("Enter an Anthropic API key, which starts with sk-ant-.".into());
    }
    entry()?
        .set_password(&key)
        .map_err(|error| format!("Could not save the key in the system keychain: {error}"))
}

#[tauri::command]
pub fn ai_key_clear() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(format!("Could not remove the key: {error}")),
    }
}

#[tauri::command]
pub async fn git_ai_commit_message(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let diff = run(&path, &["diff", "--cached", "--no-color", "--no-ext-diff"])?;
        if diff.trim().is_empty() {
            return Err("Stage the changes you want to describe first.".into());
        }
        if diff.len() > MAX_DIFF_BYTES {
            return Err("The staged changes are too large to describe in one message. Stage fewer files and try again.".into());
        }
        let recent = run(&path, &["log", "-10", "--format=%s"]).unwrap_or_default();
        generate_at(API, &api_key()?, &diff, &recent)
    })
    .await
    .map_err(|error| format!("Writing the message stopped unexpectedly: {error}"))?
}

fn too_large(text: &str) -> Result<(), String> {
    if text.len() > MAX_DIFF_BYTES {
        return Err("These changes are too large to describe in one request.".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn git_ai_explain_commit(path: String, commit: String) -> Result<String, String> {
    if commit.starts_with('-') || commit.contains(char::is_whitespace) || commit.is_empty() {
        return Err("Choose a commit from the list.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let shown = run(
            &path,
            &[
                "show",
                "--no-color",
                "--no-ext-diff",
                "--stat",
                "--patch",
                "--diff-merges=first-parent",
                &commit,
            ],
        )?;
        too_large(&shown)?;
        ask(API, &api_key()?, EXPLAIN, &shown)
    })
    .await
    .map_err(|error| format!("Explaining the commit stopped unexpectedly: {error}"))?
}

/// Describes the changes on `target` since it split from `base`.
#[tauri::command]
pub async fn git_ai_pull_request(
    path: String,
    base: String,
    target: String,
) -> Result<String, String> {
    if [&base, &target]
        .iter()
        .any(|rev| rev.is_empty() || rev.starts_with('-') || rev.contains(char::is_whitespace))
    {
        return Err("Choose the branches to compare.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let range = format!("{base}...{target}");
        let log = run(
            &path,
            &[
                "log",
                "--no-color",
                "--format=- %s%n%b",
                &format!("{base}..{target}"),
            ],
        )?;
        if log.trim().is_empty() {
            return Err(format!(
                "{target} has no commits that are not already in {base}."
            ));
        }
        let diff = run(&path, &["diff", "--no-color", "--no-ext-diff", &range])?;
        let content = format!("Commits:\n{log}\n\nChanges:\n{diff}");
        too_large(&content)?;
        ask(API, &api_key()?, PULL_REQUEST, &content)
    })
    .await
    .map_err(|error| format!("Writing the description stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn serve(status: &'static str, reply: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut bytes = [0_u8; 8192];
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
            write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{reply}", reply.len()).unwrap();
            String::from_utf8(request).unwrap()
        });
        (base, handle)
    }

    #[test]
    fn sends_the_staged_diff_and_reads_the_text_reply() {
        let (base, handle) = serve(
            "200 OK",
            r#"{"content":[{"type":"thinking","thinking":""},{"type":"text","text":"Fix login timeout\n\nRetry once before failing."}],"stop_reason":"end_turn"}"#,
        );
        let message = generate_at(&base, "sk-ant-test", "diff --git a/x b/x", "Add tests").unwrap();
        assert_eq!(message, "Fix login timeout\n\nRetry once before failing.");
        let request = handle.join().unwrap();
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("x-api-key: sk-ant-test"));
        assert!(lower.contains("anthropic-version: 2023-06-01"));
        assert!(lower.contains("anthropic-beta: server-side-fallback-2026-07-01"));
        assert!(request.contains(r#""model":"claude-opus-5-5""#));
        assert!(request.contains(r#""fallbacks":"default""#));
        assert!(request.contains("diff --git a/x b/x"));
    }

    #[test]
    fn explains_refusals_and_bad_keys() {
        let (base, handle) = serve("200 OK", r#"{"content":[],"stop_reason":"refusal"}"#);
        assert!(generate_at(&base, "k", "d", "")
            .unwrap_err()
            .contains("declined"));
        handle.join().unwrap();
        let (base, handle) = serve(
            "401 Unauthorized",
            r#"{"error":{"message":"invalid x-api-key"}}"#,
        );
        assert!(generate_at(&base, "k", "d", "")
            .unwrap_err()
            .contains("not accepted"));
        handle.join().unwrap();
        assert!(ai_key_set("not-a-key".into()).is_err());
        let (base, handle) = serve(
            "200 OK",
            r#"{"content":[{"type":"text","text":"It renames a file."}],"stop_reason":"end_turn"}"#,
        );
        assert_eq!(
            ask(&base, "k", EXPLAIN, "commit abc").unwrap(),
            "It renames a file."
        );
        assert!(handle.join().unwrap().contains("explain Git commits"));
        assert!(
            tauri::async_runtime::block_on(git_ai_explain_commit(".".into(), "--all".into()))
                .is_err()
        );
    }
}
