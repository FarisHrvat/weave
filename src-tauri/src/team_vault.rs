//! A team vault: an encrypted file in a folder the team already syncs
//! (iCloud Drive, Dropbox, OneDrive, a network share). Each Mac publishes the
//! hosts it marks as shared and picks up everyone else's. Hosts are merged one
//! by one with their last-synced state, so edits to different hosts never
//! overwrite each other. Passwords and keys are never included.

use crate::{profiles::Profile, settings_vault, AppState};
use keyring::{Entry, Error as KeyringError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use tauri::{AppHandle, Manager, State};
use zeroize::Zeroizing;

const FORMAT: &str = "weave-team-vault";

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct TeamHost {
    profile: Profile,
    updated_at: i64,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct TeamFile {
    format: String,
    hosts: BTreeMap<String, TeamHost>,
    /// Hosts someone stopped sharing, with when.
    removed: BTreeMap<String, i64>,
}

/// What this Mac remembers between syncs.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct TeamState {
    shared: BTreeSet<String>,
    snapshot: BTreeMap<String, String>,
}

#[derive(Default, Debug)]
pub struct Merge {
    file_changed: bool,
    imports: Vec<Profile>,
    deletes: Vec<String>,
    conflicts: Vec<String>,
    published: usize,
}

/// Favorite is a personal choice, so it is not part of what is shared.
fn fingerprint(profile: &Profile) -> String {
    let mut shared = profile.clone();
    shared.favorite = false;
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&shared).unwrap_or_default())
    )
}

pub fn merge(
    file: &mut TeamFile,
    team: &mut TeamState,
    local: &BTreeMap<String, Profile>,
    now: i64,
) -> Merge {
    let mut result = Merge::default();
    let ids: BTreeSet<String> = team
        .shared
        .iter()
        .chain(team.snapshot.keys())
        .chain(file.hosts.keys())
        .cloned()
        .collect();
    for id in ids {
        let mine = local.get(&id).filter(|_| team.shared.contains(&id));
        let last = team.snapshot.get(&id).cloned();
        let theirs = file.hosts.get(&id).map(|host| fingerprint(&host.profile));
        match (mine, theirs) {
            (Some(profile), theirs) => {
                let current = fingerprint(profile);
                let changed_here = last.as_deref() != Some(current.as_str());
                let changed_there = theirs.is_some() && theirs != last;
                let removed_there =
                    theirs.is_none() && file.removed.contains_key(&id) && last.is_some();
                if theirs.as_deref() == Some(current.as_str()) {
                    team.snapshot.insert(id, current);
                } else if removed_there && !changed_here {
                    result.deletes.push(id.clone());
                    team.shared.remove(&id);
                    team.snapshot.remove(&id);
                } else if changed_there {
                    if changed_here {
                        result.conflicts.push(profile.name.clone());
                    }
                    let host = file.hosts[&id].clone();
                    team.snapshot.insert(id, fingerprint(&host.profile));
                    result.imports.push(host.profile);
                } else {
                    file.removed.remove(&id);
                    file.hosts.insert(
                        id.clone(),
                        TeamHost {
                            profile: profile.clone(),
                            updated_at: now,
                        },
                    );
                    team.snapshot.insert(id, current);
                    result.file_changed = true;
                    result.published += 1;
                }
            }
            (None, Some(_)) if team.snapshot.contains_key(&id) => {
                // Deleted or unshared here: take it out of the team file.
                file.hosts.remove(&id);
                file.removed.insert(id.clone(), now);
                team.shared.remove(&id);
                team.snapshot.remove(&id);
                result.file_changed = true;
            }
            (None, Some(_)) => {
                let host = file.hosts[&id].clone();
                team.shared.insert(id.clone());
                team.snapshot.insert(id, fingerprint(&host.profile));
                result.imports.push(host.profile);
            }
            _ => {
                team.shared.remove(&id);
                team.snapshot.remove(&id);
            }
        }
    }
    // Removal notes older than 90 days have reached everyone who still syncs.
    let before = file.removed.len();
    file.removed.retain(|_, when| now - *when < 90 * 86_400);
    result.file_changed |= before != file.removed.len();
    result
}

fn passphrase_entry() -> Result<Entry, String> {
    Entry::new("app.weave.desktop", "team-vault-passphrase")
        .map_err(|error| format!("Could not open the system keychain: {error}"))
}

fn setting(state: &AppState, key: &str) -> Result<Option<String>, String> {
    let db = state
        .db
        .lock()
        .map_err(|_| "Settings are busy. Try again.")?;
    db.query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
        row.get(0)
    })
    .optional()
    .map_err(|error| format!("Could not read settings: {error}"))
}

fn set_setting(state: &AppState, key: &str, value: &str) -> Result<(), String> {
    let db = state
        .db
        .lock()
        .map_err(|_| "Settings are busy. Try again.")?;
    db.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [key, value],
    )
    .map(drop)
    .map_err(|error| format!("Could not save settings: {error}"))
}

fn team_state(state: &AppState) -> Result<TeamState, String> {
    Ok(setting(state, "team_vault.state")?
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamStatus {
    path: Option<String>,
    shared: Vec<String>,
    has_passphrase: bool,
}

#[tauri::command]
pub fn team_vault_status(state: State<'_, AppState>) -> Result<TeamStatus, String> {
    Ok(TeamStatus {
        path: setting(&state, "team_vault.path")?,
        shared: team_state(&state)?.shared.into_iter().collect(),
        has_passphrase: passphrase_entry()?.get_password().is_ok(),
    })
}

/// Chooses the team file and passphrase. An existing file must open with it.
#[tauri::command]
pub fn team_vault_configure(
    state: State<'_, AppState>,
    path: String,
    passphrase: Option<String>,
) -> Result<(), String> {
    let file = Path::new(&path);
    if !file.is_absolute()
        || file
            .extension()
            .is_none_or(|extension| extension != "weaveteam")
    {
        return Err(
            "Choose a file name ending in .weaveteam inside a folder your team syncs.".into(),
        );
    }
    if !file.parent().is_some_and(Path::is_dir) {
        return Err("That folder does not exist.".into());
    }
    let passphrase = match passphrase.filter(|value| !value.is_empty()) {
        Some(value) => Zeroizing::new(value),
        None => Zeroizing::new(
            passphrase_entry()?
                .get_password()
                .map_err(|_| "Enter the team passphrase.")?,
        ),
    };
    if file.exists() {
        let text = fs::read_to_string(file)
            .map_err(|error| format!("Could not read the team vault: {error}"))?;
        settings_vault::open::<TeamFile>(&text, &passphrase)?;
    } else if passphrase.chars().count() < 10 {
        return Err("Use a team passphrase of at least 10 characters.".into());
    }
    passphrase_entry()?
        .set_password(&passphrase)
        .map_err(|error| {
            format!("Could not save the passphrase in the system keychain: {error}")
        })?;
    set_setting(&state, "team_vault.path", &path)
}

#[tauri::command]
pub fn team_vault_disconnect(state: State<'_, AppState>) -> Result<(), String> {
    let db = state
        .db
        .lock()
        .map_err(|_| "Settings are busy. Try again.")?;
    db.execute(
        "DELETE FROM settings WHERE key IN ('team_vault.path', 'team_vault.state')",
        [],
    )
    .map_err(|error| format!("Could not change settings: {error}"))?;
    match passphrase_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(format!("Could not remove the passphrase: {error}")),
    }
}

#[tauri::command]
pub fn team_vault_share(
    state: State<'_, AppState>,
    profile_id: String,
    shared: bool,
) -> Result<(), String> {
    let mut team = team_state(&state)?;
    if shared {
        team.shared.insert(profile_id);
    } else {
        team.shared.remove(&profile_id);
    }
    set_setting(
        &state,
        "team_vault.state",
        &serde_json::to_string(&team).map_err(|error| error.to_string())?,
    )
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamReport {
    published: usize,
    imported: usize,
    removed: usize,
    conflicts: Vec<String>,
}

pub fn sync(app: &AppHandle) -> Result<Option<TeamReport>, String> {
    let state = app.state::<AppState>();
    let Some(path) = setting(&state, "team_vault.path")? else {
        return Ok(None);
    };
    let passphrase = Zeroizing::new(passphrase_entry()?.get_password().map_err(|_| {
        "The team passphrase is missing from the system keychain. Set up the team vault again."
    })?);
    let mut file = match fs::read_to_string(&path) {
        Ok(text) => settings_vault::open::<TeamFile>(&text, &passphrase)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => TeamFile {
            format: FORMAT.into(),
            ..TeamFile::default()
        },
        Err(error) => return Err(format!("Could not read the team vault: {error}")),
    };
    if file.format != FORMAT {
        return Err("This file is not a Weave team vault.".into());
    }
    let local: BTreeMap<String, Profile> = crate::profiles::profiles_list(app.state())?
        .into_iter()
        .map(|profile| (profile.id.clone(), profile))
        .collect();
    let mut team = team_state(&state)?;
    let outcome = merge(&mut file, &mut team, &local, chrono::Utc::now().timestamp());
    let mut imported = 0;
    for mut profile in outcome.imports {
        profile.favorite = local
            .get(&profile.id)
            .is_some_and(|existing| existing.favorite);
        if crate::profiles::profiles_save(app.state(), profile).is_ok() {
            imported += 1;
        }
    }
    for id in &outcome.deletes {
        let _ = crate::profiles::profiles_delete(app.state(), id.clone());
    }
    if outcome.file_changed {
        let sealed = settings_vault::seal(&file, &passphrase)?;
        let temporary = format!("{path}.{}.tmp", uuid::Uuid::new_v4().simple());
        fs::write(&temporary, sealed)
            .and_then(|()| fs::rename(&temporary, &path))
            .map_err(|error| format!("Could not save the team vault: {error}"))?;
    }
    set_setting(
        &state,
        "team_vault.state",
        &serde_json::to_string(&team).map_err(|error| error.to_string())?,
    )?;
    Ok(Some(TeamReport {
        published: outcome.published,
        imported,
        removed: outcome.deletes.len(),
        conflicts: outcome.conflicts,
    }))
}

/// Keeps the team vault in step while Weave runs.
pub fn start(app: AppHandle) {
    std::thread::spawn(move || loop {
        if let Ok(Some(report)) = sync(&app) {
            if report.published + report.imported + report.removed > 0
                || !report.conflicts.is_empty()
            {
                let _ = tauri::Emitter::emit(&app, "team-vault-synced", &report);
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(120));
    });
}

#[tauri::command]
pub async fn team_vault_sync(app: AppHandle) -> Result<Option<TeamReport>, String> {
    tauri::async_runtime::spawn_blocking(move || sync(&app))
        .await
        .map_err(|error| format!("Team sync stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(id: &str, name: &str) -> Profile {
        Profile {
            id: id.into(),
            name: name.into(),
            host: format!("{name}.example.com"),
            protocol: "ssh".into(),
            port: 22,
            username: "deploy".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        }
    }

    fn hosts(list: &[Profile]) -> BTreeMap<String, Profile> {
        list.iter()
            .map(|profile| (profile.id.clone(), profile.clone()))
            .collect()
    }

    #[test]
    fn two_macs_share_edit_and_remove_hosts() {
        let mut file = TeamFile {
            format: FORMAT.into(),
            ..TeamFile::default()
        };
        let mut ada = TeamState::default();
        let mut bob = TeamState::default();
        let web = host("web", "web");
        ada.shared.insert("web".into());
        let first = merge(
            &mut file,
            &mut ada,
            &hosts(&[web.clone(), host("private", "private")]),
            100,
        );
        assert_eq!(first.published, 1);
        assert!(file.hosts.contains_key("web") && !file.hosts.contains_key("private"));

        let received = merge(&mut file, &mut bob, &hosts(&[]), 110);
        assert_eq!(received.imports, vec![web.clone()]);
        assert!(!received.file_changed);

        // Bob renames it; Ada picks the change up without touching her favorite.
        let mut renamed = web.clone();
        renamed.name = "web (prod)".into();
        let published = merge(&mut file, &mut bob, &hosts(&[renamed.clone()]), 120);
        assert_eq!(published.published, 1);
        let mut favorite = web.clone();
        favorite.favorite = true;
        let updated = merge(&mut file, &mut ada, &hosts(&[favorite]), 130);
        assert_eq!(updated.imports[0].name, "web (prod)");
        assert!(updated.conflicts.is_empty());

        // Both edit: the team file wins and the conflict is reported.
        let mut ada_edit = renamed.clone();
        ada_edit.port = 2222;
        let mut bob_edit = renamed.clone();
        bob_edit.username = "ops".into();
        merge(&mut file, &mut bob, &hosts(&[bob_edit.clone()]), 140);
        let conflict = merge(&mut file, &mut ada, &hosts(&[ada_edit]), 150);
        assert_eq!(conflict.conflicts, ["web (prod)"]);
        assert_eq!(conflict.imports, vec![bob_edit.clone()]);

        // Ada stops sharing it; Bob's copy is removed on his next sync.
        ada.shared.remove("web");
        let unshared = merge(&mut file, &mut ada, &hosts(&[bob_edit.clone()]), 160);
        assert!(unshared.file_changed && file.removed.contains_key("web"));
        let removed = merge(&mut file, &mut bob, &hosts(&[bob_edit]), 170);
        assert_eq!(removed.deletes, ["web"]);
        assert!(!bob.shared.contains("web"));
    }

    #[test]
    fn forgets_old_removal_notes() {
        let mut file = TeamFile {
            format: FORMAT.into(),
            removed: BTreeMap::from([("old".into(), 0)]),
            ..TeamFile::default()
        };
        let outcome = merge(
            &mut file,
            &mut TeamState::default(),
            &BTreeMap::new(),
            91 * 86_400,
        );
        assert!(outcome.file_changed && file.removed.is_empty());
    }

    #[test]
    fn team_files_are_encrypted() {
        let mut file = TeamFile {
            format: FORMAT.into(),
            ..TeamFile::default()
        };
        file.hosts.insert(
            "a".into(),
            TeamHost {
                profile: host("a", "secret-box"),
                updated_at: 1,
            },
        );
        let sealed = settings_vault::seal(&file, "team passphrase 1").unwrap();
        assert!(!sealed.contains("secret-box"));
        let opened: TeamFile = settings_vault::open(&sealed, "team passphrase 1").unwrap();
        assert_eq!(opened.hosts["a"].profile.name, "secret-box");
    }
}
