use crate::{profiles, AppState};
use keyring::{Entry, Error};
use tauri::State;
use zeroize::Zeroizing;

fn entry(profile_id: &str) -> Result<Entry, String> {
    Entry::new(
        "app.weave.desktop",
        &format!("profile:{profile_id}:password"),
    )
    .map_err(|error| format!("Could not open the system keychain: {error}"))
}

fn token_entry(profile_id: &str) -> Result<Entry, String> {
    Entry::new(
        "app.weave.desktop",
        &format!("profile:{profile_id}:s3-session-token"),
    )
    .map_err(|error| format!("Could not open the system keychain: {error}"))
}

fn remove_saved_token(profile_id: &str) -> Result<(), String> {
    match token_entry(profile_id)?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(error) => Err(format!(
            "Could not remove the saved S3 session token: {error}"
        )),
    }
}

pub fn s3_token_for(
    state: &AppState,
    profile_id: &str,
) -> Result<Option<Zeroizing<String>>, String> {
    if let Some(credentials) = crate::aws_credentials::cached(profile_id) {
        return Ok(credentials.token);
    }
    if let Some(token) = state
        .session_s3_tokens
        .lock()
        .map_err(|_| "S3 token manager is busy")?
        .get(profile_id)
    {
        return Ok(Some(Zeroizing::new(token.to_string())));
    }
    match token_entry(profile_id)?.get_password() {
        Ok(token) => Ok(Some(Zeroizing::new(token))),
        Err(Error::NoEntry) => Ok(None),
        Err(error) => Err(format!(
            "Could not read the saved S3 session token: {error}"
        )),
    }
}

#[tauri::command]
pub fn s3_token_set(
    state: State<'_, AppState>,
    profile_id: String,
    token: String,
    remember: bool,
) -> Result<(), String> {
    let profile = profiles::get_profile(&state, &profile_id)?;
    if profile.protocol != "s3" {
        return Err("Choose an S3 connection to save a session token.".into());
    }
    let token = Zeroizing::new(token.trim().to_string());
    if token.is_empty() || token.len() > 8192 || token.chars().any(char::is_whitespace) {
        return Err("Enter an S3 session token without spaces, up to 8192 characters.".into());
    }
    if remember {
        token_entry(&profile_id)?
            .set_password(&token)
            .map_err(|error| {
                format!("Could not save S3 session token in the system keychain: {error}")
            })?;
    } else {
        remove_saved_token(&profile_id)?;
    }
    state
        .session_s3_tokens
        .lock()
        .map_err(|_| "S3 token manager is busy")?
        .insert(profile_id, token);
    Ok(())
}

#[tauri::command]
pub fn s3_token_clear(state: State<'_, AppState>, profile_id: String) -> Result<(), String> {
    profiles::get_profile(&state, &profile_id)?;
    remove_saved_token(&profile_id)?;
    state
        .session_s3_tokens
        .lock()
        .map_err(|_| "S3 token manager is busy")?
        .remove(&profile_id);
    Ok(())
}

fn remove_saved(profile_id: &str) -> Result<(), String> {
    match entry(profile_id)?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("Could not remove the saved password: {error}")),
    }
}

pub fn password_for(
    state: &AppState,
    profile: &profiles::Profile,
) -> Result<Zeroizing<String>, String> {
    let profile_id = &profile.id;
    if profile.protocol == "s3" {
        if let Some(credentials) = crate::aws_credentials::cached(profile_id) {
            return Ok(credentials.secret);
        }
    }
    if let Some(password) = state
        .session_passwords
        .lock()
        .map_err(|_| "Password manager is busy")?
        .get(profile_id)
    {
        return Ok(Zeroizing::new(password.to_string()));
    }
    match entry(profile_id)?.get_password() {
        Ok(password) => Ok(Zeroizing::new(password)),
        Err(Error::NoEntry)
            if matches!(profile.protocol.as_str(), "ftp" | "ftps" | "ftps-implicit")
                && profile.username.eq_ignore_ascii_case("anonymous") =>
        {
            Ok(Zeroizing::new(String::new()))
        }
        Err(Error::NoEntry) => Err(if profile.protocol == "s3" { "This S3 connection needs a secret access key. Edit the profile and enter it before opening Files." } else if profile.protocol == "azure" { "This Azure connection needs its account key or a SAS token. Edit the profile and enter it before opening Files." } else { "This connection needs a password. Edit the profile and enter its password before opening Files." }.into()),
        Err(error) => Err(format!("Could not read the saved password: {error}")),
    }
}

#[tauri::command]
pub fn profile_password_set(
    state: State<'_, AppState>,
    profile_id: String,
    password: String,
    remember: bool,
) -> Result<(), String> {
    let password = Zeroizing::new(password);
    profiles::get_profile(&state, &profile_id)?;
    if password.is_empty() {
        return Err("Enter a password for this connection.".into());
    }
    if remember {
        entry(&profile_id)?
            .set_password(&password)
            .map_err(|error| format!("Could not save password in the system keychain: {error}"))?;
    } else {
        remove_saved(&profile_id)?;
    }
    state
        .session_passwords
        .lock()
        .map_err(|_| "Password manager is busy")?
        .insert(profile_id, password);
    Ok(())
}

/// Saves a password answered at a sign-in prompt in the Keychain.
pub(crate) fn save_profile_password(
    state: &AppState,
    profile_id: &str,
    password: &str,
) -> Result<(), String> {
    entry(profile_id)?
        .set_password(password)
        .map_err(|error| format!("Could not save password in the system keychain: {error}"))?;
    state
        .session_passwords
        .lock()
        .map_err(|_| "Password manager is busy")?
        .insert(profile_id.to_string(), Zeroizing::new(password.to_string()));
    Ok(())
}

#[tauri::command]
pub fn profile_password_clear(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<(), String> {
    profiles::get_profile(&state, &profile_id)?;
    remove_saved(&profile_id)?;
    state
        .session_passwords
        .lock()
        .map_err(|_| "Password manager is busy")?
        .remove(&profile_id);
    Ok(())
}

pub fn remove_profile_password(state: &AppState, profile_id: &str) -> Result<(), String> {
    remove_saved(profile_id)?;
    remove_saved_token(profile_id)?;
    state
        .session_passwords
        .lock()
        .map_err(|_| "Password manager is busy")?
        .remove(profile_id);
    state
        .session_s3_tokens
        .lock()
        .map_err(|_| "S3 token manager is busy")?
        .remove(profile_id);
    Ok(())
}
