//! Browser sign-in for Bitbucket Cloud, Azure DevOps and Jira Cloud. The
//! result is a grant kept in the Keychain slot the API token would use, so
//! the rest of Weave reads it like a token; it is renewed before it expires.

use crate::forge_oauth::post_form;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    net::TcpListener,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

const PREFIX: &str = "oauth:";
const REDIRECT_URI: &str = "http://localhost:53682/";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Style {
    /// Client ID in the form body (Microsoft).
    Form,
    /// Client ID and secret as HTTP basic auth (Bitbucket).
    Basic,
    /// JSON body with the secret (Atlassian).
    Json,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Grant {
    pub access: String,
    pub refresh: Option<String>,
    pub expires_at: u64,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub style: Style,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

impl Grant {
    pub fn encode(&self) -> Zeroizing<String> {
        Zeroizing::new(format!(
            "{PREFIX}{}",
            serde_json::to_string(self).unwrap_or_default()
        ))
    }

    pub fn decode(stored: &str) -> Option<Self> {
        serde_json::from_str(stored.strip_prefix(PREFIX)?).ok()
    }

    fn from_reply(value: &Value, base: &Grant) -> Result<Self, String> {
        let access = value["access_token"].as_str().ok_or_else(|| {
            let reason = value["error_description"]
                .as_str()
                .or(value["error"].as_str())
                .unwrap_or("no details");
            format!("The provider did not return a sign-in token: {reason}")
        })?;
        Ok(Self {
            access: access.into(),
            refresh: value["refresh_token"]
                .as_str()
                .map(str::to_string)
                .or_else(|| base.refresh.clone()),
            expires_at: now() + value["expires_in"].as_u64().unwrap_or(3600),
            ..base.clone()
        })
    }

    fn token_request(&self, form: &[(&str, &str)]) -> Result<Value, String> {
        match self.style {
            Style::Form => {
                let mut fields = form.to_vec();
                fields.push(("client_id", &self.client_id));
                post_form(&self.token_url, &fields)
            }
            Style::Basic => Client::new()
                .post(&self.token_url)
                .basic_auth(&self.client_id, self.client_secret.as_deref())
                .form(form)
                .timeout(Duration::from_secs(30))
                .send()
                .map_err(|error| format!("Could not reach the sign-in service: {error}"))?
                .json()
                .map_err(|_| "The sign-in service sent an unreadable reply.".to_string()),
            Style::Json => {
                let mut body: serde_json::Map<String, Value> = form
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect();
                body.insert("client_id".into(), json!(self.client_id));
                body.insert("client_secret".into(), json!(self.client_secret));
                Client::new()
                    .post(&self.token_url)
                    .json(&body)
                    .timeout(Duration::from_secs(30))
                    .send()
                    .map_err(|error| format!("Could not reach the sign-in service: {error}"))?
                    .json()
                    .map_err(|_| "The sign-in service sent an unreadable reply.".to_string())
            }
        }
    }

    /// Renews the access token a minute before it expires.
    pub fn renewed(&self) -> Result<Option<Self>, String> {
        if self.expires_at > now() + 60 {
            return Ok(None);
        }
        let refresh = self
            .refresh
            .clone()
            .ok_or("The sign-in has expired. Sign in again.")?;
        let value =
            self.token_request(&[("grant_type", "refresh_token"), ("refresh_token", &refresh)])?;
        Self::from_reply(&value, self)
            .map(Some)
            .map_err(|_| "The sign-in has expired or was revoked. Sign in again.".into())
    }
}

/// The token to send: the stored API token, or a current OAuth access token.
/// A renewed grant is passed to `save`.
pub(crate) fn current_token(
    stored: &str,
    save: impl FnOnce(&Zeroizing<String>) -> Result<(), String>,
) -> Result<Zeroizing<String>, String> {
    let Some(grant) = Grant::decode(stored) else {
        return Ok(Zeroizing::new(stored.to_string()));
    };
    match grant.renewed()? {
        Some(next) => {
            save(&next.encode())?;
            Ok(Zeroizing::new(next.access))
        }
        None => Ok(Zeroizing::new(grant.access)),
    }
}

pub(crate) fn is_oauth(stored: &str) -> bool {
    stored.starts_with(PREFIX)
}

/// Opens the authorize page and exchanges the code it returns.
pub(crate) fn browser_sign_in(authorize: &str, mut base: Grant) -> Result<Grant, String> {
    let listener = TcpListener::bind(("127.0.0.1", 53682)).map_err(|_| {
        "Port 53682 is in use, maybe by another sign-in. Finish or close it, then try again."
            .to_string()
    })?;
    let state = uuid::Uuid::new_v4().simple().to_string();
    let mut url = reqwest::Url::parse(authorize).map_err(|error| error.to_string())?;
    url.query_pairs_mut()
        .append_pair("state", &state)
        .append_pair("redirect_uri", REDIRECT_URI);
    crate::platform::open_url(url.as_str())?;
    let code = crate::cloud::wait_for_code(&listener, &state, Duration::from_secs(300))?;
    base.access.clear();
    let value = base.token_request(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", REDIRECT_URI),
    ])?;
    Grant::from_reply(&value, &base)
}

pub(crate) fn bitbucket_grant(client_id: &str, secret: &str) -> Grant {
    Grant {
        access: String::new(),
        refresh: None,
        expires_at: 0,
        token_url: "https://bitbucket.org/site/oauth2/access_token".into(),
        client_id: client_id.into(),
        client_secret: Some(secret.into()),
        style: Style::Basic,
    }
}

pub(crate) fn atlassian_grant(client_id: &str, secret: &str) -> Grant {
    Grant {
        access: String::new(),
        refresh: None,
        expires_at: 0,
        token_url: "https://auth.atlassian.com/oauth/token".into(),
        client_id: client_id.into(),
        client_secret: Some(secret.into()),
        style: Style::Json,
    }
}

pub(crate) fn microsoft_grant(client_id: &str, tokens: &crate::forge_oauth::Tokens) -> Grant {
    Grant {
        access: tokens.access.to_string(),
        refresh: tokens.refresh.as_ref().map(|token| token.to_string()),
        expires_at: now() + tokens.expires_in.unwrap_or(3600),
        token_url: "https://login.microsoftonline.com/organizations/oauth2/v2.0/token".into(),
        client_id: client_id.into(),
        client_secret: None,
        style: Style::Form,
    }
}

pub(crate) fn valid_client(client_id: &str, secret: Option<&str>) -> Result<(), String> {
    let ok = |value: &str| {
        !value.is_empty() && value.len() <= 300 && !value.chars().any(char::is_whitespace)
    };
    if !ok(client_id) || secret.is_some_and(|secret| !ok(secret)) {
        return Err(
            "Enter the key (client ID) and secret from your app registration, without spaces."
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::tests::serve;
    use std::sync::Arc;

    #[test]
    fn stores_grants_like_tokens_and_renews_them() {
        let base = serve(Arc::new(|_, _, headers, body| {
            let body = String::from_utf8_lossy(body).to_string();
            if headers
                .get("authorization")
                .is_some_and(|value| value.starts_with("Basic "))
            {
                assert!(
                    body.contains("grant_type=refresh_token")
                        && body.contains("refresh_token=old-refresh")
                );
                (
                    200,
                    vec![],
                    br#"{"access_token":"new-access","expires_in":7200}"#.to_vec(),
                )
            } else {
                let sent: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(
                    (sent["client_secret"].as_str(), sent["grant_type"].as_str()),
                    (Some("shh"), Some("refresh_token"))
                );
                (200, vec![], br#"{"access_token":"jira-access","refresh_token":"rotated","expires_in":3600}"#.to_vec())
            }
        }));
        let mut grant = bitbucket_grant("key", "secret");
        grant.token_url = format!("{base}/token");
        grant.access = "old-access".into();
        grant.refresh = Some("old-refresh".into());
        let stored = grant.encode();
        assert!(is_oauth(&stored));
        assert_eq!(Grant::decode(&stored).unwrap(), grant);
        let mut saved = None;
        let token = current_token(&stored, |next| {
            saved = Some(next.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(token.as_str(), "new-access");
        let renewed = Grant::decode(saved.as_deref().unwrap()).unwrap();
        assert_eq!(renewed.refresh.as_deref(), Some("old-refresh"));
        assert!(renewed.expires_at > now() + 7000);
        assert_eq!(
            current_token(&renewed.encode(), |_| unreachable!())
                .unwrap()
                .as_str(),
            "new-access"
        );

        let mut jira = atlassian_grant("id", "shh");
        jira.token_url = format!("{base}/token");
        jira.refresh = Some("r".into());
        let next = jira.renewed().unwrap().unwrap();
        assert_eq!(
            (next.access.as_str(), next.refresh.as_deref()),
            ("jira-access", Some("rotated"))
        );

        assert_eq!(
            current_token("plain-api-token", |_| unreachable!())
                .unwrap()
                .as_str(),
            "plain-api-token"
        );
        assert!(valid_client("key", Some("has space")).is_err());
    }
}
