use crate::AppState;
use quick_xml::{events::Event, Reader};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs};
use tauri::State;

pub fn migrate_profiles(db: &rusqlite::Connection) -> rusqlite::Result<()> {
    let columns = {
        let mut statement = db.prepare("PRAGMA table_info(profiles)")?;
        let names = statement.query_map([], |row| row.get::<_, String>(1))?;
        names.collect::<Result<HashSet<_>, _>>()?
    };
    if !columns.contains("color") {
        db.execute(
            "ALTER TABLE profiles ADD COLUMN color TEXT NOT NULL DEFAULT '#9d91ec'",
            [],
        )?;
    }
    if !columns.contains("favorite") {
        db.execute(
            "ALTER TABLE profiles ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !columns.contains("protocol") {
        db.execute(
            "ALTER TABLE profiles ADD COLUMN protocol TEXT NOT NULL DEFAULT 'ssh'",
            [],
        )?;
    }
    if !columns.contains("tls_pin_sha256") {
        db.execute("ALTER TABLE profiles ADD COLUMN tls_pin_sha256 TEXT", [])?;
    }
    if !columns.contains("s3_region") {
        db.execute("ALTER TABLE profiles ADD COLUMN s3_region TEXT", [])?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub host: String,
    pub protocol: String,
    pub port: u16,
    pub username: String,
    pub key_path: Option<String>,
    pub jump_host: Option<String>,
    pub color: String,
    pub favorite: bool,
    #[serde(default)]
    pub tls_pin_sha256: Option<String>,
    #[serde(default)]
    pub s3_region: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    pub path: String,
    pub name: String,
}

pub fn get_profile(state: &AppState, id: &str) -> Result<Profile, String> {
    let mut profile = stored_profile(state, id)?;
    if profile.protocol == "s3" {
        if let Some(credentials) = crate::aws_credentials::credentials_for(state, id)? {
            profile.username = credentials.access_key;
        }
    }
    Ok(profile)
}

fn stored_profile(state: &AppState, id: &str) -> Result<Profile, String> {
    use rusqlite::OptionalExtension;
    let db = state.db.lock().map_err(|_| "Profile database is busy")?;
    db.query_row("SELECT id, name, host, protocol, port, username, key_path, jump_host, color, favorite, tls_pin_sha256, s3_region FROM profiles WHERE id=?1", [id], |row| {
        Ok(Profile {
            id: row.get(0)?, name: row.get(1)?, host: row.get(2)?, protocol: row.get(3)?, port: row.get(4)?,
            username: row.get(5)?, key_path: row.get(6)?, jump_host: row.get(7)?,
            color: row.get(8)?, favorite: row.get::<_, i64>(9)? != 0, tls_pin_sha256: row.get(10)?, s3_region: row.get(11)?,
        })
    }).optional().map_err(|error| format!("Could not load host: {error}"))?
        .ok_or_else(|| "This host profile no longer exists.".into())
}

#[tauri::command]
pub fn profiles_list(state: State<'_, AppState>) -> Result<Vec<Profile>, String> {
    let db = state.db.lock().map_err(|_| "Profile database is busy")?;
    let mut statement = db.prepare("SELECT id, name, host, protocol, port, username, key_path, jump_host, color, favorite, tls_pin_sha256, s3_region FROM profiles ORDER BY favorite DESC, name COLLATE NOCASE")
        .map_err(|error| format!("Could not load profiles: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(Profile {
                id: row.get(0)?,
                name: row.get(1)?,
                host: row.get(2)?,
                protocol: row.get(3)?,
                port: row.get(4)?,
                username: row.get(5)?,
                key_path: row.get(6)?,
                jump_host: row.get(7)?,
                color: row.get(8)?,
                favorite: row.get::<_, i64>(9)? != 0,
                tls_pin_sha256: row.get(10)?,
                s3_region: row.get(11)?,
            })
        })
        .map_err(|error| format!("Could not load profiles: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Could not read a profile: {error}"))
}

#[tauri::command]
pub fn profiles_save(state: State<'_, AppState>, mut profile: Profile) -> Result<Profile, String> {
    use rusqlite::OptionalExtension;
    profile.name = profile.name.trim().to_string();
    profile.host = profile.host.trim().to_string();
    profile.username = profile.username.trim().to_string();
    if let Some(provider) = crate::cloud::Provider::from_protocol(&profile.protocol) {
        profile.host = provider.host().into();
        profile.port = 443;
        if profile.username.is_empty() {
            profile.username = "not-signed-in".into();
        }
    }
    if profile.protocol == "azure" {
        let (_, account) = crate::azure::container_url(&profile.host)?;
        if profile.username.is_empty() {
            profile.username = account;
        }
        profile.port = 443;
    }
    profile.tls_pin_sha256 = match profile.tls_pin_sha256.take() {
        Some(pins) => crate::tls_pins::normalize(&pins)?,
        None => None,
    };
    profile.s3_region = profile
        .s3_region
        .take()
        .map(|region| region.trim().to_string());
    if profile.protocol != "s3" {
        profile.s3_region = None;
    }
    if profile.name.is_empty() || profile.host.is_empty() || profile.username.is_empty() {
        return Err("Name, host, and username are required.".into());
    }
    if profile.port == 0 {
        return Err("Port must be between 1 and 65535.".into());
    }
    validate_profile(&profile)?;
    if profile.id.is_empty() {
        profile.id = uuid::Uuid::new_v4().to_string();
    }
    let previous_protocol: Option<String> = {
        let db = state.db.lock().map_err(|_| "Profile database is busy")?;
        db.query_row(
            "SELECT protocol FROM profiles WHERE id=?1",
            [&profile.id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("Could not check the previous connection type: {error}"))?
    };
    if previous_protocol.is_some_and(|old| old != profile.protocol) {
        crate::vault::remove_profile_password(&state, &profile.id)?;
    }
    let db = state.db.lock().map_err(|_| "Profile database is busy")?;
    db.execute("INSERT INTO profiles (id, name, host, protocol, port, username, key_path, jump_host, color, favorite, tls_pin_sha256, s3_region)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        ON CONFLICT(id) DO UPDATE SET name=excluded.name, host=excluded.host, protocol=excluded.protocol, port=excluded.port,
        username=excluded.username, key_path=excluded.key_path, jump_host=excluded.jump_host,
        color=excluded.color, favorite=excluded.favorite, tls_pin_sha256=excluded.tls_pin_sha256, s3_region=excluded.s3_region",
        params![profile.id, profile.name, profile.host, profile.protocol, profile.port, profile.username,
            profile.key_path, profile.jump_host, profile.color, profile.favorite, profile.tls_pin_sha256, profile.s3_region])
        .map_err(|error| format!("Could not save profile: {error}"))?;
    Ok(profile)
}

pub(crate) fn validate_profile(profile: &Profile) -> Result<(), String> {
    if ![
        "ssh",
        "ftp",
        "ftps",
        "ftps-implicit",
        "webdav",
        "s3",
        "azure",
        "dropbox",
        "gdrive",
        "onedrive",
    ]
    .contains(&profile.protocol.as_str())
    {
        return Err("Choose SSH, FTP, FTPS, WebDAV, S3, Azure Blob, Dropbox, Google Drive or OneDrive for this connection.".into());
    }
    if profile.protocol == "webdav" {
        crate::webdav::validate_url(&profile.host)?;
    }
    if profile.protocol == "s3" {
        crate::s3::validate_profile(profile)?;
    }
    if profile.protocol == "azure" {
        crate::azure::container_url(&profile.host)?;
    }
    if let Some(pin) = &profile.tls_pin_sha256 {
        if !matches!(
            profile.protocol.as_str(),
            "ftps" | "ftps-implicit" | "webdav"
        ) {
            return Err(
                "Certificate pinning is available for FTPS and HTTPS WebDAV profiles only.".into(),
            );
        }
        crate::tls_pins::parse_pins(Some(pin))?;
        if profile.protocol == "webdav" && !profile.host.starts_with("https://") {
            return Err("Certificate pinning requires an HTTPS WebDAV URL.".into());
        }
    }
    if profile.host.starts_with('-') || profile.host.chars().any(char::is_whitespace) {
        return Err("Host must be a hostname or address without spaces or leading dashes.".into());
    }
    if profile.username.starts_with('-') || profile.username.chars().any(char::is_whitespace) {
        return Err("Username cannot contain spaces or begin with a dash.".into());
    }
    if profile
        .jump_host
        .as_ref()
        .is_some_and(|jump| jump.starts_with('-') || jump.chars().any(char::is_whitespace))
    {
        return Err("Jump host cannot contain spaces or begin with a dash.".into());
    }
    if profile.color.len() != 7
        || !profile.color.starts_with('#')
        || !profile.color[1..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("Choose a valid six-digit accent color.".into());
    }
    Ok(())
}

#[tauri::command]
pub fn profiles_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let profile = get_profile(&state, &id)?;
    forget_profile(&state, &profile)
}

/// Deletes a host and everything Weave keeps for it: Keychain passwords and
/// tokens, its settings, tunnels, sync jobs, queued transfers and its shared
/// SSH connection.
pub(crate) fn forget_profile(state: &AppState, profile: &Profile) -> Result<(), String> {
    let id = profile.id.as_str();
    crate::vault::remove_profile_password(state, id)?;
    crate::cloud::forget_sign_in(id);
    crate::webdav::forget_client_identity(id);
    crate::tunnels::stop_for_profile(state, id);
    if profile.protocol == "ssh" && cfg!(unix) {
        if let Ok(socket) = crate::ssh::control_path(profile) {
            let _ = crate::platform::command("ssh")
                .arg("-o")
                .arg(format!("ControlPath={}", socket.display()))
                .args(["-O", "exit", "weave-forget"])
                .output();
            let _ = fs::remove_file(socket);
        }
    }
    let mut db = state.db.lock().map_err(|_| "Profile database is busy")?;
    let transaction = db
        .transaction()
        .map_err(|error| format!("Could not delete the host: {error}"))?;
    let run = |sql: &str| {
        transaction
            .execute(sql, [id])
            .map(drop)
            .map_err(|error| format!("Could not delete the host's data: {error}"))
    };
    run("DELETE FROM settings WHERE substr(key, -length(?1) - 1) = ':' || ?1")?;
    let has = |table: &str| {
        transaction
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0
    };
    if has("sync_jobs") {
        run("DELETE FROM sync_job_transfers WHERE job_id IN (SELECT id FROM sync_jobs WHERE left_profile=?1 OR right_profile=?1)")?;
        run("DELETE FROM sync_jobs WHERE left_profile=?1 OR right_profile=?1")?;
    }
    if has("transfers") {
        run("DELETE FROM transfers WHERE (source_profile=?1 OR destination_profile=?1) AND status <> 'running'")?;
    }
    run("DELETE FROM profiles WHERE id=?1")?;
    transaction
        .commit()
        .map_err(|error| format!("Could not delete the host: {error}"))
}

#[tauri::command]
pub fn profiles_import_ssh(state: State<'_, AppState>) -> Result<usize, String> {
    let config_path = dirs::home_dir()
        .ok_or("Home folder is unavailable.")?
        .join(".ssh/config");
    let config = fs::read_to_string(&config_path)
        .map_err(|error| format!("Could not read {}: {error}", config_path.display()))?;
    let aliases = ssh_aliases(&config);
    if aliases.is_empty() {
        return Err("No named hosts were found in ~/.ssh/config.".into());
    }
    let mut db = state.db.lock().map_err(|_| "Profile database is busy")?;
    let transaction = db
        .transaction()
        .map_err(|error| format!("Could not start import: {error}"))?;
    let mut imported = 0;
    for alias in aliases {
        let exists: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM profiles WHERE name=?1",
                [&alias],
                |row| row.get(0),
            )
            .map_err(|error| format!("Could not check existing hosts: {error}"))?;
        if exists != 0 {
            continue;
        }
        let output = crate::platform::command("ssh")
            .args(["-G", &alias])
            .output()
            .map_err(|error| format!("OpenSSH is unavailable: {error}"))?;
        if !output.status.success() {
            continue;
        }
        let resolved = String::from_utf8_lossy(&output.stdout);
        let field = |key: &str| {
            resolved
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{key} ")))
                .map(str::to_string)
        };
        let host = field("hostname").unwrap_or_else(|| alias.clone());
        let username = field("user").unwrap_or_else(|| std::env::var("USER").unwrap_or_default());
        let port = field("port")
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(22);
        let key_path = field("identityfile").filter(|value| value != "none");
        let jump_host = field("proxyjump").filter(|value| value != "none");
        let unsafe_field =
            |value: &str| value.starts_with('-') || value.chars().any(char::is_whitespace);
        if unsafe_field(&host)
            || unsafe_field(&username)
            || jump_host.as_deref().is_some_and(unsafe_field)
        {
            continue;
        }
        transaction.execute("INSERT INTO profiles (id, name, host, port, username, key_path, jump_host, color, favorite)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '#9d91ec', 0)",
            params![uuid::Uuid::new_v4().to_string(), alias, host, port, username, key_path, jump_host])
            .map_err(|error| format!("Could not import host {alias}: {error}"))?;
        imported += 1;
    }
    transaction
        .commit()
        .map_err(|error| format!("Could not save imported hosts: {error}"))?;
    Ok(imported)
}

#[derive(Default)]
struct FileZillaSite {
    name: String,
    fallback_name: String,
    host: String,
    port: String,
    protocol: String,
    logon_type: String,
    username: String,
    key_path: String,
}

impl FileZillaSite {
    fn into_profile(self) -> Option<Profile> {
        let name = if self.name.trim().is_empty() {
            self.fallback_name.trim()
        } else {
            self.name.trim()
        };
        let protocol = match self.protocol.as_str() {
            "0" | "4" => "ftps",
            "1" => "ssh",
            "3" => "ftps-implicit",
            "6" => "ftp",
            _ => return None,
        };
        let username = if self.logon_type.trim() == "0" && protocol != "ssh" {
            "anonymous"
        } else {
            self.username.trim()
        };
        let port = self.port.parse::<u16>().ok()?;
        if name.is_empty() || self.host.trim().is_empty() || username.is_empty() || port == 0 {
            return None;
        }
        let profile = Profile {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            host: self.host.trim().into(),
            protocol: protocol.into(),
            port,
            username: username.into(),
            key_path: (protocol == "ssh" && !self.key_path.trim().is_empty())
                .then(|| self.key_path.trim().into()),
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        validate_profile(&profile).ok()?;
        Some(profile)
    }
}

fn append_filezilla_value(site: &mut FileZillaSite, field: Option<&str>, value: &str) {
    match field {
        Some("Name") => site.name.push_str(value),
        Some("Host") => site.host.push_str(value),
        Some("Port") => site.port.push_str(value),
        Some("Protocol") => site.protocol.push_str(value),
        Some("Logontype") => site.logon_type.push_str(value),
        Some("User") => site.username.push_str(value),
        Some("Keyfile") => site.key_path.push_str(value),
        Some("Server") => site.fallback_name.push_str(value),
        _ => {}
    }
}

fn filezilla_sites(xml: &str) -> Result<Vec<FileZillaSite>, String> {
    if xml.len() > 5 * 1024 * 1024 {
        return Err(
            "FileZilla export is larger than 5 MB. Export a smaller Site Manager file.".into(),
        );
    }
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<String>::new();
    let mut sites = Vec::new();
    let mut current: Option<FileZillaSite> = None;
    let mut filezilla_root = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = String::from_utf8_lossy(event.local_name().as_ref()).to_string();
                if stack.is_empty() && name == "FileZilla3" {
                    filezilla_root = true;
                }
                if name == "Server" {
                    current = Some(FileZillaSite::default());
                }
                stack.push(name);
            }
            Ok(Event::Text(event)) => {
                let Some(site) = current.as_mut() else {
                    continue;
                };
                let value = event
                    .decode()
                    .map_err(|error| format!("Could not read FileZilla export text: {error}"))?;
                let value = quick_xml::escape::unescape(&value)
                    .map_err(|error| format!("Could not read FileZilla XML entity: {error}"))?;
                append_filezilla_value(site, stack.last().map(String::as_str), &value);
            }
            Ok(Event::GeneralRef(event)) => {
                let Some(site) = current.as_mut() else {
                    continue;
                };
                let value = if let Some(character) = event
                    .resolve_char_ref()
                    .map_err(|error| format!("Could not read FileZilla XML entity: {error}"))?
                {
                    character.to_string()
                } else {
                    let name = event
                        .decode()
                        .map_err(|error| format!("Could not read FileZilla XML entity: {error}"))?;
                    quick_xml::escape::resolve_xml_entity(&name)
                        .ok_or_else(|| {
                            format!("FileZilla export contains unknown XML entity: &{name};")
                        })?
                        .to_string()
                };
                append_filezilla_value(site, stack.last().map(String::as_str), &value);
            }
            Ok(Event::End(event)) => {
                if event.local_name().as_ref() == b"Server" {
                    if let Some(site) = current.take() {
                        sites.push(site);
                    }
                }
                stack.pop();
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("FileZilla export contains invalid XML: {error}")),
            _ => {}
        }
    }
    if !filezilla_root {
        return Err("Choose a FileZilla Site Manager XML export.".into());
    }
    Ok(sites)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileZillaImportReport {
    pub imported: usize,
    pub skipped: usize,
}

#[tauri::command]
pub fn profiles_import_filezilla(
    state: State<'_, AppState>,
    path: String,
) -> Result<FileZillaImportReport, String> {
    let size = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect FileZilla export {path}: {error}"))?
        .len();
    if size > 5 * 1024 * 1024 {
        return Err(
            "FileZilla export is larger than 5 MB. Export a smaller Site Manager file.".into(),
        );
    }
    let xml = fs::read_to_string(&path)
        .map_err(|error| format!("Could not read FileZilla export {path}: {error}"))?;
    let sites = filezilla_sites(&xml)?;
    if sites.is_empty() {
        return Err("This FileZilla export contains no saved sites.".into());
    }
    let mut db = state.db.lock().map_err(|_| "Profile database is busy")?;
    insert_filezilla_sites(&mut db, sites)
}

fn insert_filezilla_sites(
    db: &mut rusqlite::Connection,
    sites: Vec<FileZillaSite>,
) -> Result<FileZillaImportReport, String> {
    insert_imported_profiles(
        db,
        sites.into_iter().map(FileZillaSite::into_profile).collect(),
    )
}

/// Saves imported profiles, skipping unusable entries (`None`) and
/// connections that are already saved.
pub(crate) fn insert_imported_profiles(
    db: &mut rusqlite::Connection,
    profiles: Vec<Option<Profile>>,
) -> Result<FileZillaImportReport, String> {
    let mut report = FileZillaImportReport {
        imported: 0,
        skipped: 0,
    };
    let transaction = db
        .transaction()
        .map_err(|error| format!("Could not start the import: {error}"))?;
    for profile in profiles {
        let Some(profile) = profile else {
            report.skipped += 1;
            continue;
        };
        let exists: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM profiles WHERE name=?1 AND host=?2 AND port=?3 AND protocol=?4 AND username=?5",
            params![profile.name, profile.host, profile.port, profile.protocol, profile.username],
            |row| row.get(0),
        ).map_err(|error| format!("Could not check imported site {}: {error}", profile.name))?;
        if exists > 0 {
            report.skipped += 1;
            continue;
        }
        transaction.execute("INSERT INTO profiles (id, name, host, protocol, port, username, key_path, jump_host, color, favorite) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, 0)",
            params![profile.id, profile.name, profile.host, profile.protocol, profile.port, profile.username, profile.key_path, profile.color])
            .map_err(|error| format!("Could not import site {}: {error}", profile.name))?;
        report.imported += 1;
    }
    transaction
        .commit()
        .map_err(|error| format!("Could not save imported sites: {error}"))?;
    Ok(report)
}

fn ssh_aliases(config: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut aliases = Vec::new();
    for line in config.lines() {
        let mut parts = line.split_whitespace();
        if !parts
            .next()
            .is_some_and(|key| key.eq_ignore_ascii_case("host"))
        {
            continue;
        }
        for alias in parts.take_while(|part| !part.starts_with('#')) {
            if alias.contains(['*', '?', '!']) || !seen.insert(alias.to_string()) {
                continue;
            }
            aliases.push(alias.to_string());
        }
    }
    aliases
}

#[tauri::command]
pub fn repos_list(state: State<'_, AppState>) -> Result<Vec<Repo>, String> {
    let db = state.db.lock().map_err(|_| "Repository database is busy")?;
    let mut statement = db
        .prepare("SELECT path, name FROM repos ORDER BY last_opened DESC")
        .map_err(|error| format!("Could not load repositories: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(Repo {
                path: row.get(0)?,
                name: row.get(1)?,
            })
        })
        .map_err(|error| format!("Could not load repositories: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Could not read a repository: {error}"))
}

#[tauri::command]
pub fn repos_add(state: State<'_, AppState>, path: String) -> Result<Repo, String> {
    let canonical =
        std::fs::canonicalize(&path).map_err(|error| format!("Cannot open {path}: {error}"))?;
    let output = crate::platform::command("git")
        .arg("-C")
        .arg(&canonical)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("Git is not available: {error}"))?;
    if !output.status.success() {
        return Err("This folder is not inside a Git repository.".into());
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let name = std::path::Path::new(&root)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let db = state.db.lock().map_err(|_| "Repository database is busy")?;
    db.execute(
        "INSERT INTO repos (path, name, last_opened) VALUES (?1, ?2, unixepoch())
        ON CONFLICT(path) DO UPDATE SET last_opened=unixepoch()",
        params![root, name],
    )
    .map_err(|error| format!("Could not remember repository: {error}"))?;
    Ok(Repo { path: root, name })
}

#[tauri::command]
pub fn repos_remove(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "Repository database is busy")?;
    db.execute("DELETE FROM repos WHERE path=?1", [path])
        .map_err(|error| format!("Could not remove repository from recents: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        filezilla_sites, insert_filezilla_sites, migrate_profiles, ssh_aliases, validate_profile,
        Profile,
    };

    #[test]
    fn deleting_a_host_removes_its_settings_jobs_and_queued_transfers() {
        use super::{forget_profile, stored_profile};
        use crate::AppState;
        let state = AppState::for_tests();
        let id = format!("delete-test-{}", uuid::Uuid::new_v4());
        {
            let db = state.db.lock().unwrap();
            db.execute_batch(
                "CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, protocol TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT, color TEXT, favorite INTEGER, tls_pin_sha256 TEXT, s3_region TEXT);
                 CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, destination_profile TEXT, status TEXT);",
            )
            .unwrap();
            crate::sync_jobs::migrate(&db).unwrap();
            db.execute("INSERT INTO profiles VALUES (?1, 'HPC', 'hpc.example.edu', 'webdav', 443, 'ada', NULL, NULL, '', 0, NULL, NULL)", [&id]).unwrap();
            for key in [
                format!("host_options:{id}"),
                format!("s3.aws_profile:{id}"),
                format!("host_options:{id}x"),
                "theme".into(),
            ] {
                db.execute("INSERT INTO settings VALUES (?1, '1')", [&key])
                    .unwrap();
            }
            db.execute("INSERT INTO sync_jobs (id, name, left_profile, left_path, right_path, mode, interval_minutes, next_run) VALUES ('job', 'Job', ?1, '/a', '/b', 'left-to-right', 60, 0)", [&id]).unwrap();
            db.execute("INSERT INTO transfers VALUES ('queued', ?1, NULL, 'queued'), ('busy', NULL, ?1, 'running'), ('other', NULL, NULL, 'queued')", [&id]).unwrap();
        }
        state
            .session_passwords
            .lock()
            .unwrap()
            .insert(id.clone(), zeroize::Zeroizing::new("pw".into()));
        let profile = stored_profile(&state, &id).unwrap();
        forget_profile(&state, &profile).unwrap();
        let db = state.db.lock().unwrap();
        let count = |sql: &str| db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT COUNT(*) FROM profiles"), 0);
        let column = |sql: &str| -> Vec<String> {
            db.prepare(sql)
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(
            column("SELECT key FROM settings ORDER BY key"),
            vec![format!("host_options:{id}x"), "theme".to_string()]
        );
        assert_eq!(count("SELECT COUNT(*) FROM sync_jobs"), 0);
        assert_eq!(
            column("SELECT id FROM transfers ORDER BY id"),
            vec!["busy".to_string(), "other".to_string()]
        );
        assert!(!state.session_passwords.lock().unwrap().contains_key(&id));
    }

    #[test]
    fn imports_supported_filezilla_sites_without_passwords_or_duplicates() {
        let xml = r#"<?xml version="1.0"?><FileZilla3><Servers>
            <Server><Host>ftp.example.test</Host><Port>21</Port><Protocol>0</Protocol><User>alice</User><Pass encoding="base64">c2VjcmV0</Pass><Name>Secure &amp; shared</Name></Server>
            <Server><Host>ssh.example.test</Host><Port>22</Port><Protocol>1</Protocol><User>bob</User><Keyfile>/tmp/id_ed25519</Keyfile><Name>SSH</Name></Server>
            <Server><Host>legacy.example.test</Host><Port>990</Port><Protocol>3</Protocol><User>carol</User><Name>Legacy TLS</Name></Server>
            <Server><Host>plain.example.test</Host><Port>21</Port><Protocol>6</Protocol><User>dave</User><Name>Plain FTP</Name></Server>
            <Server><Host>public.example.test</Host><Port>21</Port><Protocol>6</Protocol><Logontype>0</Logontype><Name>Public files</Name></Server>
            <Server><Host>cloud.example.test</Host><Port>443</Port><Protocol>7</Protocol><User>eve</User><Name>Cloud</Name></Server>
        </Servers></FileZilla3>"#;
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, protocol TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT, color TEXT, favorite INTEGER, tls_pin_sha256 TEXT, s3_region TEXT);").unwrap();
        let report = insert_filezilla_sites(&mut db, filezilla_sites(xml).unwrap()).unwrap();
        assert_eq!((report.imported, report.skipped), (5, 1));
        let mapped: Vec<(String, String, String)> = db
            .prepare("SELECT name, protocol, username FROM profiles ORDER BY name")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            mapped,
            vec![
                ("Legacy TLS".into(), "ftps-implicit".into(), "carol".into()),
                ("Plain FTP".into(), "ftp".into(), "dave".into()),
                ("Public files".into(), "ftp".into(), "anonymous".into()),
                ("SSH".into(), "ssh".into(), "bob".into()),
                ("Secure & shared".into(), "ftps".into(), "alice".into())
            ]
        );
        let report = insert_filezilla_sites(&mut db, filezilla_sites(xml).unwrap()).unwrap();
        assert_eq!((report.imported, report.skipped), (0, 6));
        let password_columns: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('profiles') WHERE name='password'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(password_columns, 0);
    }

    #[test]
    fn reads_filezilla_site_names_stored_as_server_text() {
        let xml = r#"<FileZilla3><Servers><Server><Host>files.example.test</Host><Port>21</Port><Protocol>4</Protocol><User>alice</User>Team &amp; archive</Server></Servers></FileZilla3>"#;
        let site = filezilla_sites(xml).unwrap().pop().unwrap();
        let profile = site.into_profile().unwrap();
        assert_eq!(profile.name, "Team & archive");
        assert_eq!(profile.protocol, "ftps");
    }

    #[test]
    fn adds_profile_fields_to_an_older_database() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT);
             INSERT INTO profiles VALUES ('one', 'Server', 'server.example', 22, 'alice', NULL, NULL);",
        )
        .unwrap();
        migrate_profiles(&db).unwrap();
        migrate_profiles(&db).unwrap();
        let result: (String, i64, String) = db
            .query_row(
                "SELECT color, favorite, protocol FROM profiles WHERE id='one'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(result, ("#9d91ec".into(), 0, "ssh".into()));
        let pin: Option<String> = db
            .query_row(
                "SELECT tls_pin_sha256 FROM profiles WHERE id='one'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(pin.is_none());
        let region: Option<String> = db
            .query_row("SELECT s3_region FROM profiles WHERE id='one'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(region.is_none());
    }

    #[test]
    fn certificate_pin_requires_tls_and_sha256_hex() {
        let mut profile = Profile {
            id: String::new(),
            name: "Server".into(),
            host: "server.example.test".into(),
            protocol: "ftps".into(),
            port: 21,
            username: "alice".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: Some("ab".repeat(32)),
            s3_region: None,
        };
        assert!(validate_profile(&profile).is_ok());
        profile.tls_pin_sha256 = Some("not a fingerprint".into());
        assert!(validate_profile(&profile).unwrap_err().contains("SHA-256"));
        profile.tls_pin_sha256 = Some("ab".repeat(32));
        profile.protocol = "ftp".into();
        assert!(validate_profile(&profile).unwrap_err().contains("FTPS"));
        profile.protocol = "webdav".into();
        profile.host = "http://127.0.0.1:8080/dav/".into();
        assert!(validate_profile(&profile).unwrap_err().contains("HTTPS"));
        profile.host = "https://server.example.test/dav/".into();
        assert!(validate_profile(&profile).is_ok());
    }

    #[test]
    fn parses_named_ssh_hosts_without_patterns_or_duplicates() {
        let config = "Host *\n  User alice\nHost hpc login # cluster\nHost hpc\nHost !blocked *.example.com\n";
        assert_eq!(ssh_aliases(config), vec!["hpc", "login"]);
    }

    #[test]
    fn rejects_unsafe_host_arguments() {
        let mut profile = Profile {
            id: String::new(),
            name: "Test".into(),
            host: "server.example".into(),
            protocol: "ssh".into(),
            port: 22,
            username: "alice".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        assert!(validate_profile(&profile).is_ok());
        profile.host = "-oProxyCommand=bad".into();
        assert!(validate_profile(&profile).unwrap_err().contains("Host"));
    }
}
