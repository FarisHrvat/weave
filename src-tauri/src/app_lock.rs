//! Locks Weave's window when it opens and after a chosen idle time. On macOS
//! this asks for Touch ID or the login password; on Windows and Linux it asks
//! for a lock password kept as an Argon2 hash in the system's credential store.
//! Secrets stay in that store either way; the lock keeps hosts, sessions and
//! files out of view of someone at the computer.

use crate::AppState;
use serde::{Deserialize, Serialize};
use tauri::State;

const KEY: &str = "app_lock";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppLock {
    pub enabled: bool,
    /// Lock again after this many idle minutes; 0 locks only when Weave opens.
    pub idle_minutes: u32,
    /// Filled in by Weave: true where unlocking asks for Weave's own password.
    #[serde(default)]
    pub uses_password: bool,
}

impl AppLock {
    fn validate(&self) -> Result<(), String> {
        if self.idle_minutes > 24 * 60 {
            return Err("Choose an idle time of at most 24 hours.".into());
        }
        Ok(())
    }
}

fn load(state: &AppState) -> AppLock {
    let saved: AppLock = state
        .db
        .lock()
        .ok()
        .and_then(|db| {
            db.query_row("SELECT value FROM settings WHERE key=?1", [KEY], |row| {
                row.get::<_, String>(0)
            })
            .ok()
        })
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default();
    AppLock {
        uses_password: !cfg!(target_os = "macos"),
        ..saved
    }
}

#[cfg(target_os = "macos")]
mod system {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};
    use std::{sync::mpsc, time::Duration};

    /// Asks macOS for Touch ID, an Apple Watch or the login password.
    pub fn authenticate(reason: &str) -> Result<(), String> {
        let (sender, receiver) = mpsc::channel();
        unsafe {
            let context = LAContext::new();
            if let Err(error) = context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication)
            {
                return Err(match error.code() {
                    -5 => "This Mac has no login password, so Weave cannot be locked.".into(),
                    _ => format!(
                        "macOS cannot confirm it is you: {}",
                        error.localizedDescription()
                    ),
                });
            }
            let reply = RcBlock::new(move |success: Bool, error: *mut NSError| {
                let result = if success.as_bool() {
                    Ok(())
                } else {
                    let code = error.as_ref().map_or(0, |error| error.code());
                    Err(match code {
                        -2 | -4 | -9 => "Unlocking was cancelled.".to_string(),
                        -8 => "Touch ID is locked after too many tries. Use your password.".into(),
                        _ => error.as_ref().map_or_else(
                            || "macOS could not confirm it is you.".to_string(),
                            |error| error.localizedDescription().to_string(),
                        ),
                    })
                };
                let _ = sender.send(result);
            });
            context.evaluatePolicy_localizedReason_reply(
                LAPolicy::DeviceOwnerAuthentication,
                &NSString::from_str(reason),
                &reply,
            );
        }
        receiver
            .recv_timeout(Duration::from_secs(300))
            .unwrap_or_else(|_| Err("Unlocking timed out. Try again.".into()))
    }
}

/// Weave's own lock password, for systems without a built-in prompt.
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod password {
    use argon2::{
        password_hash::{
            rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
        },
        Argon2,
    };
    use keyring::{Entry, Error};

    fn entry() -> Result<Entry, String> {
        Entry::new("app.weave.desktop", "app-lock-password")
            .map_err(|error| format!("Could not open the credential store: {error}"))
    }

    pub fn hash(password: &str) -> Result<String, String> {
        if password.chars().count() < 6 {
            return Err("Choose a lock password of at least 6 characters.".into());
        }
        Argon2::default()
            .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
            .map(|hash| hash.to_string())
            .map_err(|error| format!("Could not protect the password: {error}"))
    }

    pub fn matches(stored: &str, password: &str) -> bool {
        PasswordHash::new(stored).is_ok_and(|hash| {
            Argon2::default()
                .verify_password(password.as_bytes(), &hash)
                .is_ok()
        })
    }

    pub fn stored() -> Result<Option<String>, String> {
        match entry()?.get_password() {
            Ok(hash) => Ok(Some(hash)),
            Err(Error::NoEntry) => Ok(None),
            Err(error) => Err(format!("Could not read the lock password: {error}")),
        }
    }

    pub fn save(hash: &str) -> Result<(), String> {
        entry()?
            .set_password(hash)
            .map_err(|error| format!("Could not save the lock password: {error}"))
    }

    pub fn check(given: Option<&str>) -> Result<(), String> {
        let Some(stored) = stored()? else {
            return Err("No lock password is set.".into());
        };
        match given {
            Some(given) if matches(&stored, given) => Ok(()),
            Some(_) => Err("That password is not right.".into()),
            None => Err("Enter your lock password.".into()),
        }
    }
}

/// Confirms it is the person who set up the lock.
async fn confirm(reason: &'static str, given: Option<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        {
            let _ = given;
            system::authenticate(reason)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reason;
            password::check(given.as_deref())
        }
    })
    .await
    .map_err(|error| format!("Unlocking stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub fn app_lock_get(state: State<'_, AppState>) -> AppLock {
    load(&state)
}

/// Changing the lock first asks for the same proof as unlocking, so it cannot
/// be switched off by someone at an unlocked window. Where Weave uses its own
/// password, `new_password` sets it the first time (or replaces it).
#[tauri::command]
pub async fn app_lock_set(
    state: State<'_, AppState>,
    lock: AppLock,
    password: Option<String>,
    new_password: Option<String>,
) -> Result<AppLock, String> {
    lock.validate()?;
    let current = load(&state);
    let uses_password = current.uses_password;
    let has_password = uses_password && password::stored()?.is_some();
    if lock.enabled && uses_password && !has_password && new_password.is_none() {
        return Err("Choose a lock password first.".into());
    }
    let changed = lock.enabled != current.enabled
        || lock.idle_minutes != current.idle_minutes
        || new_password.is_some();
    if changed && (!uses_password || has_password) {
        confirm("change Weave's lock settings", password).await?;
    }
    if let Some(new_password) = new_password {
        password::save(&password::hash(&new_password)?)?;
    }
    let saved = AppLock {
        uses_password,
        ..lock
    };
    let db = state
        .db
        .lock()
        .map_err(|_| "Settings are busy. Try again.")?;
    db.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [KEY, &serde_json::to_string(&saved).map_err(|error| error.to_string())?],
    )
    .map_err(|error| format!("Could not save the lock settings: {error}"))?;
    Ok(saved)
}

#[tauri::command]
pub async fn app_unlock(password: Option<String>) -> Result<(), String> {
    confirm("unlock Weave", password).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_validates_lock_settings() {
        let state = AppState::for_tests();
        state
            .db
            .lock()
            .unwrap()
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        let empty = load(&state);
        assert!(!empty.enabled);
        assert_eq!(empty.uses_password, !cfg!(target_os = "macos"));
        let lock = AppLock {
            enabled: true,
            idle_minutes: 15,
            uses_password: false,
        };
        state
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)",
                [KEY, &serde_json::to_string(&lock).unwrap()],
            )
            .unwrap();
        assert_eq!(load(&state).idle_minutes, 15);
        assert!(AppLock {
            enabled: true,
            idle_minutes: 2000,
            uses_password: false,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn checks_lock_passwords_against_their_hash() {
        let hash = password::hash("correct horse").unwrap();
        assert!(password::matches(&hash, "correct horse"));
        assert!(!password::matches(&hash, "wrong horse"));
        assert!(password::hash("short").is_err());
    }
}
