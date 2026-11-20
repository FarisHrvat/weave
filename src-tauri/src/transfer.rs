use crate::{
    ftp::{FtpClient, FtpData},
    profiles,
    s3::{S3Client, S3Upload},
    sftp::{join_remote, SftpClient},
    vault,
    webdav::{WebDavClient, WebDavUpload},
    AppState,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

const MAX_EXTERNAL_EDIT_SIZE: u64 = 512 * 1024 * 1024;

pub struct ExternalEditSession {
    directory: std::path::PathBuf,
    profile_id: String,
    remote_path: String,
    expected_hash: [u8; 32],
    /// Upload automatically whenever the editing copy is saved.
    auto_upload: bool,
}

#[derive(Deserialize, Serialize)]
struct ExternalEditRecord {
    id: String,
    profile_id: String,
    remote_path: String,
    expected_hash: [u8; 32],
    #[serde(default)]
    auto_upload: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalEditInfo {
    pub(crate) id: String,
    pub(crate) local_path: String,
    pub(crate) remote_path: String,
    pub(crate) changed: bool,
    pub(crate) warning: Option<String>,
    pub(crate) auto_upload: bool,
}

fn edit_info(
    id: String,
    session: &ExternalEditSession,
    changed: bool,
    warning: Option<String>,
) -> ExternalEditInfo {
    ExternalEditInfo {
        id,
        local_path: session.local_path().to_string_lossy().into_owned(),
        remote_path: session.remote_path.clone(),
        changed,
        warning,
        auto_upload: session.auto_upload,
    }
}

impl ExternalEditSession {
    fn local_path(&self) -> std::path::PathBuf {
        self.directory.join("file").join(
            Path::new(&self.remote_path)
                .file_name()
                .expect("checkout always has a filename"),
        )
    }

    fn save_record(&self, id: &str) -> Result<(), String> {
        let record = ExternalEditRecord {
            id: id.into(),
            profile_id: self.profile_id.clone(),
            remote_path: self.remote_path.clone(),
            expected_hash: self.expected_hash,
            auto_upload: self.auto_upload,
        };
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)
            .map_err(|error| format!("Could not save the editing session: {error}"))?;
        serde_json::to_writer(&mut temporary, &record)
            .map_err(|error| format!("Could not write the editing session: {error}"))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| format!("Could not save the editing session: {error}"))?;
        temporary
            .persist(self.directory.join("session.json"))
            .map_err(|error| format!("Could not replace the editing session record: {error}"))?;
        Ok(())
    }
}

pub fn restore_external_edits(root: &Path) -> Result<HashMap<String, ExternalEditSession>, String> {
    fs::create_dir_all(root)
        .map_err(|error| format!("Could not open saved editing copies: {error}"))?;
    let mut sessions = HashMap::new();
    for entry in fs::read_dir(root)
        .map_err(|error| format!("Could not list saved editing copies: {error}"))?
    {
        let entry = entry.map_err(|error| format!("Could not read an editing copy: {error}"))?;
        if !entry
            .file_type()
            .map_err(|error| format!("Could not inspect an editing copy: {error}"))?
            .is_dir()
        {
            continue;
        }
        let record_path = entry.path().join("session.json");
        if !fs::symlink_metadata(&record_path).is_ok_and(|metadata| metadata.is_file()) {
            continue;
        }
        let Ok(contents) = fs::read(&record_path) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<ExternalEditRecord>(&contents) else {
            continue;
        };
        if uuid::Uuid::parse_str(&record.id).is_err()
            || Path::new(&record.remote_path).file_name().is_none()
        {
            continue;
        }
        let session = ExternalEditSession {
            directory: entry.path(),
            profile_id: record.profile_id,
            remote_path: record.remote_path,
            expected_hash: record.expected_hash,
            auto_upload: record.auto_upload,
        };
        if fs::symlink_metadata(session.local_path()).is_ok_and(|metadata| metadata.is_file()) {
            sessions.insert(record.id, session);
        }
    }
    Ok(sessions)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferRecord {
    pub id: String,
    pub source_profile: Option<String>,
    pub source_path: String,
    pub destination_profile: Option<String>,
    pub destination_dir: String,
    pub target_path: Option<String>,
    pub conflict: String,
    pub status: String,
    pub bytes_done: u64,
    pub total_bytes: u64,
    pub error: Option<String>,
    pub created_at: i64,
    pub queue_order: i64,
    /// Empty for the built-in engine, or "scp" / "rsync" for OpenSSH tools.
    pub method: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSettings {
    pub parallelism: usize,
    pub speed_limit_kbps: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchTransfer {
    source_profile: Option<String>,
    source_path: String,
    destination_profile: Option<String>,
    destination_dir: String,
    conflict: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchManifest {
    version: u8,
    transfers: Vec<BatchTransfer>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchPreview {
    sha256: String,
    transfers: Vec<BatchTransfer>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchQueueResult {
    records: Vec<TransferRecord>,
    warning: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncCopy {
    pub source_profile: Option<String>,
    pub source_path: String,
    pub destination_profile: Option<String>,
    pub destination_dir: String,
    pub reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDelete {
    pub destination_profile: Option<String>,
    pub path: String,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlan {
    pub copies: Vec<SyncCopy>,
    pub deletes: Vec<SyncDelete>,
    pub issues: Vec<String>,
}

fn scan_links(source: &mut Backend, folder: &str, visited: &mut usize) -> Result<bool, String> {
    for item in source.list(folder)? {
        *visited += 1;
        if *visited > 20_000 {
            return Err("This folder has more than 20,000 items. Compare a smaller folder.".into());
        }
        if item.is_symlink || (item.is_dir && scan_links(source, &item.path, visited)?) {
            return Ok(true);
        }
    }
    Ok(false)
}

struct SyncContext<'a> {
    source_profile: &'a Option<String>,
    destination_profile: &'a Option<String>,
    plan: &'a mut SyncPlan,
    visited: &'a mut usize,
    mirror: bool,
    /// Name masks to leave out on both sides, such as "*.tmp; node_modules".
    exclude: &'a str,
}

fn plan_one_way(
    source: &mut Backend,
    destination: &mut Backend,
    source_dir: &str,
    destination_dir: &str,
    context: &mut SyncContext<'_>,
) -> Result<(), String> {
    let excluded =
        |name: &str| !context.exclude.trim().is_empty() && mask_matches(context.exclude, name);
    let destination_items = destination
        .list(destination_dir)?
        .into_iter()
        .filter(|item| !excluded(&item.name))
        .map(|item| (item.name.clone(), item))
        .collect::<HashMap<_, _>>();
    let mut source_names = HashSet::new();
    for item in source
        .list(source_dir)?
        .into_iter()
        .filter(|item| !excluded(&item.name))
    {
        source_names.insert(item.name.clone());
        *context.visited += 1;
        if *context.visited > 20_000 {
            return Err("This folder has more than 20,000 items. Compare a smaller folder.".into());
        }
        if item.is_symlink {
            context.plan.issues.push(format!(
                "{}: symbolic links need manual handling",
                item.path
            ));
            continue;
        }
        let target = destination.path(destination_dir, &item.name);
        match destination_items.get(&item.name) {
            None => {
                if item.is_dir && scan_links(source, &item.path, context.visited)? {
                    context.plan.issues.push(format!(
                        "{}: folder contains a symbolic link; copy it manually",
                        item.path
                    ));
                    continue;
                }
                context.plan.copies.push(SyncCopy {
                    source_profile: context.source_profile.clone(),
                    source_path: item.path,
                    destination_profile: context.destination_profile.clone(),
                    destination_dir: destination_dir.into(),
                    reason: "Missing at destination".into(),
                });
            }
            Some(other) if other.is_symlink || item.is_dir != other.is_dir => {
                context.plan.issues.push(format!(
                    "{}: source and destination have different item types",
                    item.path
                ));
            }
            Some(_other) if item.is_dir => {
                plan_one_way(source, destination, &item.path, &target, context)?;
            }
            Some(other) => match (item.modified, other.modified) {
                (Some(source_time), Some(destination_time)) if source_time > destination_time => {
                    if item.size == other.size {
                        let active = AtomicBool::new(true);
                        if checksum(source, &item, &active)?
                            == checksum(destination, other, &active)?
                        {
                            continue;
                        }
                    }
                    context.plan.copies.push(SyncCopy {
                        source_profile: context.source_profile.clone(),
                        source_path: item.path,
                        destination_profile: context.destination_profile.clone(),
                        destination_dir: destination_dir.into(),
                        reason: "Source is newer".into(),
                    });
                }
                (Some(source_time), Some(destination_time)) if source_time < destination_time => {}
                _ if item.size != other.size => {
                    context.plan.issues.push(format!(
                        "{}: same or unknown modification time, but different sizes",
                        item.path
                    ));
                }
                _ => {}
            },
        }
    }
    if context.mirror {
        for item in destination_items.into_values() {
            if source_names.contains(&item.name) {
                continue;
            }
            *context.visited += 1;
            if *context.visited > 20_000 {
                return Err(
                    "This folder has more than 20,000 items. Compare a smaller folder.".into(),
                );
            }
            if item.is_symlink
                || (item.is_dir && scan_links(destination, &item.path, context.visited)?)
            {
                context.plan.issues.push(format!(
                    "{}: destination-only symbolic link needs manual handling",
                    item.path
                ));
                continue;
            }
            context.plan.deletes.push(SyncDelete {
                destination_profile: context.destination_profile.clone(),
                path: item.path,
            });
        }
    }
    Ok(())
}

/// Reads up to `max` bytes from the start of a file on any connection.
pub(crate) fn read_bytes(
    state: &AppState,
    profile_id: &Option<String>,
    path: &str,
    max: u64,
) -> Result<(Vec<u8>, u64), String> {
    let mut backend = open_backend(state, profile_id)?;
    let item = backend.stat(path)?;
    if item.is_dir {
        return Err("Choose a file to preview.".into());
    }
    let wanted = item.size.min(max);
    let mut handle = backend.open_read(path, 0)?;
    let mut data = Vec::with_capacity(wanted as usize);
    while (data.len() as u64) < wanted {
        let chunk = backend.read_chunk(
            &mut handle,
            data.len() as u64,
            (wanted as usize - data.len()).min(256 * 1024),
        )?;
        if chunk.is_empty() {
            break;
        }
        data.extend_from_slice(&chunk);
    }
    backend.close(handle, false)?;
    Ok((data, item.size))
}

/// True when `name` matches a mask such as `*.log` or `report-??.pdf`
/// (case-insensitive; several masks may be separated by `;`).
pub(crate) fn mask_matches(mask: &str, name: &str) -> bool {
    fn matches(pattern: &[char], text: &[char]) -> bool {
        match (pattern.first(), text.first()) {
            (None, None) => true,
            (Some('*'), _) => {
                matches(&pattern[1..], text) || (!text.is_empty() && matches(pattern, &text[1..]))
            }
            (Some('?'), Some(_)) => matches(&pattern[1..], &text[1..]),
            (Some(p), Some(t)) if p == t => matches(&pattern[1..], &text[1..]),
            _ => false,
        }
    }
    let name: Vec<char> = name.to_lowercase().chars().collect();
    mask.split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .any(|part| {
            let pattern: Vec<char> = part.to_lowercase().chars().collect();
            matches(&pattern, &name)
        })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundFile {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

/// Searches a folder tree on any connection for names matching `mask`.
pub(crate) fn folder_is_empty(
    state: &AppState,
    profile: &Option<String>,
    folder: &str,
) -> Result<bool, String> {
    Ok(open_backend(state, profile)?.list(folder)?.is_empty())
}

pub(crate) fn find_files(
    state: &AppState,
    profile_id: &Option<String>,
    root: &str,
    mask: &str,
    limit: usize,
) -> Result<(Vec<FoundFile>, bool), String> {
    if mask.trim().is_empty() {
        return Err("Enter a name or pattern, such as *.log.".into());
    }
    let mask = if mask.contains(['*', '?']) {
        mask.to_string()
    } else {
        format!("*{}*", mask.trim())
    };
    let mut backend = open_backend(state, profile_id)?;
    let mut found = Vec::new();
    let mut stack = vec![root.to_string()];
    let mut visited = 0;
    while let Some(directory) = stack.pop() {
        let Ok(items) = backend.list(&directory) else {
            continue;
        };
        for item in items {
            visited += 1;
            if mask_matches(&mask, &item.name) {
                found.push(FoundFile {
                    path: item.path.clone(),
                    name: item.name.clone(),
                    is_dir: item.is_dir,
                    size: item.size,
                    modified: item.modified,
                });
                if found.len() >= limit {
                    return Ok((found, true));
                }
            }
            if item.is_dir && !item.is_symlink {
                stack.push(item.path);
            }
        }
        if visited > 200_000 {
            return Ok((found, true));
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((found, false))
}

/// A digest of a folder tree's names, types, sizes and modification times on
/// any connection, used to notice changes in a sync job's source.
pub(crate) fn folder_fingerprint(
    state: &AppState,
    profile_id: &Option<String>,
    folder: &str,
) -> Result<String, String> {
    let mut backend = open_backend(state, profile_id)?;
    let mut digest = Sha256::new();
    let mut stack = vec![folder.to_string()];
    let mut visited = 0;
    while let Some(directory) = stack.pop() {
        let mut items = backend.list(&directory)?;
        items.sort_by(|a, b| a.path.cmp(&b.path));
        for item in items {
            visited += 1;
            if visited > 20_000 {
                return Err("This watched folder has more than 20,000 items. Choose a smaller source folder.".into());
            }
            digest.update(item.path.as_bytes());
            digest.update([0, item.is_dir as u8, item.is_symlink as u8]);
            if !item.is_dir {
                digest.update(item.size.to_le_bytes());
                digest.update(item.modified.unwrap_or(0).to_le_bytes());
            } else if !item.is_symlink {
                stack.push(item.path);
            }
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
#[cfg_attr(all(test, windows), allow(dead_code))]
pub(crate) fn plan_sync(
    state: &AppState,
    left_profile: &Option<String>,
    left_path: &str,
    right_profile: &Option<String>,
    right_path: &str,
    mode: &str,
) -> Result<SyncPlan, String> {
    plan_sync_excluding(
        state,
        left_profile,
        left_path,
        right_profile,
        right_path,
        mode,
        "",
    )
}

pub(crate) fn plan_sync_excluding(
    state: &AppState,
    left_profile: &Option<String>,
    left_path: &str,
    right_profile: &Option<String>,
    right_path: &str,
    mode: &str,
    exclude: &str,
) -> Result<SyncPlan, String> {
    if ![
        "left-to-right",
        "right-to-left",
        "two-way",
        "mirror-left-to-right",
        "mirror-right-to-left",
    ]
    .contains(&mode)
    {
        return Err("Choose a direction for folder sync.".into());
    }
    if left_profile == right_profile {
        let (left_root, right_root) = if left_profile.is_none() {
            (
                dunce::canonicalize(left_path)
                    .map_err(|error| format!("Could not open the left folder: {error}"))?,
                dunce::canonicalize(right_path)
                    .map_err(|error| format!("Could not open the right folder: {error}"))?,
            )
        } else {
            for path in [left_path, right_path] {
                let path = Path::new(path);
                if !path.is_absolute()
                    || path
                        .components()
                        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
                {
                    return Err("Choose absolute remote folders without parent or current-folder aliases for sync.".into());
                }
            }
            (
                Path::new(left_path).to_path_buf(),
                Path::new(right_path).to_path_buf(),
            )
        };
        if left_root.starts_with(&right_root) || right_root.starts_with(&left_root) {
            return Err("Choose separate folders that are not inside each other for sync.".into());
        }
    }
    let mut left = open_backend(state, left_profile)?;
    let mut right = open_backend(state, right_profile)?;
    for (backend, path, label) in [
        (&mut left, left_path, "left"),
        (&mut right, right_path, "right"),
    ] {
        let item = backend.stat(path)?;
        if !item.is_dir || item.is_symlink {
            return Err(format!(
                "Choose a regular {label} folder, not a file or symbolic link."
            ));
        }
    }
    let mut plan = SyncPlan::default();
    let mut visited = 0;
    if mode != "right-to-left" && mode != "mirror-right-to-left" {
        let mut context = SyncContext {
            source_profile: left_profile,
            destination_profile: right_profile,
            plan: &mut plan,
            visited: &mut visited,
            mirror: mode == "mirror-left-to-right",
            exclude,
        };
        plan_one_way(&mut left, &mut right, left_path, right_path, &mut context)?;
    }
    if mode != "left-to-right" && mode != "mirror-left-to-right" {
        let mut context = SyncContext {
            source_profile: right_profile,
            destination_profile: left_profile,
            plan: &mut plan,
            visited: &mut visited,
            mirror: mode == "mirror-right-to-left",
            exclude,
        };
        plan_one_way(&mut right, &mut left, right_path, left_path, &mut context)?;
    }
    plan.issues.sort();
    plan.issues.dedup();
    plan.deletes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(plan)
}

#[tauri::command]
pub async fn sync_plan(
    app: AppHandle,
    left_profile: Option<String>,
    left_path: String,
    right_profile: Option<String>,
    right_path: String,
    mode: String,
    exclude: Option<String>,
) -> Result<SyncPlan, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        plan_sync_excluding(
            &state,
            &left_profile,
            &left_path,
            &right_profile,
            &right_path,
            &mode,
            exclude.as_deref().unwrap_or(""),
        )
    })
    .await
    .map_err(|error| format!("Folder comparison stopped unexpectedly: {error}"))?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn sync_mirror_delete(
    app: AppHandle,
    left_profile: Option<String>,
    left_path: String,
    right_profile: Option<String>,
    right_path: String,
    mode: String,
    expected_paths: Vec<String>,
    exclude: Option<String>,
) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        mirror_delete(
            &app,
            &left_profile,
            &left_path,
            &right_profile,
            &right_path,
            &mode,
            expected_paths,
            exclude.as_deref().unwrap_or(""),
        )
    })
    .await
    .map_err(|error| format!("Mirror cleanup stopped unexpectedly: {error}"))?
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn mirror_delete<R: Runtime>(
    app: &AppHandle<R>,
    left_profile: &Option<String>,
    left_path: &str,
    right_profile: &Option<String>,
    right_path: &str,
    mode: &str,
    mut expected_paths: Vec<String>,
    exclude: &str,
) -> Result<usize, String> {
    if !["mirror-left-to-right", "mirror-right-to-left"].contains(&mode) {
        return Err("Choose a one-way mirror direction before deleting destination extras.".into());
    }
    let state = app.state::<AppState>();
    let plan = plan_sync_excluding(
        &state,
        left_profile,
        left_path,
        right_profile,
        right_path,
        mode,
        exclude,
    )?;
    if !plan.copies.is_empty() || !plan.issues.is_empty() {
        return Err("The folders changed or still need copies or manual review. Finish transfers, then compare again before deleting extras.".into());
    }
    expected_paths.sort();
    let actual: Vec<_> = plan.deletes.iter().map(|item| item.path.clone()).collect();
    if expected_paths != actual || actual.is_empty() {
        return Err(
            "The deletion preview changed. Compare the folders again before deleting extras."
                .into(),
        );
    }
    let destination_profile = if mode == "mirror-left-to-right" {
        right_profile
    } else {
        left_profile
    };
    let mut destination = open_backend(&state, destination_profile)?;
    for (index, item) in plan.deletes.iter().enumerate() {
        let current = destination.stat(&item.path)?;
        let mut visited = 0;
        if current.is_symlink
            || (current.is_dir && scan_links(&mut destination, &item.path, &mut visited)?)
        {
            return Err(format!("Deleted {index} of {} extras. {} now contains a symbolic link; compare again before continuing.", plan.deletes.len(), item.path));
        }
        destination.remove_tree(&item.path).map_err(|error| {
            format!(
                "Deleted {index} of {} extras. Could not delete {}: {error}",
                plan.deletes.len(),
                item.path
            )
        })?;
    }
    Ok(plan.deletes.len())
}

pub fn migrate_queue_order(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    let has_queue_order = {
        let mut statement = connection.prepare("PRAGMA table_info(transfers)")?;
        let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
        columns
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "queue_order")
    };
    if !has_queue_order {
        connection.execute(
            "ALTER TABLE transfers ADD COLUMN queue_order INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    connection.execute(
        "UPDATE transfers SET queue_order=rowid WHERE queue_order=0",
        [],
    )?;
    let has_method = connection
        .prepare("PRAGMA table_info(transfers)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == "method");
    if !has_method {
        connection.execute(
            "ALTER TABLE transfers ADD COLUMN method TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    Ok(())
}

fn read_settings(state: &AppState) -> Result<TransferSettings, String> {
    let db = state.db.lock().map_err(|_| "Transfer database is busy")?;
    let read = |key: &str| -> Result<Option<String>, String> {
        db.query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|error| format!("Could not read transfer settings: {error}"))
    };
    Ok(TransferSettings {
        parallelism: read("transfer.parallelism")?
            .and_then(|value| value.parse().ok())
            .unwrap_or(2)
            .clamp(1, 8),
        speed_limit_kbps: read("transfer.speed_limit_kbps")?
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
    })
}

#[tauri::command]
pub fn transfer_settings(state: State<'_, AppState>) -> Result<TransferSettings, String> {
    read_settings(&state)
}

#[tauri::command]
pub fn transfer_settings_save(
    app: AppHandle,
    state: State<'_, AppState>,
    parallelism: usize,
    speed_limit_kbps: u64,
) -> Result<TransferSettings, String> {
    if !(1..=8).contains(&parallelism) {
        return Err("Parallel transfers must be between 1 and 8.".into());
    }
    if speed_limit_kbps > 1_000_000 {
        return Err("Speed limit cannot exceed 1,000,000 KB/s.".into());
    }
    {
        let db = state.db.lock().map_err(|_| "Transfer database is busy")?;
        for (key, value) in [
            ("transfer.parallelism", parallelism.to_string()),
            ("transfer.speed_limit_kbps", speed_limit_kbps.to_string()),
        ] {
            db.execute("INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value])
                .map_err(|error| format!("Could not save transfer settings: {error}"))?;
        }
    }
    pump_queue(&app)?;
    read_settings(&state)
}

#[derive(Clone)]
struct Item {
    path: String,
    name: String,
    is_dir: bool,
    is_symlink: bool,
    size: u64,
    modified: Option<u64>,
}

/// A single path component a listing may legitimately contain.
pub(crate) fn safe_entry_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

enum Backend {
    Local,
    Remote(SftpClient),
    Ftp(FtpClient),
    WebDav(WebDavClient),
    S3(S3Client),
    Cloud(crate::cloud::CloudClient),
    Azure(crate::azure::AzureClient),
}

enum FileHandle {
    Local(fs::File),
    Remote(Vec<u8>),
    Ftp(Box<dyn FtpData>, u64),
    WebDavRead(reqwest::blocking::Response, u64),
    WebDavWrite(WebDavUpload),
    S3Read(reqwest::blocking::Response, u64),
    S3Write(S3Upload),
    CloudRead(reqwest::blocking::Response, u64),
    CloudWrite(crate::cloud::CloudUpload),
    AzureRead(reqwest::blocking::Response, u64),
    AzureWrite(crate::azure::AzureUpload),
}

fn local_file_options(read: bool, write: bool, truncate: bool) -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options
        .read(read)
        .write(write)
        .create(write)
        .truncate(truncate);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options
}

struct RateLimit {
    bytes_per_second: u64,
    transferred: u64,
    started: Instant,
}

impl RateLimit {
    fn new(kbps: u64) -> Self {
        Self {
            bytes_per_second: kbps.saturating_mul(1024),
            transferred: 0,
            started: Instant::now(),
        }
    }

    fn chunk_size(&self) -> u64 {
        if self.bytes_per_second == 0 {
            64 * 1024
        } else {
            (self.bytes_per_second / 4).clamp(1024, 64 * 1024)
        }
    }

    fn pace(&mut self, bytes: u64, flag: &AtomicBool) -> Result<(), String> {
        if self.bytes_per_second == 0 {
            return Ok(());
        }
        self.transferred = self.transferred.saturating_add(bytes);
        let expected =
            Duration::from_secs_f64(self.transferred as f64 / self.bytes_per_second as f64);
        while self.started.elapsed() < expected {
            if !flag.load(Ordering::Relaxed) {
                return Err("Transfer paused.".into());
            }
            std::thread::sleep(
                expected
                    .saturating_sub(self.started.elapsed())
                    .min(Duration::from_millis(100)),
            );
        }
        Ok(())
    }
}

impl Backend {
    fn path(&self, parent: &str, name: &str) -> String {
        match self {
            Self::Local => Path::new(parent).join(name).to_string_lossy().to_string(),
            Self::Remote(_)
            | Self::Ftp(_)
            | Self::WebDav(_)
            | Self::S3(_)
            | Self::Cloud(_)
            | Self::Azure(_) => join_remote(parent, name),
        }
    }

    fn stat(&mut self, path: &str) -> Result<Item, String> {
        let name = match self {
            // Local paths use the system's separator (a backslash on Windows).
            Self::Local => Path::new(path).file_name().map_or_else(
                || path.to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            _ => path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(path)
                .to_string(),
        };
        match self {
            Self::Local => {
                let meta = fs::symlink_metadata(path)
                    .map_err(|error| format!("Cannot inspect {path}: {error}"))?;
                Ok(Item {
                    path: path.into(),
                    name,
                    is_dir: meta.is_dir(),
                    is_symlink: meta.file_type().is_symlink(),
                    size: meta.len(),
                    modified: meta
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                        .map(|duration| duration.as_secs()),
                })
            }
            Self::Remote(client) => {
                let attrs = client.stat(path)?;
                let mode = attrs.permissions.unwrap_or(0);
                Ok(Item {
                    path: path.into(),
                    name,
                    is_dir: mode & 0o170000 == 0o040000,
                    is_symlink: mode & 0o170000 == 0o120000,
                    size: attrs.size,
                    modified: attrs.modified,
                })
            }
            Self::Ftp(client) => {
                let entry = client.stat(path)?;
                Ok(Item {
                    path: entry.path,
                    name: entry.name,
                    is_dir: entry.is_dir,
                    is_symlink: entry.is_symlink,
                    size: entry.size,
                    modified: entry.modified,
                })
            }
            Self::WebDav(client) => {
                let entry = client.stat(path)?;
                Ok(Item {
                    path: entry.path,
                    name: entry.name,
                    is_dir: entry.is_dir,
                    is_symlink: false,
                    size: entry.size,
                    modified: entry.modified,
                })
            }
            Self::S3(client) => {
                let entry = client.stat(path)?;
                Ok(Item {
                    path: entry.path,
                    name: entry.name,
                    is_dir: entry.is_dir,
                    is_symlink: false,
                    size: entry.size,
                    modified: entry.modified,
                })
            }
            Self::Cloud(client) => {
                let entry = client.stat(path)?;
                Ok(Item {
                    path: entry.path,
                    name: entry.name,
                    is_dir: entry.is_dir,
                    is_symlink: false,
                    size: entry.size,
                    modified: entry.modified,
                })
            }
            Self::Azure(client) => {
                let entry = client.stat(path)?;
                Ok(Item {
                    path: entry.path,
                    name: entry.name,
                    is_dir: entry.is_dir,
                    is_symlink: false,
                    size: entry.size,
                    modified: entry.modified,
                })
            }
        }
    }

    /// Lists a folder. Names from a server are checked, so a hostile listing
    /// entry such as `..` cannot steer a copy outside its destination.
    fn list(&mut self, path: &str) -> Result<Vec<Item>, String> {
        let items = self.list_unchecked(path)?;
        if !matches!(self, Self::Local) {
            if let Some(item) = items.iter().find(|item| !safe_entry_name(&item.name)) {
                return Err(format!(
                    "The server listed an unsafe name in {path} ({:?}). Nothing was copied from that folder.",
                    item.name
                ));
            }
        }
        Ok(items)
    }

    fn list_unchecked(&mut self, path: &str) -> Result<Vec<Item>, String> {
        match self {
            Self::Local => fs::read_dir(path)
                .map_err(|error| format!("Cannot list {path}: {error}"))?
                .map(|entry| {
                    let entry =
                        entry.map_err(|error| format!("Cannot read folder entry: {error}"))?;
                    self.stat(&entry.path().to_string_lossy())
                })
                .collect(),
            Self::Remote(client) => Ok(client
                .list(path)?
                .entries
                .into_iter()
                .map(|entry| Item {
                    path: entry.path,
                    name: entry.name,
                    is_dir: entry.is_dir,
                    is_symlink: entry.is_symlink,
                    size: entry.size,
                    modified: entry.modified,
                })
                .collect()),
            Self::Ftp(client) => Ok(client
                .list(path)?
                .entries
                .into_iter()
                .map(|entry| Item {
                    path: entry.path,
                    name: entry.name,
                    is_dir: entry.is_dir,
                    is_symlink: entry.is_symlink,
                    size: entry.size,
                    modified: entry.modified,
                })
                .collect()),
            Self::WebDav(client) => Ok(client
                .list(path)?
                .entries
                .into_iter()
                .map(|entry| Item {
                    path: entry.path,
                    name: entry.name,
                    is_dir: entry.is_dir,
                    is_symlink: false,
                    size: entry.size,
                    modified: entry.modified,
                })
                .collect()),
            Self::S3(client) => Ok(client
                .list(path)?
                .entries
                .into_iter()
                .map(|entry| Item {
                    path: entry.path,
                    name: entry.name,
                    is_dir: entry.is_dir,
                    is_symlink: false,
                    size: entry.size,
                    modified: entry.modified,
                })
                .collect()),
            Self::Cloud(client) => Ok(client
                .list(path)?
                .entries
                .into_iter()
                .map(|entry| Item {
                    path: entry.path,
                    name: entry.name,
                    is_dir: entry.is_dir,
                    is_symlink: false,
                    size: entry.size,
                    modified: entry.modified,
                })
                .collect()),
            Self::Azure(client) => Ok(client
                .list(path)?
                .entries
                .into_iter()
                .map(|entry| Item {
                    path: entry.path,
                    name: entry.name,
                    is_dir: entry.is_dir,
                    is_symlink: false,
                    size: entry.size,
                    modified: entry.modified,
                })
                .collect()),
        }
    }

    fn exists(&mut self, path: &str) -> Result<bool, String> {
        match self {
            Self::Local => match fs::symlink_metadata(path) {
                Ok(_) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(format!("Cannot inspect {path}: {error}")),
            },
            Self::Remote(_)
            | Self::Ftp(_)
            | Self::WebDav(_)
            | Self::S3(_)
            | Self::Cloud(_)
            | Self::Azure(_) => {
                let (parent, name) = path.rsplit_once('/').ok_or("Invalid remote path.")?;
                let parent = if parent.is_empty() { "/" } else { parent };
                Ok(self.list(parent)?.iter().any(|item| item.name == name))
            }
        }
    }

    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        match self {
            Self::Local => {
                fs::create_dir(path).map_err(|error| format!("Cannot create {path}: {error}"))
            }
            Self::Remote(client) => client.mkdir(path),
            Self::Ftp(client) => client.mkdir(path),
            Self::WebDav(client) => client.mkdir(path),
            Self::S3(client) => client.mkdir(path),
            Self::Cloud(client) => client.mkdir(path),
            Self::Azure(client) => client.mkdir(path),
        }
    }

    fn set_modified(&mut self, path: &str, seconds: u64) -> Result<(), String> {
        match self {
            Self::Local => {
                let file = fs::OpenOptions::new()
                    .write(true)
                    .open(path)
                    .map_err(|error| {
                        format!("Cannot reopen {path} to preserve its modification time: {error}")
                    })?;
                file.set_times(
                    fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)),
                )
                .map_err(|error| {
                    format!("Cannot preserve the modification time of {path}: {error}")
                })
            }
            Self::Remote(client) => client.set_mtime(path, seconds),
            Self::Ftp(client) => client.set_mtime(path, seconds),
            Self::WebDav(_) | Self::S3(_) | Self::Cloud(_) | Self::Azure(_) => Ok(()),
        }
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        match self {
            Self::Local => {
                fs::rename(from, to).map_err(|error| format!("Cannot rename {from}: {error}"))
            }
            Self::Remote(client) => client.rename(from, to),
            Self::Ftp(client) => client.rename(from, to),
            Self::WebDav(client) => client.rename(from, to),
            Self::S3(client) => client.rename(from, to),
            Self::Cloud(client) => client.rename(from, to),
            Self::Azure(client) => client.rename(from, to),
        }
    }

    fn remove_tree(&mut self, path: &str) -> Result<(), String> {
        let item = self.stat(path)?;
        if item.is_dir {
            match self {
                Self::Local => fs::remove_dir_all(path)
                    .map_err(|error| format!("Cannot remove {path}: {error}")),
                Self::WebDav(client) => client.delete(path, true),
                Self::S3(client) => client.delete(path, true),
                Self::Cloud(client) => client.delete(path, true),
                Self::Azure(client) => client.delete(path, true),
                Self::Remote(_) | Self::Ftp(_) => {
                    for child in self.list(path)? {
                        self.remove_tree(&child.path)?;
                    }
                    if let Self::Remote(client) = self {
                        client.remove(path, true)
                    } else if let Self::Ftp(client) = self {
                        client.remove(path, true)
                    } else {
                        unreachable!()
                    }
                }
            }
        } else {
            match self {
                Self::Local => {
                    fs::remove_file(path).map_err(|error| format!("Cannot remove {path}: {error}"))
                }
                Self::Remote(client) => client.remove(path, false),
                Self::Ftp(client) => client.remove(path, false),
                Self::WebDav(client) => client.delete(path, false),
                Self::S3(client) => client.delete(path, false),
                Self::Cloud(client) => client.delete(path, false),
                Self::Azure(client) => client.delete(path, false),
            }
        }
    }

    fn open_read(&mut self, path: &str, offset: u64) -> Result<FileHandle, String> {
        match self {
            Self::Local => local_file_options(true, false, false)
                .open(path)
                .map(FileHandle::Local)
                .map_err(|error| format!("Cannot open {path}: {error}")),
            Self::Remote(client) => client.open_read(path).map(FileHandle::Remote),
            Self::Ftp(client) => client
                .open_read(path, offset)
                .map(|stream| FileHandle::Ftp(stream, offset)),
            Self::WebDav(client) => client
                .open_read(path, offset)
                .map(|response| FileHandle::WebDavRead(response, offset)),
            Self::S3(client) => client
                .open_read(path, offset)
                .map(|response| FileHandle::S3Read(response, offset)),
            Self::Cloud(client) => client
                .open_read(path, offset)
                .map(|response| FileHandle::CloudRead(response, offset)),
            Self::Azure(client) => client
                .open_read(path, offset)
                .map(|response| FileHandle::AzureRead(response, offset)),
        }
    }

    fn open_write(
        &mut self,
        path: &str,
        truncate: bool,
        offset: u64,
    ) -> Result<FileHandle, String> {
        match self {
            Self::Local => local_file_options(false, true, truncate)
                .open(path)
                .map(FileHandle::Local)
                .map_err(|error| format!("Cannot open {path} for writing: {error}")),
            Self::Remote(client) => client.open_write(path, truncate).map(FileHandle::Remote),
            Self::Ftp(client) => client
                .open_write(path, !truncate)
                .map(|stream| FileHandle::Ftp(stream, offset)),
            Self::WebDav(client) => client
                .open_write(path, truncate, offset)
                .map(FileHandle::WebDavWrite),
            Self::S3(client) => client
                .open_write(path, truncate, offset)
                .map(FileHandle::S3Write),
            Self::Cloud(client) => client
                .open_write(path, truncate, offset)
                .map(FileHandle::CloudWrite),
            Self::Azure(client) => client
                .open_write(path, truncate, offset)
                .map(FileHandle::AzureWrite),
        }
    }

    fn read_chunk(
        &mut self,
        handle: &mut FileHandle,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, String> {
        match (self, handle) {
            (Self::Local, FileHandle::Local(file)) => {
                file.seek(SeekFrom::Start(offset))
                    .map_err(|error| format!("Cannot seek source file: {error}"))?;
                let mut data = vec![0; length];
                let count = file
                    .read(&mut data)
                    .map_err(|error| format!("Cannot read source file: {error}"))?;
                data.truncate(count);
                Ok(data)
            }
            (Self::Remote(client), FileHandle::Remote(handle)) => Ok(client
                .read(handle, offset, length as u32)?
                .unwrap_or_default()),
            (Self::Ftp(_), FileHandle::Ftp(stream, position)) => {
                if *position != offset {
                    return Err(
                        "FTP download offset changed unexpectedly. Retry the transfer.".into(),
                    );
                }
                let mut data = vec![0; length];
                let count = stream
                    .read(&mut data)
                    .map_err(|error| format!("Could not read FTP data: {error}"))?;
                data.truncate(count);
                *position += count as u64;
                Ok(data)
            }
            (Self::WebDav(_), FileHandle::WebDavRead(response, position)) => {
                if *position != offset {
                    return Err(
                        "WebDAV download offset changed unexpectedly. Retry the transfer.".into(),
                    );
                }
                let mut data = vec![0; length];
                let count = response
                    .read(&mut data)
                    .map_err(|error| format!("Could not read WebDAV download: {error}"))?;
                data.truncate(count);
                *position += count as u64;
                Ok(data)
            }
            (Self::S3(_), FileHandle::S3Read(response, position)) => {
                if *position != offset {
                    return Err(
                        "S3 download offset changed unexpectedly. Retry the transfer.".into(),
                    );
                }
                let mut data = vec![0; length];
                let count = response
                    .read(&mut data)
                    .map_err(|error| format!("Could not read S3 download: {error}"))?;
                data.truncate(count);
                *position += count as u64;
                Ok(data)
            }
            (Self::Cloud(_), FileHandle::CloudRead(response, position))
            | (Self::Azure(_), FileHandle::AzureRead(response, position)) => {
                if *position != offset {
                    return Err(
                        "The download position changed unexpectedly. Retry the transfer.".into(),
                    );
                }
                let mut data = vec![0; length];
                let count = response
                    .read(&mut data)
                    .map_err(|error| format!("Could not read the download: {error}"))?;
                data.truncate(count);
                *position += count as u64;
                Ok(data)
            }
            _ => Err("Transfer endpoint changed while reading.".into()),
        }
    }

    fn write_chunk(
        &mut self,
        handle: &mut FileHandle,
        offset: u64,
        data: &[u8],
    ) -> Result<(), String> {
        match (self, handle) {
            (Self::Local, FileHandle::Local(file)) => {
                file.seek(SeekFrom::Start(offset))
                    .map_err(|error| format!("Cannot seek destination file: {error}"))?;
                file.write_all(data)
                    .map_err(|error| format!("Cannot write destination file: {error}"))
            }
            (Self::Remote(client), FileHandle::Remote(handle)) => {
                client.write(handle, offset, data)
            }
            (Self::Ftp(_), FileHandle::Ftp(stream, position)) => {
                if *position != offset {
                    return Err(
                        "FTP upload offset changed unexpectedly. Retry the transfer.".into(),
                    );
                }
                stream
                    .write_all(data)
                    .map_err(|error| format!("Could not write FTP data: {error}"))?;
                *position += data.len() as u64;
                Ok(())
            }
            (Self::WebDav(_), FileHandle::WebDavWrite(upload)) => {
                WebDavClient::write_chunk(upload, offset, data)
            }
            (Self::S3(_), FileHandle::S3Write(upload)) => {
                S3Client::write_chunk(upload, offset, data)
            }
            (Self::Cloud(_), FileHandle::CloudWrite(upload)) => {
                crate::cloud::CloudClient::write_chunk(upload, offset, data)
            }
            (Self::Azure(_), FileHandle::AzureWrite(upload)) => {
                crate::azure::AzureClient::write_chunk(upload, offset, data)
            }
            _ => Err("Transfer endpoint changed while writing.".into()),
        }
    }

    fn close(&mut self, handle: FileHandle, written: bool) -> Result<(), String> {
        match (self, handle) {
            (Self::Local, FileHandle::Local(file)) if written => file
                .sync_all()
                .map_err(|error| format!("Could not save destination file: {error}")),
            (Self::Local, FileHandle::Local(_)) => Ok(()),
            (Self::Remote(client), FileHandle::Remote(handle)) => client.close(&handle),
            (Self::Ftp(_), FileHandle::Ftp(stream, _)) => stream.finish(),
            (Self::WebDav(_), FileHandle::WebDavRead(_, _)) => Ok(()),
            (Self::WebDav(client), FileHandle::WebDavWrite(upload)) if written => {
                client.finish_write(upload)
            }
            (Self::S3(_), FileHandle::S3Read(_, _)) => Ok(()),
            (Self::S3(client), FileHandle::S3Write(upload)) if written => {
                client.finish_write(upload)
            }
            (Self::Cloud(_), FileHandle::CloudRead(_, _)) => Ok(()),
            (Self::Cloud(client), FileHandle::CloudWrite(upload)) if written => {
                client.finish_write(upload)
            }
            (Self::Azure(_), FileHandle::AzureRead(_, _)) => Ok(()),
            (Self::Azure(client), FileHandle::AzureWrite(upload)) if written => {
                client.finish_write(upload)
            }
            _ => Err("Transfer endpoint changed while closing.".into()),
        }
    }
}

fn open_backend(state: &AppState, profile_id: &Option<String>) -> Result<Backend, String> {
    match profile_id {
        None => Ok(Backend::Local),
        Some(id) => {
            let profile = profiles::get_profile(state, id)?;
            if crate::cloud::is_cloud(&profile.protocol) {
                Ok(Backend::Cloud(crate::cloud::CloudClient::connect(
                    &profile,
                )?))
            } else if profile.protocol == "azure" {
                let password = vault::password_for(state, &profile)?;
                Ok(Backend::Azure(crate::azure::AzureClient::connect(
                    &profile, &password,
                )?))
            } else if profile.protocol == "ssh" {
                Ok(Backend::Remote(SftpClient::connect(&profile)?))
            } else if profile.protocol == "webdav" {
                let password = vault::password_for(state, &profile)?;
                Ok(Backend::WebDav(WebDavClient::connect(&profile, &password)?))
            } else if profile.protocol == "s3" {
                let password = vault::password_for(state, &profile)?;
                let token = vault::s3_token_for(state, id)?;
                Ok(Backend::S3(S3Client::connect_with_token(
                    &profile,
                    &password,
                    token.as_ref().map(|value| value.as_str()),
                )?))
            } else {
                let password = vault::password_for(state, &profile)?;
                Ok(Backend::Ftp(FtpClient::connect(&profile, &password)?))
            }
        }
    }
}

fn read_transfer(state: &AppState, id: &str) -> Result<TransferRecord, String> {
    let db = state.db.lock().map_err(|_| "Transfer database is busy")?;
    db.query_row("SELECT id, source_profile, source_path, destination_profile, destination_dir, target_path, conflict, status, bytes_done, total_bytes, error, created_at, queue_order, method FROM transfers WHERE id=?1", [id], |row| Ok(TransferRecord {
        id: row.get(0)?, source_profile: row.get(1)?, source_path: row.get(2)?, destination_profile: row.get(3)?,
        destination_dir: row.get(4)?, target_path: row.get(5)?, conflict: row.get(6)?, status: row.get(7)?,
        bytes_done: row.get::<_, i64>(8)? as u64, total_bytes: row.get::<_, i64>(9)? as u64, error: row.get(10)?, created_at: row.get(11)?, queue_order: row.get(12)?, method: row.get(13)?,
    })).map_err(|error| format!("Could not load transfer: {error}"))
}

fn update_transfer<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    status: &str,
    done: u64,
    total: u64,
    target: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    {
        let db = state.db.lock().map_err(|_| "Transfer database is busy")?;
        db.execute("UPDATE transfers SET status=?2, bytes_done=?3, total_bytes=?4, target_path=COALESCE(?5,target_path), error=?6, updated_at=unixepoch() WHERE id=?1",
            params![id, status, done.min(i64::MAX as u64) as i64, total.min(i64::MAX as u64) as i64, target, error]).map_err(|cause| format!("Could not save transfer progress: {cause}"))?;
    }
    let record = read_transfer(&state, id)?;
    let _ = app.emit("transfer-progress", record);
    Ok(())
}

fn resolve_batch_profile(state: &AppState, value: &mut Option<String>) -> Result<(), String> {
    let Some(name_or_id) = value else {
        return Ok(());
    };
    let db = state.db.lock().map_err(|_| "Profile database is busy")?;
    let exact: Option<String> = db
        .query_row(
            "SELECT id FROM profiles WHERE id=?1",
            [&*name_or_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("Could not find the saved connection: {error}"))?;
    if let Some(id) = exact {
        *name_or_id = id;
        return Ok(());
    }
    let mut statement = db
        .prepare("SELECT id FROM profiles WHERE name=?1 COLLATE NOCASE LIMIT 2")
        .map_err(|error| format!("Could not search saved connections: {error}"))?;
    let matches = statement
        .query_map([&*name_or_id], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Could not search saved connections: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Could not read saved connections: {error}"))?;
    match matches.as_slice() {
        [id] => {
            *name_or_id = id.clone();
            Ok(())
        }
        [] => Err(format!("No saved connection is named or identified as {name_or_id}.")),
        _ => Err(format!("More than one saved connection is named {name_or_id}. Use its unique connection ID instead.")),
    }
}

fn read_batch_manifest(state: &AppState, path: &str) -> Result<BatchPreview, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect the copy plan: {error}"))?;
    if !metadata.is_file() {
        return Err("Choose a regular JSON copy plan file, not a folder or link.".into());
    }
    if metadata.len() > 1024 * 1024 {
        return Err("The copy plan is larger than 1 MB. Split it into smaller plans.".into());
    }
    let bytes = fs::read(path).map_err(|error| format!("Could not read the copy plan: {error}"))?;
    if bytes.len() > 1024 * 1024 {
        return Err("The copy plan grew beyond 1 MB while opening. Try again.".into());
    }
    let mut manifest: BatchManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Could not read the copy plan JSON: {error}"))?;
    if manifest.version != 1 {
        return Err("This copy plan version is unsupported. Set version to 1.".into());
    }
    if manifest.transfers.is_empty() || manifest.transfers.len() > 200 {
        return Err("A copy plan must contain between 1 and 200 transfers.".into());
    }
    for (index, item) in manifest.transfers.iter_mut().enumerate() {
        let row = index + 1;
        if !["overwrite", "skip", "rename", "newer"].contains(&item.conflict.as_str()) {
            return Err(format!(
                "Copy plan row {row}: choose overwrite, skip, rename, or newer for conflict."
            ));
        }
        for (label, path) in [
            ("sourcePath", &item.source_path),
            ("destinationDir", &item.destination_dir),
        ] {
            if path.is_empty() || path.contains('\0') || !Path::new(path).is_absolute() {
                return Err(format!("Copy plan row {row}: {label} must be an absolute path without null characters."));
            }
        }
        resolve_batch_profile(state, &mut item.source_profile)
            .map_err(|error| format!("Copy plan row {row} sourceProfile: {error}"))?;
        resolve_batch_profile(state, &mut item.destination_profile)
            .map_err(|error| format!("Copy plan row {row} destinationProfile: {error}"))?;
        if item.source_profile.is_none() {
            let source = fs::symlink_metadata(&item.source_path).map_err(|error| {
                format!(
                    "Copy plan row {row}: cannot inspect the local source {}: {error}",
                    item.source_path
                )
            })?;
            if source.file_type().is_symlink() || (!source.is_file() && !source.is_dir()) {
                return Err(format!("Copy plan row {row}: the local source must be a regular file or folder, not a link or special file."));
            }
        }
        if item.destination_profile.is_none() {
            let destination = fs::symlink_metadata(&item.destination_dir).map_err(|error| {
                format!(
                    "Copy plan row {row}: cannot inspect the local destination {}: {error}",
                    item.destination_dir
                )
            })?;
            if !destination.is_dir() {
                return Err(format!("Copy plan row {row}: the local destination must be an existing folder, not a file or link."));
            }
        }
        if item.source_profile.is_none() && item.destination_profile.is_none() {
            let source = dunce::canonicalize(&item.source_path).map_err(|error| {
                format!("Copy plan row {row}: cannot resolve the local source: {error}")
            })?;
            let destination = dunce::canonicalize(&item.destination_dir).map_err(|error| {
                format!("Copy plan row {row}: cannot resolve the local destination: {error}")
            })?;
            if source.is_dir() && destination.starts_with(&source) {
                return Err(format!("Copy plan row {row}: a folder cannot be copied into itself or one of its subfolders."));
            }
        }
        if item.source_profile == item.destination_profile
            && item.source_path == item.destination_dir
        {
            return Err(format!(
                "Copy plan row {row}: source and destination cannot be the same folder."
            ));
        }
    }
    Ok(BatchPreview {
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        transfers: manifest.transfers,
    })
}

#[tauri::command]
pub fn transfer_batch_preview(
    state: State<'_, AppState>,
    path: String,
) -> Result<BatchPreview, String> {
    read_batch_manifest(&state, &path)
}

fn queue_batch<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    path: &str,
    expected_sha256: &str,
) -> Result<BatchQueueResult, String> {
    let preview = read_batch_manifest(state, path)?;
    if preview.sha256 != expected_sha256 {
        return Err(
            "The copy plan changed after preview. Open it again before adding transfers.".into(),
        );
    }
    let ids = preview
        .transfers
        .iter()
        .map(|_| uuid::Uuid::new_v4().to_string())
        .collect::<Vec<_>>();
    {
        let mut db = state.db.lock().map_err(|_| "Transfer database is busy")?;
        let transaction = db
            .transaction()
            .map_err(|error| format!("Could not start the copy plan: {error}"))?;
        for (id, item) in ids.iter().zip(&preview.transfers) {
            transaction.execute(
                "INSERT INTO transfers (id, source_profile, source_path, destination_profile, destination_dir, conflict, status, queue_order) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', (SELECT COALESCE(MAX(queue_order), 0) + 1 FROM transfers))",
                params![id, item.source_profile, item.source_path, item.destination_profile, item.destination_dir, item.conflict],
            ).map_err(|error| format!("Could not add the copy plan to the queue: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("Could not save the copy plan queue: {error}"))?;
    }
    let records = ids
        .iter()
        .map(|id| read_transfer(state, id))
        .collect::<Result<Vec<_>, _>>()?;
    let warning = pump_queue(app).err().map(|error| {
        format!("The transfers were added to the queue, but could not start yet: {error}")
    });
    Ok(BatchQueueResult { records, warning })
}

#[tauri::command]
pub fn transfer_batch_enqueue(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    expected_sha256: String,
) -> Result<BatchQueueResult, String> {
    queue_batch(&app, &state, &path, &expected_sha256)
}

#[tauri::command]
pub fn transfer_enqueue(
    app: AppHandle,
    state: State<'_, AppState>,
    source_profile: Option<String>,
    source_path: String,
    destination_profile: Option<String>,
    destination_dir: String,
    conflict: String,
) -> Result<TransferRecord, String> {
    enqueue_transfer(
        &app,
        &state,
        source_profile,
        source_path,
        destination_profile,
        destination_dir,
        conflict,
    )
}

/// Queues a copy that runs through the system's scp or rsync instead of the
/// built-in engine. One side must be this Mac and the other an SSH host.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn transfer_enqueue_direct(
    app: AppHandle,
    state: State<'_, AppState>,
    method: String,
    source_profile: Option<String>,
    source_path: String,
    destination_profile: Option<String>,
    destination_dir: String,
    conflict: String,
) -> Result<TransferRecord, String> {
    if !["scp", "rsync"].contains(&method.as_str()) {
        return Err("Choose SCP or rsync.".into());
    }
    let hosts = [&source_profile, &destination_profile]
        .into_iter()
        .flatten()
        .map(|id| profiles::get_profile(&state, id))
        .collect::<Result<Vec<_>, _>>()?;
    if hosts.is_empty() || hosts.iter().any(|host| host.protocol != "ssh") {
        return Err(
            "SCP and rsync copy between this computer and an SSH host, or between two SSH hosts."
                .into(),
        );
    }
    if hosts.len() == 2 && method != "scp" {
        return Err(
            "Between two SSH hosts, choose SCP. rsync needs one side on this computer.".into(),
        );
    }
    insert_transfer(
        &app,
        &state,
        source_profile,
        source_path,
        destination_profile,
        destination_dir,
        conflict,
        &method,
    )
}

pub(crate) fn enqueue_transfer<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    source_profile: Option<String>,
    source_path: String,
    destination_profile: Option<String>,
    destination_dir: String,
    conflict: String,
) -> Result<TransferRecord, String> {
    insert_transfer(
        app,
        state,
        source_profile,
        source_path,
        destination_profile,
        destination_dir,
        conflict,
        "",
    )
}

#[allow(clippy::too_many_arguments)]
fn insert_transfer<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    source_profile: Option<String>,
    source_path: String,
    destination_profile: Option<String>,
    destination_dir: String,
    conflict: String,
    method: &str,
) -> Result<TransferRecord, String> {
    if !["overwrite", "skip", "rename", "newer"].contains(&conflict.as_str()) {
        return Err("Choose overwrite, skip, rename, or newer only.".into());
    }
    if source_path.is_empty() || destination_dir.is_empty() {
        return Err("Choose a source and destination folder.".into());
    }
    if let Some(id) = &source_profile {
        profiles::get_profile(state, id)?;
    }
    if let Some(id) = &destination_profile {
        profiles::get_profile(state, id)?;
    }
    let id = uuid::Uuid::new_v4().to_string();
    {
        let db = state.db.lock().map_err(|_| "Transfer database is busy")?;
        db.execute("INSERT INTO transfers (id, source_profile, source_path, destination_profile, destination_dir, conflict, status, queue_order, method) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', (SELECT COALESCE(MAX(queue_order), 0) + 1 FROM transfers), ?7)",
            params![id, source_profile, source_path, destination_profile, destination_dir, conflict, method])
            .map_err(|error| format!("Could not queue transfer: {error}"))?;
    }
    let record = read_transfer(state, &id)?;
    pump_queue(app)?;
    Ok(record)
}

#[tauri::command]
pub fn transfer_list(state: State<'_, AppState>) -> Result<Vec<TransferRecord>, String> {
    let db = state.db.lock().map_err(|_| "Transfer database is busy")?;
    let mut statement = db.prepare("SELECT id, source_profile, source_path, destination_profile, destination_dir, target_path, conflict, status, bytes_done, total_bytes, error, created_at, queue_order, method FROM transfers ORDER BY CASE WHEN status IN ('queued', 'running', 'paused') THEN 0 ELSE 1 END, CASE WHEN status IN ('queued', 'running', 'paused') THEN queue_order ELSE -created_at END, rowid DESC LIMIT 200")
        .map_err(|error| format!("Could not load transfers: {error}"))?;
    let records = statement
        .query_map([], |row| {
            Ok(TransferRecord {
                id: row.get(0)?,
                source_profile: row.get(1)?,
                source_path: row.get(2)?,
                destination_profile: row.get(3)?,
                destination_dir: row.get(4)?,
                target_path: row.get(5)?,
                conflict: row.get(6)?,
                status: row.get(7)?,
                bytes_done: row.get::<_, i64>(8)? as u64,
                total_bytes: row.get::<_, i64>(9)? as u64,
                error: row.get(10)?,
                created_at: row.get(11)?,
                queue_order: row.get(12)?,
                method: row.get(13)?,
            })
        })
        .map_err(|error| format!("Could not load transfers: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Could not read a transfer: {error}"));
    records
}

fn move_queued(db: &mut rusqlite::Connection, id: &str, direction: &str) -> Result<(), String> {
    if direction != "up" && direction != "down" {
        return Err("Choose up or down to reorder a queued transfer.".into());
    }
    let transaction = db
        .transaction()
        .map_err(|error| format!("Could not reorder transfers: {error}"))?;
    let mut queued = {
        let mut statement = transaction
            .prepare("SELECT id FROM transfers WHERE status='queued' ORDER BY queue_order, rowid")
            .map_err(|error| format!("Could not load queued transfers: {error}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("Could not read queued transfers: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Could not read queued transfers: {error}"))?;
        rows
    };
    let index = queued
        .iter()
        .position(|queued_id| queued_id == id)
        .ok_or("This transfer is no longer queued. Refresh the activity list.")?;
    let neighbor = if direction == "up" {
        index.checked_sub(1)
    } else {
        (index + 1 < queued.len()).then_some(index + 1)
    };
    if let Some(neighbor) = neighbor {
        queued.swap(index, neighbor);
        for (position, queued_id) in queued.iter().enumerate() {
            transaction
                .execute(
                    "UPDATE transfers SET queue_order=?2 WHERE id=?1",
                    params![queued_id, position as i64 + 1],
                )
                .map_err(|error| format!("Could not save transfer order: {error}"))?;
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("Could not save transfer order: {error}"))
}

#[tauri::command]
pub fn transfer_move(
    state: State<'_, AppState>,
    id: String,
    direction: String,
) -> Result<(), String> {
    let mut db = state.db.lock().map_err(|_| "Transfer database is busy")?;
    move_queued(&mut db, &id, &direction)
}

#[tauri::command]
pub fn transfer_pause(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    pause_transfer(&app, &state, &id)
}

fn pause_transfer<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    id: &str,
) -> Result<(), String> {
    let record = read_transfer(state, id)?;
    if record.status == "queued" {
        return update_transfer(
            app,
            id,
            "paused",
            record.bytes_done,
            record.total_bytes,
            None,
            None,
        );
    }
    let active = state
        .transfers
        .lock()
        .map_err(|_| "Transfer manager is busy")?;
    let flag = active.get(id).ok_or("This transfer is not running.")?;
    flag.store(false, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub fn transfer_resume(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let record = read_transfer(&state, &id)?;
    if record.status != "paused" && record.status != "failed" {
        return Err("Only paused or failed transfers can be resumed.".into());
    }
    update_transfer(
        &app,
        &id,
        "queued",
        record.bytes_done,
        record.total_bytes,
        None,
        None,
    )?;
    pump_queue(&app)
}

#[tauri::command]
pub fn transfer_remove(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if state
        .transfers
        .lock()
        .map_err(|_| "Transfer manager is busy")?
        .contains_key(&id)
    {
        return Err("Pause the transfer before removing it from history.".into());
    }
    let record = read_transfer(&state, &id)?;
    if record.status == "paused" || record.status == "failed" {
        let mut destination = open_backend(&state, &record.destination_profile)?;
        let partial = destination.path(&record.destination_dir, &format!(".weave-part-{id}"));
        if destination.exists(&partial)? {
            destination.remove_tree(&partial)?;
        }
    }
    let db = state.db.lock().map_err(|_| "Transfer database is busy")?;
    db.execute("DELETE FROM transfers WHERE id=?1", [id])
        .map_err(|error| format!("Could not remove transfer: {error}"))?;
    Ok(())
}

fn spawn_transfer<R: Runtime>(app: AppHandle<R>, id: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let flag = Arc::new(AtomicBool::new(true));
    {
        let mut active = state
            .transfers
            .lock()
            .map_err(|_| "Transfer manager is busy")?;
        if active.contains_key(&id) {
            return Err("Transfer is already running.".into());
        }
        active.insert(id.clone(), flag.clone());
    }
    std::thread::spawn(move || {
        let mut outcome = run_transfer(&app, &id, &flag);
        // Dropped connections are retried; the copy resumes from its partial file.
        for (attempt, wait) in [2_u64, 5, 15].into_iter().enumerate() {
            match &outcome {
                Err(error) if flag.load(Ordering::Relaxed) && retryable(error) => {
                    let state = app.state::<AppState>();
                    if let Ok(record) = read_transfer(&state, &id) {
                        let note = format!(
                            "Connection lost; retrying in {wait} s (attempt {} of 3).",
                            attempt + 1
                        );
                        let _ = update_transfer(
                            &app,
                            &id,
                            "running",
                            record.bytes_done,
                            record.total_bytes,
                            None,
                            Some(&note),
                        );
                    }
                    std::thread::sleep(Duration::from_secs(wait));
                    if !flag.load(Ordering::Relaxed) {
                        break;
                    }
                    outcome = run_transfer(&app, &id, &flag);
                }
                _ => break,
            }
        }
        let state = app.state::<AppState>();
        let previous = read_transfer(&state, &id).ok();
        let done = previous.as_ref().map_or(0, |record| record.bytes_done);
        let total = previous.as_ref().map_or(0, |record| record.total_bytes);
        let status = match &outcome {
            Ok("skipped") => "skipped",
            Ok(_) => "done",
            Err(_) if !flag.load(Ordering::Relaxed) => "paused",
            Err(_) => "failed",
        };
        let error = outcome.err();
        if let Ok(mut active) = state.transfers.lock() {
            active.remove(&id);
        };
        let _ = update_transfer(&app, &id, status, done, total, None, error.as_deref());
        let _ = pump_queue(&app);
    });
    Ok(())
}

/// Network failures that are worth retrying, as opposed to permission or
/// path problems that would fail the same way again.
pub(crate) fn retryable(error: &str) -> bool {
    let lower = error.to_lowercase();
    [
        "timed out",
        "timeout",
        "connection reset",
        "connection refused",
        "connection closed",
        "connection aborted",
        "broken pipe",
        "could not reach",
        "network is unreachable",
        "temporarily",
        "421 ",
        "eof",
        "http 503",
        "http 502",
        "http 504",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub(crate) fn start_pending<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    pump_queue(app)
}

fn pump_queue<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if !cfg!(test) && !crate::startup::owns_queue() {
        return Ok(());
    }
    let state = app.state::<AppState>();
    let _queue = state
        .queue_lock
        .lock()
        .map_err(|_| "Transfer queue is busy")?;
    let active = state
        .transfers
        .lock()
        .map_err(|_| "Transfer manager is busy")?;
    let room = read_settings(&state)?
        .parallelism
        .saturating_sub(active.len());
    let active_ids: std::collections::HashSet<String> = active.keys().cloned().collect();
    drop(active);
    if room == 0 {
        return Ok(());
    }
    let queued = {
        let db = state.db.lock().map_err(|_| "Transfer database is busy")?;
        let mut statement = db
            .prepare("SELECT id FROM transfers WHERE status='queued' ORDER BY queue_order, rowid")
            .map_err(|error| format!("Could not read transfer queue: {error}"))?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("Could not read transfer queue: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Could not read transfer queue: {error}"))?;
        ids
    };
    for id in queued
        .into_iter()
        .filter(|id| !active_ids.contains(id))
        .take(room)
    {
        spawn_transfer(app.clone(), id)?;
    }
    Ok(())
}

fn run_transfer<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    flag: &AtomicBool,
) -> Result<&'static str, String> {
    let state = app.state::<AppState>();
    let record = read_transfer(&state, id)?;
    update_transfer(
        app,
        id,
        "running",
        record.bytes_done,
        record.total_bytes,
        None,
        None,
    )?;
    let mut source = open_backend(&state, &record.source_profile)?;
    let mut destination = open_backend(&state, &record.destination_profile)?;
    let source_item = source.stat(&record.source_path)?;
    if source_item.is_symlink {
        return Err("Copying symbolic links is not supported.".into());
    }
    let mut target = record
        .target_path
        .clone()
        .unwrap_or_else(|| destination.path(&record.destination_dir, &source_item.name));
    if source_item.is_dir
        && record.source_profile == record.destination_profile
        && (record.destination_dir == record.source_path
            || if record.source_profile.is_none() {
                Path::new(&record.destination_dir).starts_with(&record.source_path)
            } else {
                record
                    .destination_dir
                    .starts_with(&(record.source_path.trim_end_matches('/').to_string() + "/"))
            })
    {
        return Err("Cannot copy a folder into itself or a subfolder.".into());
    }
    if record.target_path.is_none() && destination.exists(&target)? {
        match record.conflict.as_str() {
            "skip" => return Ok("skipped"),
            "newer" => {
                if source_item.is_dir {
                    return Err("Newer-only applies to individual files. Choose Keep both or Replace for a folder.".into());
                }
                let current = destination.stat(&target)?;
                if !source_is_newer(source_item.modified, current.modified)? {
                    return Ok("skipped");
                }
            }
            "rename" => {
                let mut number = 2;
                let (stem, extension) = split_name(&source_item.name);
                loop {
                    let name = format!("{stem} ({number}){extension}");
                    let candidate = destination.path(&record.destination_dir, &name);
                    if !destination.exists(&candidate)? {
                        target = candidate;
                        break;
                    }
                    number += 1;
                }
            }
            _ => {}
        }
    }
    if record.source_profile == record.destination_profile && record.source_path == target {
        return Err(
            "Source and destination are the same location. Choose Keep both to create a copy here."
                .into(),
        );
    }
    let total = count_bytes(&mut source, &source_item)?;
    if !record.method.is_empty() {
        update_transfer(app, id, "running", 0, total, Some(&target), None)?;
        let replacing_folder = source_item.is_dir && destination.exists(&target)?;
        if let (Some(from), Some(to)) = (&record.source_profile, &record.destination_profile) {
            crate::direct_transfer::copy_between(
                &profiles::get_profile(&state, from)?,
                &profiles::get_profile(&state, to)?,
                &record.source_path,
                &target,
                replacing_folder,
                flag,
            )?;
            update_transfer(app, id, "running", total, total, Some(&target), None)?;
            return Ok("done");
        }
        let profile_id = record
            .source_profile
            .as_ref()
            .or(record.destination_profile.as_ref())
            .ok_or("SCP and rsync need an SSH host.")?;
        let profile = profiles::get_profile(&state, profile_id)?;
        crate::direct_transfer::copy(
            &record.method,
            &profile,
            record.source_profile.is_some(),
            &record.source_path,
            &target,
            source_item.is_dir,
            replacing_folder,
            flag,
        )?;
        update_transfer(app, id, "running", total, total, Some(&target), None)?;
        return Ok("done");
    }
    let partial = destination.path(&record.destination_dir, &format!(".weave-part-{id}"));
    update_transfer(app, id, "running", 0, total, Some(&target), None)?;
    let mut done = 0;
    let mut last_update = Instant::now();
    let mut rate = RateLimit::new(read_settings(&state)?.speed_limit_kbps);
    copy_item(
        app,
        id,
        flag,
        &mut source,
        &mut destination,
        record.source_profile.is_some() && record.source_profile == record.destination_profile,
        &source_item,
        &partial,
        total,
        &mut done,
        &mut last_update,
        &mut rate,
    )?;
    if !flag.load(Ordering::Relaxed) {
        return Err("Transfer paused.".into());
    }
    if let Err(error) = verify_item(flag, &mut source, &mut destination, &source_item, &partial) {
        if flag.load(Ordering::Relaxed) {
            let _ = destination.remove_tree(&partial);
        }
        return Err(error);
    }
    if destination.exists(&target)? {
        let backup = destination.path(&record.destination_dir, &format!(".weave-backup-{id}"));
        destination.rename(&target, &backup)?;
        if let Err(error) = destination.rename(&partial, &target) {
            let _ = destination.rename(&backup, &target);
            return Err(format!(
                "Could not finish transfer: {error}. The old copy was preserved."
            ));
        }
        destination.remove_tree(&backup)?;
    } else {
        destination.rename(&partial, &target)?;
    }
    update_transfer(app, id, "running", total, total, Some(&target), None)?;
    Ok("done")
}

fn source_is_newer(source: Option<u64>, destination: Option<u64>) -> Result<bool, String> {
    match (source, destination) {
        (Some(source), Some(destination)) => Ok(source > destination),
        _ => Err("Cannot compare modification times for these files. Choose Replace, Keep both, or Skip instead.".into()),
    }
}

fn count_bytes(backend: &mut Backend, item: &Item) -> Result<u64, String> {
    if item.is_symlink {
        return Err(format!(
            "{} is a symbolic link. Copying links is not supported.",
            item.path
        ));
    }
    if !item.is_dir {
        return Ok(item.size);
    }
    let mut total = 0_u64;
    for child in backend.list(&item.path)? {
        total = total.saturating_add(count_bytes(backend, &child)?);
    }
    Ok(total)
}

fn checksum(backend: &mut Backend, item: &Item, flag: &AtomicBool) -> Result<[u8; 32], String> {
    let mut handle = backend.open_read(&item.path, 0)?;
    let mut digest = Sha256::new();
    let mut offset = 0;
    while offset < item.size {
        if !flag.load(Ordering::Relaxed) {
            return Err("Transfer paused.".into());
        }
        let length = (item.size - offset).min(64 * 1024) as usize;
        let data = backend.read_chunk(&mut handle, offset, length)?;
        if data.is_empty() {
            return Err(format!(
                "File changed or ended while verifying: {}",
                item.path
            ));
        }
        digest.update(&data);
        offset += data.len() as u64;
    }
    backend.close(handle, false)?;
    Ok(digest.finalize().into())
}

fn edit_file(backend: &mut Backend, path: &str) -> Result<Item, String> {
    let item = backend.stat(path)?;
    if item.is_dir || item.is_symlink {
        return Err(
            "Choose a regular file. Folders and symbolic links cannot be edited externally.".into(),
        );
    }
    if item.size > MAX_EXTERNAL_EDIT_SIZE {
        return Err("This file is larger than 512 MB. Download it with the transfer queue to edit it locally.".into());
    }
    Ok(item)
}

pub(crate) fn checkout_edit(
    state: &AppState,
    root: &Path,
    profile_id: String,
    remote_path: String,
) -> Result<ExternalEditInfo, String> {
    let mut remote = open_backend(state, &Some(profile_id.clone()))?;
    let item = edit_file(&mut remote, &remote_path)?;
    let filename = Path::new(&remote_path)
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or("Choose a file with a valid name.")?;
    fs::create_dir_all(root)
        .map_err(|error| format!("Could not create the editing copies folder: {error}"))?;
    let directory = tempfile::Builder::new()
        .prefix("weaveduct-edit-")
        .tempdir_in(root)
        .map_err(|error| format!("Could not create a private editing folder: {error}"))?;
    let files_dir = directory.path().join("file");
    fs::create_dir(&files_dir)
        .map_err(|error| format!("Could not create the editing copy folder: {error}"))?;
    let local_path = files_dir.join(filename);
    let mut local = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&local_path)
        .map_err(|error| format!("Could not create the editing copy: {error}"))?;
    let mut source = remote.open_read(&remote_path, 0)?;
    let copied = (|| {
        let mut digest = Sha256::new();
        let mut offset = 0;
        while offset < item.size {
            let bytes = remote.read_chunk(
                &mut source,
                offset,
                (item.size - offset).min(64 * 1024) as usize,
            )?;
            if bytes.is_empty() {
                return Err("The remote file ended during download. Try opening it again.".into());
            }
            local
                .write_all(&bytes)
                .map_err(|error| format!("Could not write the editing copy: {error}"))?;
            digest.update(&bytes);
            offset += bytes.len() as u64;
        }
        local
            .sync_all()
            .map_err(|error| format!("Could not save the editing copy: {error}"))?;
        Ok::<[u8; 32], String>(digest.finalize().into())
    })();
    let closed = remote.close(source, false);
    let original_hash = copied?;
    closed?;
    let current = edit_file(&mut remote, &remote_path)?;
    if current.size != item.size
        || checksum(&mut remote, &current, &AtomicBool::new(true))? != original_hash
    {
        return Err(
            "The remote file changed during download. Open it again to get a consistent copy."
                .into(),
        );
    }
    let id = uuid::Uuid::new_v4().to_string();
    let session = ExternalEditSession {
        directory: directory.path().to_path_buf(),
        profile_id,
        remote_path,
        expected_hash: original_hash,
        auto_upload: false,
    };
    session.save_record(&id)?;
    let _ = directory.keep();
    let info = edit_info(id.clone(), &session, false, None);
    state
        .external_edits
        .lock()
        .map_err(|_| "External editor sessions are busy")?
        .insert(id, session);
    Ok(info)
}

pub(crate) fn upload_edit(
    state: &AppState,
    session: &mut ExternalEditSession,
    id: String,
) -> Result<ExternalEditInfo, String> {
    let local_path = session.local_path();
    let mut local = Backend::Local;
    let local_item = edit_file(&mut local, &local_path.to_string_lossy())?;
    let edited_hash = checksum(&mut local, &local_item, &AtomicBool::new(true))?;
    if edited_hash == session.expected_hash {
        return Ok(edit_info(id, session, false, None));
    }
    let mut remote = open_backend(state, &Some(session.profile_id.clone()))?;
    let original = edit_file(&mut remote, &session.remote_path)?;
    if checksum(&mut remote, &original, &AtomicBool::new(true))? != session.expected_hash {
        return Err("The remote file changed since you opened it. Your local editing copy is intact; download the current remote file before replacing it.".into());
    }
    let suffix = uuid::Uuid::new_v4();
    let temporary = format!("{}.weave-edit-{suffix}", session.remote_path);
    let backup = format!("{}.weave-backup-{suffix}", session.remote_path);
    let uploaded = (|| {
        let mut source = local.open_read(&local_item.path, 0)?;
        let mut target = remote.open_write(&temporary, true, 0)?;
        let copied = (|| {
            let mut offset = 0;
            while offset < local_item.size {
                let bytes = local.read_chunk(
                    &mut source,
                    offset,
                    (local_item.size - offset).min(64 * 1024) as usize,
                )?;
                if bytes.is_empty() {
                    return Err(
                        "The local editing copy changed during upload. Save it and try again."
                            .into(),
                    );
                }
                remote.write_chunk(&mut target, offset, &bytes)?;
                offset += bytes.len() as u64;
            }
            Ok::<(), String>(())
        })();
        let source_closed = local.close(source, false);
        let target_closed = remote.close(target, true);
        copied?;
        source_closed?;
        target_closed?;
        let staged = edit_file(&mut remote, &temporary)?;
        if staged.size != local_item.size
            || checksum(&mut remote, &staged, &AtomicBool::new(true))? != edited_hash
        {
            return Err("The server returned different data after upload. The original remote file was kept.".into());
        }
        let current_local = edit_file(&mut local, &local_item.path)?;
        if current_local.size != local_item.size
            || checksum(&mut local, &current_local, &AtomicBool::new(true))? != edited_hash
        {
            return Err(
                "The local editing copy changed during upload. Save it and try again.".into(),
            );
        }
        let current_remote = edit_file(&mut remote, &session.remote_path)?;
        if checksum(&mut remote, &current_remote, &AtomicBool::new(true))? != session.expected_hash
        {
            return Err("The remote file changed during upload. Your local editing copy is intact; the remote file was kept.".into());
        }
        if let Backend::Remote(client) = &mut remote {
            let permissions = client
                .stat(&session.remote_path)?
                .permissions
                .unwrap_or(0o100644)
                & 0o7777;
            client.chmod(&temporary, permissions)?;
        }
        remote.rename(&session.remote_path, &backup)?;
        if let Err(error) = remote.rename(&temporary, &session.remote_path) {
            let restored = remote.rename(&backup, &session.remote_path);
            return Err(match restored {
                Ok(()) => format!("Could not replace the remote file: {error}. The original was restored."),
                Err(restore_error) => format!("Could not replace the remote file: {error}. Restore the original from {backup}: {restore_error}"),
            });
        }
        session.expected_hash = edited_hash;
        let warnings = [
            remote.remove_tree(&backup).err().map(|error| {
                format!("File saved, but its backup at {backup} could not be removed: {error}")
            }),
            session.save_record(&id).err().map(|error| {
                format!("File saved, but its editing session could not be recorded for recovery: {error}")
            }),
        ];
        let warning = warnings.into_iter().flatten().collect::<Vec<_>>().join(" ");
        let warning = (!warning.is_empty()).then_some(warning);
        Ok(edit_info(id, session, true, warning))
    })();
    if uploaded.is_err() {
        let _ = remote.remove_tree(&temporary);
    }
    uploaded
}

#[tauri::command]
pub async fn external_edit_checkout(
    app: AppHandle,
    profile_id: String,
    path: String,
) -> Result<ExternalEditInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("Could not locate saved editing copies: {error}"))?
            .join("external-edits");
        checkout_edit(&app.state::<AppState>(), &root, profile_id, path)
    })
    .await
    .map_err(|error| format!("External editor checkout stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub fn external_edit_list(state: State<'_, AppState>) -> Result<Vec<ExternalEditInfo>, String> {
    let sessions = state
        .external_edits
        .lock()
        .map_err(|_| "External editor sessions are busy")?;
    let mut items = sessions
        .iter()
        .map(|(id, session)| edit_info(id.clone(), session, false, None))
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.local_path.cmp(&right.local_path));
    Ok(items)
}

#[tauri::command]
pub async fn external_edit_upload(app: AppHandle, id: String) -> Result<ExternalEditInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut session = state
            .external_edits
            .lock()
            .map_err(|_| "External editor sessions are busy")?
            .remove(&id)
            .ok_or("This editing session has ended. Open the remote file again.")?;
        let result = upload_edit(&state, &mut session, id.clone());
        state
            .external_edits
            .lock()
            .map_err(|_| "External editor sessions are busy")?
            .insert(id, session);
        result
    })
    .await
    .map_err(|error| format!("External editor upload stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub fn external_edit_auto_upload(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<ExternalEditInfo, String> {
    let mut sessions = state
        .external_edits
        .lock()
        .map_err(|_| "External editor sessions are busy")?;
    let session = sessions
        .get_mut(&id)
        .ok_or("This editing session has ended. Open the remote file again.")?;
    session.auto_upload = enabled;
    session.save_record(&id)?;
    Ok(edit_info(id, session, false, None))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AutoUploadEvent {
    id: String,
    remote_path: String,
    error: Option<String>,
}

type Signature = (SystemTime, u64);

fn file_signature(path: &Path) -> Option<Signature> {
    fs::metadata(path)
        .ok()
        .map(|meta| (meta.modified().unwrap_or(UNIX_EPOCH), meta.len()))
}

/// The editing copy as last uploaded, and a newer version waiting to settle.
struct Watched {
    uploaded: Signature,
    pending: Option<(Signature, Instant)>,
}

impl Watched {
    /// True once a change has stayed the same for a moment, so editors that
    /// save in several steps are uploaded once, after they finish.
    fn ready(&mut self, current: Signature, now: Instant) -> bool {
        if current == self.uploaded {
            self.pending = None;
            return false;
        }
        match self.pending {
            Some((pending, since)) if pending == current => {
                if now.duration_since(since) < Duration::from_millis(1500) {
                    return false;
                }
                self.uploaded = current;
                self.pending = None;
                true
            }
            _ => {
                self.pending = Some((current, now));
                false
            }
        }
    }
}

/// Uploads editing copies with automatic upload on, once a save has settled.
/// A failed upload turns automatic upload off so it is not retried blindly.
pub fn watch_external_edits<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let mut seen: HashMap<String, Watched> = HashMap::new();
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let state = app.state::<AppState>();
            let candidates: Vec<(String, std::path::PathBuf)> = match state.external_edits.lock() {
                Ok(sessions) => sessions
                    .iter()
                    .filter(|(_, session)| session.auto_upload)
                    .map(|(id, session)| (id.clone(), session.local_path()))
                    .collect(),
                Err(_) => continue,
            };
            seen.retain(|id, _| candidates.iter().any(|(candidate, _)| candidate == id));
            for (id, path) in candidates {
                let Some(current) = file_signature(&path) else {
                    continue;
                };
                let watched = seen.entry(id.clone()).or_insert(Watched {
                    uploaded: current,
                    pending: None,
                });
                if !watched.ready(current, Instant::now()) {
                    continue;
                }
                let Some(mut session) = state
                    .external_edits
                    .lock()
                    .ok()
                    .and_then(|mut sessions| sessions.remove(&id))
                else {
                    continue;
                };
                let result = upload_edit(&state, &mut session, id.clone());
                if result.is_err() {
                    session.auto_upload = false;
                    let _ = session.save_record(&id);
                }
                let event = AutoUploadEvent {
                    id: id.clone(),
                    remote_path: session.remote_path.clone(),
                    error: result.err(),
                };
                if let Ok(mut sessions) = state.external_edits.lock() {
                    sessions.insert(id, session);
                }
                let _ = app.emit("external-edit-uploaded", event);
            }
        }
    });
}

#[tauri::command]
pub fn external_edit_discard(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut sessions = state
        .external_edits
        .lock()
        .map_err(|_| "External editor sessions are busy")?;
    let session = sessions
        .remove(&id)
        .ok_or("This editing session has already ended.")?;
    if let Err(error) = fs::remove_dir_all(&session.directory) {
        sessions.insert(id, session);
        return Err(format!("Could not remove the editing copy: {error}"));
    }
    Ok(())
}

fn verify_item(
    flag: &AtomicBool,
    source: &mut Backend,
    destination: &mut Backend,
    item: &Item,
    target: &str,
) -> Result<(), String> {
    if !flag.load(Ordering::Relaxed) {
        return Err("Transfer paused.".into());
    }
    let current_source = source.stat(&item.path)?;
    if current_source.size != item.size
        || (item.modified.is_some() && current_source.modified != item.modified)
    {
        return Err(format!(
            "Source changed while copying: {}. Retry the transfer.",
            item.path
        ));
    }
    let copy = destination.stat(target)?;
    if item.is_dir {
        if !copy.is_dir {
            return Err(format!("Verification failed: {target} is not a folder."));
        }
        let children = source.list(&item.path)?;
        let copies = destination.list(target)?;
        if children.len() != copies.len() {
            return Err(format!("Verification failed: folder contents differ at {target}. The original copy was preserved."));
        }
        for child in children {
            let next = destination.path(target, &child.name);
            verify_item(flag, source, destination, &child, &next)?;
        }
        return Ok(());
    }
    if copy.is_dir
        || copy.size != item.size
        || checksum(source, item, flag)? != checksum(destination, &copy, flag)?
    {
        return Err(format!("Checksum verification failed for {}. The original copy was preserved; retry the transfer.", item.path));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn copy_item<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    flag: &AtomicBool,
    source: &mut Backend,
    destination: &mut Backend,
    same_profile: bool,
    item: &Item,
    target: &str,
    total: u64,
    done: &mut u64,
    last_update: &mut Instant,
    rate: &mut RateLimit,
) -> Result<(), String> {
    if !flag.load(Ordering::Relaxed) {
        return Err("Transfer paused.".into());
    }
    if item.is_dir {
        if !destination.exists(target)? {
            destination.mkdir(target)?;
        }
        for child in source.list(&item.path)? {
            let next = destination.path(target, &child.name);
            copy_item(
                app,
                id,
                flag,
                source,
                destination,
                same_profile,
                &child,
                &next,
                total,
                done,
                last_update,
                rate,
            )?;
        }
        return Ok(());
    }
    let existing = if destination.exists(target)? {
        let partial = destination.stat(target)?;
        if partial.is_symlink || partial.is_dir {
            return Err(format!(
                "Cannot resume into {target}: the partial copy is a link or folder. Remove it and retry."
            ));
        }
        partial.size
    } else {
        0
    };
    let mut offset = if existing > item.size { 0 } else { existing };
    *done = done.saturating_add(offset);
    if same_profile && offset == 0 && rate.bytes_per_second == 0 {
        if let Backend::Remote(client) = destination {
            if client.supports_copy_data() {
                let from = client.open_read(&item.path)?;
                let to = match client.open_write(target, true) {
                    Ok(handle) => handle,
                    Err(error) => {
                        let _ = client.close(&from);
                        return Err(error);
                    }
                };
                let copied = (|| {
                    while offset < item.size {
                        if !flag.load(Ordering::Relaxed) {
                            return Err("Transfer paused.".into());
                        }
                        let length = (item.size - offset).min(8 * 1024 * 1024);
                        client.copy_data(&from, &to, offset, length)?;
                        offset += length;
                        *done += length;
                        update_transfer(app, id, "running", *done, total, None, None)?;
                    }
                    Ok::<(), String>(())
                })();
                let from_closed = client.close(&from);
                let to_closed = client.close(&to);
                copied?;
                from_closed?;
                to_closed?;
                if let Some(modified) = item.modified {
                    let _ = client.set_mtime(target, modified);
                }
                return Ok(());
            }
        }
    }
    let fresh = offset == 0;
    let mut reader = source.open_read(&item.path, offset)?;
    let mut writer = destination.open_write(target, fresh, offset)?;
    while offset < item.size {
        if !flag.load(Ordering::Relaxed) {
            return Err("Transfer paused.".into());
        }
        let length = (item.size - offset).min(rate.chunk_size()) as usize;
        let data = source.read_chunk(&mut reader, offset, length)?;
        if data.is_empty() {
            return Err(format!("Source file ended before expected: {}", item.path));
        }
        destination.write_chunk(&mut writer, offset, &data)?;
        offset += data.len() as u64;
        *done += data.len() as u64;
        rate.pace(data.len() as u64, flag)?;
        if last_update.elapsed() >= Duration::from_millis(200) {
            update_transfer(app, id, "running", *done, total, None, None)?;
            *last_update = Instant::now();
        }
    }
    source.close(reader, false)?;
    if let (FileHandle::WebDavWrite(upload), Some(modified)) = (&mut writer, item.modified) {
        upload.set_modified(modified);
    }
    destination.close(writer, true)?;
    if let Some(modified) = item.modified {
        let _ = destination.set_modified(target, modified);
    }
    update_transfer(app, id, "running", *done, total, None, None)?;
    Ok(())
}

fn split_name(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((stem, _extension)) if !stem.is_empty() => (stem, &name[stem.len()..]),
        _ => (name, ""),
    }
}

pub(crate) mod script;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_listing_names_that_leave_the_folder() {
        for name in ["report.csv", "a\\b", "..hidden", "...", "ü file"] {
            assert!(safe_entry_name(name), "{name}");
        }
        for name in ["", ".", "..", "../x", "a/b", "nul\0"] {
            assert!(!safe_entry_name(name), "{name:?}");
        }
    }

    #[test]
    fn matches_name_masks() {
        assert!(mask_matches("*.log", "Server.LOG"));
        assert!(mask_matches("report-??.pdf", "report-07.pdf"));
        assert!(!mask_matches("report-??.pdf", "report-7.pdf"));
        assert!(mask_matches("*.jpg; *.png", "a.png"));
        assert!(!mask_matches("*.jpg", "a.jpeg"));
    }

    #[test]
    fn finds_files_and_reads_previews() {
        let state = AppState::for_tests();
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("a/b")).unwrap();
        fs::write(root.path().join("a/b/error.log"), "boom").unwrap();
        fs::write(root.path().join("a/notes.txt"), "hello world").unwrap();
        let base = root.path().to_string_lossy().to_string();
        let (found, more) = find_files(&state, &None, &base, "*.log", 10).unwrap();
        assert_eq!(
            found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["error.log"]
        );
        assert!(!more);
        let (by_word, _) = find_files(&state, &None, &base, "note", 10).unwrap();
        assert_eq!(by_word[0].name, "notes.txt");
        let (bytes, size) = read_bytes(&state, &None, &format!("{base}/a/notes.txt"), 5).unwrap();
        assert_eq!((bytes.as_slice(), size), (&b"hello"[..], 11));
        assert!(read_bytes(&state, &None, &format!("{base}/a"), 5).is_err());
    }

    #[test]
    fn leaves_excluded_names_out_of_compare_and_mirror() {
        let state = AppState::for_tests();
        let root = tempfile::tempdir().unwrap();
        let (left, right) = (root.path().join("left"), root.path().join("right"));
        fs::create_dir_all(left.join("node_modules")).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("app.js"), "x").unwrap();
        fs::write(left.join("node_modules/lib.js"), "x").unwrap();
        fs::write(left.join("cache.tmp"), "x").unwrap();
        fs::write(right.join("local.tmp"), "keep me").unwrap();
        let (l, r) = (
            left.to_string_lossy().to_string(),
            right.to_string_lossy().to_string(),
        );
        let plan = plan_sync_excluding(
            &state,
            &None,
            &l,
            &None,
            &r,
            "mirror-left-to-right",
            "*.tmp; node_modules",
        )
        .unwrap();
        let copied: Vec<_> = plan
            .copies
            .iter()
            .map(|copy| {
                Path::new(&copy.source_path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(copied, ["app.js"]);
        assert!(
            plan.deletes.is_empty(),
            "excluded destination files must never be deleted"
        );
        let all =
            plan_sync_excluding(&state, &None, &l, &None, &r, "mirror-left-to-right", "").unwrap();
        assert_eq!(all.copies.len(), 3);
        assert_eq!(all.deletes.len(), 1);
    }

    #[test]
    fn waits_for_a_save_to_settle_before_uploading() {
        let start = Instant::now();
        let first = (UNIX_EPOCH + Duration::from_secs(1), 10);
        let second = (UNIX_EPOCH + Duration::from_secs(2), 12);
        let mut watched = Watched {
            uploaded: first,
            pending: None,
        };
        assert!(!watched.ready(first, start));
        assert!(!watched.ready(second, start));
        assert!(!watched.ready(second, start + Duration::from_millis(500)));
        assert!(watched.ready(second, start + Duration::from_millis(1600)));
        assert!(!watched.ready(second, start + Duration::from_secs(5)));
        let third = (UNIX_EPOCH + Duration::from_secs(3), 12);
        assert!(!watched.ready(third, start + Duration::from_secs(6)));
        let fourth = (UNIX_EPOCH + Duration::from_secs(4), 14);
        assert!(!watched.ready(fourth, start + Duration::from_secs(7)));
        assert!(watched.ready(fourth, start + Duration::from_secs(9)));
    }

    #[test]
    fn classifies_retryable_errors() {
        assert!(retryable(
            "Could not read FTP data: Connection reset by peer"
        ));
        assert!(retryable(
            "Invalid response: [421] 421 Passive data channel timed out."
        ));
        assert!(!retryable("Permission denied"));
        assert!(!retryable(
            "Cannot copy a folder into itself or a subfolder."
        ));
    }

    #[test]
    fn folder_fingerprint_changes_when_the_tree_changes() {
        let state = AppState::for_tests();
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().to_string_lossy().to_string();
        fs::create_dir(root.path().join("sub")).unwrap();
        fs::write(root.path().join("sub/a.txt"), "one").unwrap();
        let first = folder_fingerprint(&state, &None, &folder).unwrap();
        assert_eq!(folder_fingerprint(&state, &None, &folder).unwrap(), first);
        fs::write(root.path().join("sub/a.txt"), "longer").unwrap();
        let second = folder_fingerprint(&state, &None, &folder).unwrap();
        assert_ne!(second, first);
        fs::write(root.path().join("b.txt"), "").unwrap();
        assert_ne!(folder_fingerprint(&state, &None, &folder).unwrap(), second);
    }
    use crate::profiles::Profile;
    use rusqlite::Connection;
    use std::{collections::HashMap, sync::Mutex};
    use zeroize::Zeroizing;

    #[test]
    fn saved_external_edit_survives_restart_until_discarded() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("edit-one");
        fs::create_dir_all(directory.join("file")).unwrap();
        fs::write(directory.join("file/notes.bin"), [0_u8, 255]).unwrap();
        let session = ExternalEditSession {
            directory: directory.clone(),
            profile_id: "profile-one".into(),
            remote_path: "/notes.bin".into(),
            expected_hash: Sha256::digest([0_u8, 255]).into(),
            auto_upload: true,
        };
        let id = uuid::Uuid::new_v4().to_string();
        session.save_record(&id).unwrap();
        let restored = restore_external_edits(root.path()).unwrap();
        assert_eq!(restored.len(), 1);
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(Connection::open_in_memory().unwrap()),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(restored),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        assert_eq!(external_edit_list(app.state()).unwrap().len(), 1);
        external_edit_discard(app.state(), id).unwrap();
        assert!(!directory.exists());
        assert!(external_edit_list(app.state()).unwrap().is_empty());
    }

    #[test]
    fn batch_copy_plan_previews_and_queues_atomically() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        fs::create_dir(&destination).unwrap();
        let first = root.path().join("first.txt");
        let second = root.path().join("second.txt");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let manifest_path = root.path().join("copies.json");
        let manifest = serde_json::json!({
            "version": 1,
            "transfers": [
                {"sourcePath": first, "destinationDir": destination, "conflict": "overwrite"},
                {"sourcePath": second, "destinationDir": destination, "conflict": "skip"}
            ]
        });
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, protocol TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT, color TEXT, favorite INTEGER, tls_pin_sha256 TEXT, s3_region TEXT); CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER NOT NULL DEFAULT 0, total_bytes INTEGER NOT NULL DEFAULT 0, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER DEFAULT (unixepoch()), updated_at INTEGER DEFAULT (unixepoch())); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        let state = app.state::<AppState>();
        let path = manifest_path.to_string_lossy();
        let preview = read_batch_manifest(&state, &path).unwrap();
        assert_eq!(preview.transfers.len(), 2);
        state.db.lock().unwrap().execute("INSERT INTO profiles (id, name, host, protocol, port, username, color, favorite) VALUES ('profile-one', 'Office NAS', 'example.test', 'ssh', 22, 'alice', '#9d91ec', 0)", []).unwrap();
        let mut named = manifest.clone();
        named["transfers"][0]["sourceProfile"] = serde_json::json!("Office NAS");
        fs::write(&manifest_path, serde_json::to_vec(&named).unwrap()).unwrap();
        assert_eq!(
            read_batch_manifest(&state, &path).unwrap().transfers[0]
                .source_profile
                .as_deref(),
            Some("profile-one")
        );
        let mut changed = manifest.clone();
        changed["transfers"][1]["conflict"] = serde_json::json!("rename");
        fs::write(&manifest_path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(queue_batch(app.handle(), &state, &path, &preview.sha256)
            .err()
            .unwrap()
            .contains("changed after preview"));
        assert_eq!(
            state
                .db
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM transfers", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let mut missing_source = manifest.clone();
        missing_source["transfers"][0]["sourcePath"] =
            serde_json::json!(root.path().join("missing.txt"));
        fs::write(&manifest_path, serde_json::to_vec(&missing_source).unwrap()).unwrap();
        assert!(read_batch_manifest(&state, &path)
            .err()
            .unwrap()
            .contains("local source"));
        let mut file_destination = manifest.clone();
        file_destination["transfers"][0]["destinationDir"] = serde_json::json!(first);
        fs::write(
            &manifest_path,
            serde_json::to_vec(&file_destination).unwrap(),
        )
        .unwrap();
        assert!(read_batch_manifest(&state, &path)
            .err()
            .unwrap()
            .contains("existing folder"));
        let nested = root.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let child = nested.join("child");
        fs::create_dir(&child).unwrap();
        let mut recursive = manifest.clone();
        recursive["transfers"][0]["sourcePath"] = serde_json::json!(nested);
        recursive["transfers"][0]["destinationDir"] = serde_json::json!(child);
        fs::write(&manifest_path, serde_json::to_vec(&recursive).unwrap()).unwrap();
        assert!(read_batch_manifest(&state, &path)
            .err()
            .unwrap()
            .contains("subfolders"));
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let result = queue_batch(app.handle(), &state, &path, &preview.sha256).unwrap();
        assert_eq!(result.records.len(), 2);
        assert!(result.warning.is_none());
        for _ in 0..100 {
            if destination.join("first.txt").exists() && destination.join("second.txt").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fs::read_to_string(destination.join("first.txt")).unwrap(),
            "first"
        );
        assert_eq!(
            fs::read_to_string(destination.join("second.txt")).unwrap(),
            "second"
        );
    }

    fn exercise_external_edit(state: &AppState, root: &Path, profile_id: &str, remote_path: &str) {
        let checked_out =
            checkout_edit(state, root, profile_id.into(), remote_path.into()).unwrap();
        let recovered = restore_external_edits(root).unwrap();
        assert!(recovered.contains_key(&checked_out.id));
        let mut session = state
            .external_edits
            .lock()
            .unwrap()
            .remove(&checked_out.id)
            .unwrap();
        assert_eq!(
            fs::read(session.local_path()).unwrap(),
            b"from the local computer"
        );
        let edited = (0..1_100_000)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(session.local_path(), &edited).unwrap();
        let uploaded = upload_edit(state, &mut session, checked_out.id).unwrap();
        assert!(uploaded.changed);
        assert_eq!(
            restore_external_edits(root)
                .unwrap()
                .get(&uploaded.id)
                .unwrap()
                .expected_hash,
            session.expected_hash,
        );
        let mut remote = open_backend(state, &Some(profile_id.into())).unwrap();
        let item = remote.stat(remote_path).unwrap();
        assert_eq!(item.size, edited.len() as u64);
        assert_eq!(
            checksum(&mut remote, &item, &AtomicBool::new(true)).unwrap(),
            <[u8; 32]>::from(Sha256::digest(&edited))
        );

        let stale = checkout_edit(state, root, profile_id.into(), remote_path.into()).unwrap();
        let mut stale_session = state
            .external_edits
            .lock()
            .unwrap()
            .remove(&stale.id)
            .unwrap();
        fs::write(stale_session.local_path(), b"my changes").unwrap();
        let mut writer = remote.open_write(remote_path, true, 0).unwrap();
        remote
            .write_chunk(&mut writer, 0, b"new remote data")
            .unwrap();
        remote.close(writer, true).unwrap();
        let error = upload_edit(state, &mut stale_session, stale.id).unwrap_err();
        assert!(error.contains("remote file changed"), "{error}");
        let current = remote.stat(remote_path).unwrap();
        assert_eq!(
            checksum(&mut remote, &current, &AtomicBool::new(true)).unwrap(),
            <[u8; 32]>::from(Sha256::digest(b"new remote data"))
        );
        assert_eq!(fs::read(stale_session.local_path()).unwrap(), b"my changes");
    }

    #[test]
    #[ignore = "requires the local WebDAV test server"]
    fn transfers_through_a_saved_webdav_profile() {
        let port = std::env::var("WEAVE_WEBDAV_TEST_PORT").expect("Set WEAVE_WEBDAV_TEST_PORT");
        let root =
            std::env::temp_dir().join(format!("weave-webdav-transfer-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let local_file = root.join("upload.txt");
        fs::write(&local_file, "from the local computer").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&local_file)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1_600_000_000)),
            )
            .unwrap();
        let profile = Profile {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Local WebDAV test".into(),
            host: format!("http://127.0.0.1:{port}/dav/"),
            protocol: "webdav".into(),
            port: 443,
            username: "weave".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let remote_dir = format!("/transfer-{}", uuid::Uuid::new_v4());
        let client = WebDavClient::connect(&profile, "weave-test").unwrap();
        client.mkdir(&remote_dir).unwrap();
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, protocol TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT, color TEXT, favorite INTEGER, tls_pin_sha256 TEXT, s3_region TEXT); CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER, total_bytes INTEGER, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        db.execute(
            "INSERT INTO profiles VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7, 0, NULL, NULL)",
            params![
                profile.id,
                profile.name,
                profile.host,
                profile.protocol,
                profile.port,
                profile.username,
                profile.color
            ],
        )
        .unwrap();
        let upload_id = uuid::Uuid::new_v4().to_string();
        db.execute("INSERT INTO transfers (id, source_path, destination_profile, destination_dir, conflict, status, bytes_done, total_bytes, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'overwrite', 'queued', 0, 0, 0, 0)", params![upload_id, local_file.to_string_lossy(), profile.id, remote_dir]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::from([(
                profile.id.clone(),
                Zeroizing::new("weave-test".into()),
            )])),
        });
        assert_eq!(
            run_transfer(app.handle(), &upload_id, &AtomicBool::new(true)).unwrap(),
            "done"
        );
        let remote_file = join_remote(&remote_dir, "upload.txt");
        assert_eq!(
            client.stat(&remote_file).unwrap().modified,
            Some(1_600_000_000)
        );
        let download_dir = root.join("download");
        fs::create_dir(&download_dir).unwrap();
        let download_id = uuid::Uuid::new_v4().to_string();
        let state = app.state::<AppState>();
        state.db.lock().unwrap().execute("INSERT INTO transfers (id, source_profile, source_path, destination_dir, conflict, status, bytes_done, total_bytes, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'overwrite', 'queued', 0, 0, 0, 0)", params![download_id, profile.id, remote_file, download_dir.to_string_lossy()]).unwrap();
        assert_eq!(
            run_transfer(app.handle(), &download_id, &AtomicBool::new(true)).unwrap(),
            "done"
        );
        assert_eq!(
            fs::read_to_string(download_dir.join("upload.txt")).unwrap(),
            "from the local computer"
        );
        exercise_external_edit(
            &state,
            &root.join("external-edits"),
            &profile.id,
            &remote_file,
        );
        client.delete(&remote_dir, true).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires the local S3 test server"]
    fn s3_integration_transfers_and_external_edits() {
        let port = std::env::var("WEAVE_S3_TEST_PORT").unwrap();
        let root = tempfile::tempdir().unwrap();
        let local_file = root.path().join("upload.txt");
        fs::write(&local_file, "from the local computer").unwrap();
        let profile = Profile {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Local S3 test".into(),
            host: format!("http://127.0.0.1:{port}/weave-test"),
            protocol: "s3".into(),
            port: 443,
            username: "AKIDTEMP".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: Some("us-east-1".into()),
        };
        let remote_dir = format!("/transfer-{}", uuid::Uuid::new_v4());
        let client = S3Client::connect_with_token(
            &profile,
            "weave-test-secret",
            Some("weave+token/2026=temporary"),
        )
        .unwrap();
        client.mkdir(&remote_dir).unwrap();
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, protocol TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT, color TEXT, favorite INTEGER, tls_pin_sha256 TEXT, s3_region TEXT); CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER, total_bytes INTEGER, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        db.execute(
            "INSERT INTO profiles VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7, 0, NULL, ?8)",
            params![
                profile.id,
                profile.name,
                profile.host,
                profile.protocol,
                profile.port,
                profile.username,
                profile.color,
                profile.s3_region
            ],
        )
        .unwrap();
        let upload_id = uuid::Uuid::new_v4().to_string();
        db.execute("INSERT INTO transfers (id, source_path, destination_profile, destination_dir, conflict, status, bytes_done, total_bytes, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'overwrite', 'queued', 0, 0, 0, 0)", params![upload_id, local_file.to_string_lossy(), profile.id, remote_dir]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::from([(
                profile.id.clone(),
                Zeroizing::new("weave+token/2026=temporary".into()),
            )])),
            session_passwords: Mutex::new(HashMap::from([(
                profile.id.clone(),
                Zeroizing::new("weave-test-secret".into()),
            )])),
        });
        assert_eq!(
            run_transfer(app.handle(), &upload_id, &AtomicBool::new(true)).unwrap(),
            "done"
        );
        let remote_file = join_remote(&remote_dir, "upload.txt");
        let download = root.path().join("download");
        fs::create_dir(&download).unwrap();
        let download_id = uuid::Uuid::new_v4().to_string();
        let state = app.state::<AppState>();
        state.db.lock().unwrap().execute("INSERT INTO transfers (id, source_profile, source_path, destination_dir, conflict, status, bytes_done, total_bytes, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'overwrite', 'queued', 0, 0, 0, 0)", params![download_id, profile.id, remote_file, download.to_string_lossy()]).unwrap();
        assert_eq!(
            run_transfer(app.handle(), &download_id, &AtomicBool::new(true)).unwrap(),
            "done"
        );
        assert_eq!(
            fs::read_to_string(download.join("upload.txt")).unwrap(),
            "from the local computer"
        );
        exercise_external_edit(
            &state,
            &root.path().join("external-edits"),
            &profile.id,
            &remote_file,
        );
        client.delete(&remote_dir, true).unwrap();
    }

    #[test]
    fn newer_only_requires_both_modification_times() {
        assert!(source_is_newer(Some(12), Some(11)).unwrap());
        assert!(!source_is_newer(Some(11), Some(12)).unwrap());
        assert!(source_is_newer(None, Some(12)).is_err());
        assert!(source_is_newer(Some(12), None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_partial_copy_that_points_to_another_file() {
        let root = std::env::temp_dir().join(format!("weave-transfer-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("data.txt"), "new data").unwrap();
        let protected = root.join("protected.txt");
        fs::write(&protected, "keep me").unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let partial = destination.join(format!(".weave-part-{id}"));
        std::os::unix::fs::symlink(&protected, &partial).unwrap();

        let mut local = Backend::Local;
        assert!(local
            .open_write(&partial.to_string_lossy(), true, 0)
            .is_err());
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER, total_bytes INTEGER, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        db.execute("INSERT INTO transfers (id, source_path, destination_dir, conflict, status, bytes_done, total_bytes, created_at, updated_at) VALUES (?1, ?2, ?3, 'overwrite', 'paused', 0, 0, 0, 0)", params![id, source.join("data.txt").to_string_lossy(), destination.to_string_lossy()]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        assert!(run_transfer(app.handle(), &id, &AtomicBool::new(true))
            .unwrap_err()
            .contains("partial copy is a link"));
        assert_eq!(fs::read_to_string(protected).unwrap(), "keep me");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires the local FTP test server"]
    fn transfers_through_a_saved_ftp_profile() {
        let port = std::env::var("WEAVE_FTP_TEST_PORT")
            .expect("Set WEAVE_FTP_TEST_PORT to the local FTP test server port");
        let root = std::env::temp_dir().join(format!("weave-ftp-profile-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let local_file = root.join("upload.txt");
        fs::write(&local_file, "from the local computer").unwrap();
        let remote_dir = format!("/profile-{}", uuid::Uuid::new_v4());
        let profile = Profile {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Local FTP test".into(),
            host: "127.0.0.1".into(),
            protocol: "ftp".into(),
            port: port.parse().unwrap(),
            username: "weave".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        FtpClient::connect(&profile, "weave-test")
            .unwrap()
            .mkdir(&remote_dir)
            .unwrap();
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, protocol TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT, color TEXT, favorite INTEGER, tls_pin_sha256 TEXT, s3_region TEXT); CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER, total_bytes INTEGER, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        db.execute(
            "INSERT INTO profiles VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7, 0, NULL, NULL)",
            params![
                profile.id,
                profile.name,
                profile.host,
                profile.protocol,
                profile.port,
                profile.username,
                profile.color
            ],
        )
        .unwrap();
        let upload_id = uuid::Uuid::new_v4().to_string();
        db.execute("INSERT INTO transfers (id, source_path, destination_profile, destination_dir, conflict, status, bytes_done, total_bytes, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'overwrite', 'queued', 0, 0, 0, 0)", params![upload_id, local_file.to_string_lossy(), profile.id, remote_dir]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::from([(
                profile.id.clone(),
                Zeroizing::new("weave-test".into()),
            )])),
        });
        assert_eq!(
            run_transfer(app.handle(), &upload_id, &AtomicBool::new(true)).unwrap(),
            "done"
        );
        let remote_file = join_remote(&remote_dir, "upload.txt");
        let download_dir = root.join("download");
        fs::create_dir(&download_dir).unwrap();
        let download_id = uuid::Uuid::new_v4().to_string();
        let state = app.state::<AppState>();
        state.db.lock().unwrap().execute("INSERT INTO transfers (id, source_profile, source_path, destination_dir, conflict, status, bytes_done, total_bytes, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'overwrite', 'queued', 0, 0, 0, 0)", params![download_id, profile.id, remote_file, download_dir.to_string_lossy()]).unwrap();
        assert_eq!(
            run_transfer(app.handle(), &download_id, &AtomicBool::new(true)).unwrap(),
            "done"
        );
        assert_eq!(
            fs::read_to_string(download_dir.join("upload.txt")).unwrap(),
            "from the local computer"
        );
        exercise_external_edit(
            &state,
            &root.join("external-edits"),
            &profile.id,
            &remote_file,
        );
        FtpClient::connect(&profile, "weave-test")
            .unwrap()
            .delete(&remote_dir, true)
            .unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires the local FTP test server"]
    fn resumes_and_verifies_ftp_upload_and_download() {
        let port = std::env::var("WEAVE_FTP_TEST_PORT")
            .expect("Set WEAVE_FTP_TEST_PORT to the local FTP test server port");
        let profile = Profile {
            id: "ftp-transfer-test".into(),
            name: "Local FTP test".into(),
            host: "127.0.0.1".into(),
            protocol: "ftp".into(),
            port: port.parse().unwrap(),
            username: "weave".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let root =
            std::env::temp_dir().join(format!("weave-ftp-transfer-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let contents = (0..150_000)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let source_path = root.join("data.bin");
        fs::write(&source_path, &contents).unwrap();
        let remote_dir = format!("/transfer-{}", uuid::Uuid::new_v4());
        let remote_path = join_remote(&remote_dir, "data.bin");
        let mut client = FtpClient::connect(&profile, "weave-test").unwrap();
        client.mkdir(&remote_dir).unwrap();
        let mut partial = client.open_write(&remote_path, false).unwrap();
        partial.write_all(&contents[..50_000]).unwrap();
        partial.finish().unwrap();

        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER, total_bytes INTEGER, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        db.execute("INSERT INTO transfers (id, source_path, destination_dir, conflict, status, bytes_done, total_bytes, created_at, updated_at) VALUES (?1, ?2, ?3, 'overwrite', 'running', 0, 150000, 0, 0)", params![id, source_path.to_string_lossy(), remote_dir]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        let flag = AtomicBool::new(true);
        let mut local = Backend::Local;
        let mut remote = Backend::Ftp(client);
        let item = local.stat(&source_path.to_string_lossy()).unwrap();
        let mut done = 0;
        copy_item(
            app.handle(),
            &id,
            &flag,
            &mut local,
            &mut remote,
            false,
            &item,
            &remote_path,
            contents.len() as u64,
            &mut done,
            &mut Instant::now(),
            &mut RateLimit::new(0),
        )
        .unwrap();
        assert_eq!(done, contents.len() as u64);
        verify_item(&flag, &mut local, &mut remote, &item, &remote_path).unwrap();

        let download = root.join("download.bin");
        fs::write(&download, &contents[..65_000]).unwrap();
        let remote_item = remote.stat(&remote_path).unwrap();
        done = 0;
        copy_item(
            app.handle(),
            &id,
            &flag,
            &mut remote,
            &mut local,
            false,
            &remote_item,
            &download.to_string_lossy(),
            contents.len() as u64,
            &mut done,
            &mut Instant::now(),
            &mut RateLimit::new(0),
        )
        .unwrap();
        assert_eq!(done, contents.len() as u64);
        assert_eq!(fs::read(&download).unwrap(), contents);
        if let Backend::Ftp(client) = &mut remote {
            client.delete(&remote_dir, true).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preserves_extension_when_renaming_conflict() {
        assert_eq!(split_name("report.csv"), ("report", ".csv"));
        assert_eq!(split_name(".config"), (".config", ""));
    }

    #[test]
    fn reorders_only_queued_transfers() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE transfers (id TEXT PRIMARY KEY, status TEXT, queue_order INTEGER);
             INSERT INTO transfers VALUES ('one', 'queued', 1);
             INSERT INTO transfers VALUES ('running', 'running', 2);
             INSERT INTO transfers VALUES ('two', 'queued', 3);
             INSERT INTO transfers VALUES ('three', 'queued', 4);",
        )
        .unwrap();
        move_queued(&mut db, "three", "up").unwrap();
        let mut statement = db
            .prepare("SELECT id FROM transfers WHERE status='queued' ORDER BY queue_order")
            .unwrap();
        let order = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(order, ["one", "three", "two"]);
        drop(statement);
        assert!(move_queued(&mut db, "running", "up").is_err());
        assert!(move_queued(&mut db, "one", "sideways").is_err());
    }

    #[test]
    fn migrates_existing_transfer_order_without_losing_rows() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE transfers (id TEXT PRIMARY KEY, status TEXT);
             INSERT INTO transfers VALUES ('one', 'queued');
             INSERT INTO transfers VALUES ('two', 'queued');",
        )
        .unwrap();
        migrate_queue_order(&db).unwrap();
        migrate_queue_order(&db).unwrap();
        let order = db
            .prepare("SELECT queue_order FROM transfers ORDER BY rowid")
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(order, [1, 2]);
    }

    #[cfg(unix)]
    #[test]
    fn plans_recursive_folder_sync_without_copying_links() {
        let root = std::env::temp_dir().join(format!("weave-sync-{}", uuid::Uuid::new_v4()));
        let left = root.join("left");
        let right = root.join("right");
        fs::create_dir_all(left.join("existing")).unwrap();
        fs::create_dir_all(left.join("new-folder")).unwrap();
        fs::create_dir_all(right.join("existing")).unwrap();
        fs::write(left.join("new.txt"), "new").unwrap();
        fs::write(left.join("new-folder/child.txt"), "child").unwrap();
        fs::write(left.join("existing/child.txt"), "child").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(left.join("new.txt"), left.join("link.txt")).unwrap();

        let mut plan = SyncPlan::default();
        let mut visited = 0;
        let mut context = SyncContext {
            source_profile: &None,
            destination_profile: &None,
            plan: &mut plan,
            visited: &mut visited,
            mirror: false,
            exclude: "",
        };
        plan_one_way(
            &mut Backend::Local,
            &mut Backend::Local,
            &left.to_string_lossy(),
            &right.to_string_lossy(),
            &mut context,
        )
        .unwrap();
        assert_eq!(plan.copies.len(), 3);
        assert!(plan
            .copies
            .iter()
            .any(|copy| copy.source_path.ends_with("new-folder")));
        assert!(plan
            .copies
            .iter()
            .any(|copy| copy.source_path.ends_with("existing/child.txt")));
        #[cfg(unix)]
        assert!(plan.issues.iter().any(|issue| issue.contains("link.txt")));

        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER, total_bytes INTEGER, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        for copy in &plan.copies {
            let id = uuid::Uuid::new_v4().to_string();
            app.state::<AppState>()
                .db
                .lock()
                .unwrap()
                .execute("INSERT INTO transfers (id, source_path, destination_dir, conflict, status, bytes_done, total_bytes, created_at, updated_at) VALUES (?1, ?2, ?3, 'newer', 'queued', 0, 0, 0, 0)", params![id, copy.source_path, copy.destination_dir])
                .unwrap();
            assert_eq!(
                run_transfer(app.handle(), &id, &AtomicBool::new(true)).unwrap(),
                "done"
            );
        }
        assert_eq!(fs::read_to_string(right.join("new.txt")).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(right.join("new-folder/child.txt")).unwrap(),
            "child"
        );
        assert_eq!(
            fs::read_to_string(right.join("existing/child.txt")).unwrap(),
            "child"
        );
        assert!(!right.join("link.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn mirror_cleanup_requires_current_preview_and_no_pending_copies() {
        let root = std::env::temp_dir().join(format!("weave-mirror-{}", uuid::Uuid::new_v4()));
        let left = root.join("left");
        let right = root.join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(right.join("old-folder")).unwrap();
        fs::write(right.join("old.txt"), "old").unwrap();
        fs::write(right.join("old-folder/child.txt"), "old").unwrap();
        fs::write(left.join("new.txt"), "new").unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(Connection::open_in_memory().unwrap()),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        let left_path = left.to_string_lossy().to_string();
        let right_path = right.to_string_lossy().to_string();
        let preview = plan_sync(
            &app.state::<AppState>(),
            &None,
            &left_path,
            &None,
            &right_path,
            "mirror-left-to-right",
        )
        .unwrap();
        assert_eq!(preview.copies.len(), 1);
        assert_eq!(preview.deletes.len(), 2);
        let expected = preview
            .deletes
            .iter()
            .map(|item| item.path.clone())
            .collect::<Vec<_>>();
        assert!(mirror_delete(
            app.handle(),
            &None,
            &left_path,
            &None,
            &right_path,
            "mirror-left-to-right",
            expected.clone(),
            "",
        )
        .is_err());
        assert!(right.join("old.txt").exists());
        fs::copy(left.join("new.txt"), right.join("new.txt")).unwrap();
        assert!(mirror_delete(
            app.handle(),
            &None,
            &left_path,
            &None,
            &right_path,
            "mirror-left-to-right",
            vec![right.join("old.txt").to_string_lossy().to_string()],
            "",
        )
        .is_err());
        let removed = mirror_delete(
            app.handle(),
            &None,
            &left_path,
            &None,
            &right_path,
            "mirror-left-to-right",
            expected,
            "",
        )
        .unwrap();
        assert_eq!(removed, 2);
        assert!(!right.join("old.txt").exists());
        assert!(!right.join("old-folder").exists());
        assert!(right.join("new.txt").exists());
        fs::OpenOptions::new()
            .write(true)
            .open(left.join("new.txt"))
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1_600_000_000)),
            )
            .unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(right.join("new.txt"))
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1_600_000_100)),
            )
            .unwrap();
        let two_way = plan_sync(
            &app.state::<AppState>(),
            &None,
            &left.to_string_lossy(),
            &None,
            &right.to_string_lossy(),
            "two-way",
        )
        .unwrap();
        assert!(two_way.copies.is_empty());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(left.join("new.txt"), right.join("linked.txt")).unwrap();
            let preview = plan_sync(
                &app.state::<AppState>(),
                &None,
                &left.to_string_lossy(),
                &None,
                &right.to_string_lossy(),
                "mirror-left-to-right",
            )
            .unwrap();
            assert!(preview.deletes.is_empty());
            assert!(preview
                .issues
                .iter()
                .any(|issue| issue.contains("linked.txt")));
            assert!(right.join("linked.txt").exists());
            let alias = root.join("left-alias");
            std::os::unix::fs::symlink(&left, &alias).unwrap();
            assert!(plan_sync(
                &app.state::<AppState>(),
                &None,
                &left.to_string_lossy(),
                &None,
                &alias.to_string_lossy(),
                "mirror-left-to-right",
            )
            .is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copies_and_replaces_file_without_losing_old_copy() {
        let root = std::env::temp_dir().join(format!("weave-transfer-{}", uuid::Uuid::new_v4()));
        let source_dir = root.join("source");
        let destination_dir = root.join("destination");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&destination_dir).unwrap();
        fs::write(source_dir.join("data.txt"), "new content").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(source_dir.join("data.txt"))
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1_600_000_000)),
            )
            .unwrap();
        fs::write(destination_dir.join("data.txt"), "old content").unwrap();

        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER, total_bytes INTEGER, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER);").unwrap();
        db.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        db.execute("INSERT INTO transfers (id, source_profile, source_path, destination_profile, destination_dir, target_path, conflict, status, bytes_done, total_bytes, error, created_at, updated_at) VALUES (?1, NULL, ?2, NULL, ?3, NULL, 'overwrite', 'queued', 0, 0, NULL, 0, 0)",
            params![id, source_dir.join("data.txt").to_string_lossy(), destination_dir.to_string_lossy()]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });

        let outcome = run_transfer(app.handle(), &id, &AtomicBool::new(true)).unwrap();
        assert_eq!(outcome, "done");
        assert_eq!(
            fs::read_to_string(destination_dir.join("data.txt")).unwrap(),
            "new content"
        );
        assert_eq!(fs::read_dir(&destination_dir).unwrap().count(), 1);
        assert_eq!(
            fs::metadata(destination_dir.join("data.txt"))
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            1_600_000_000,
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resumes_partial_file() {
        let root = std::env::temp_dir().join(format!("weave-transfer-{}", uuid::Uuid::new_v4()));
        let source_dir = root.join("source");
        let destination_dir = root.join("destination");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&destination_dir).unwrap();
        fs::write(source_dir.join("data.txt"), "resume this content").unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        fs::write(destination_dir.join(format!(".weave-part-{id}")), "resume ").unwrap();

        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER, total_bytes INTEGER, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER);").unwrap();
        db.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        db.execute("INSERT INTO transfers (id, source_profile, source_path, destination_profile, destination_dir, target_path, conflict, status, bytes_done, total_bytes, error, created_at, updated_at) VALUES (?1, NULL, ?2, NULL, ?3, ?4, 'overwrite', 'paused', 7, 19, NULL, 0, 0)",
            params![id, source_dir.join("data.txt").to_string_lossy(), destination_dir.to_string_lossy(), destination_dir.join("data.txt").to_string_lossy()]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });

        run_transfer(app.handle(), &id, &AtomicBool::new(true)).unwrap();
        assert_eq!(
            fs::read_to_string(destination_dir.join("data.txt")).unwrap(),
            "resume this content"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn relays_between_two_sftp_endpoints() {
        let server = "/usr/libexec/sftp-server";
        if !Path::new(server).exists() {
            return;
        }
        let root = std::env::temp_dir().join(format!("weave-transfer-{}", uuid::Uuid::new_v4()));
        let source_dir = root.join("source");
        let destination_dir = root.join("destination");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&destination_dir).unwrap();
        let contents = vec![b'x'; 150_000];
        fs::write(source_dir.join("data.bin"), &contents).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(source_dir.join("data.bin"))
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1_600_000_000)),
            )
            .unwrap();
        fs::write(destination_dir.join("data.bin"), &contents[..50_000]).unwrap();
        let source_client = SftpClient::from_command(crate::platform::command(server)).unwrap();
        let destination_client =
            SftpClient::from_command(crate::platform::command(server)).unwrap();
        let mut source = Backend::Remote(source_client);
        let mut destination = Backend::Remote(destination_client);
        let source_item = source
            .stat(&source_dir.join("data.bin").to_string_lossy())
            .unwrap();
        let target = destination_dir
            .join("data.bin")
            .to_string_lossy()
            .to_string();

        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER, total_bytes INTEGER, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER);").unwrap();
        db.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        db.execute("INSERT INTO transfers (id, source_profile, source_path, destination_profile, destination_dir, target_path, conflict, status, bytes_done, total_bytes, error, created_at, updated_at) VALUES (?1, NULL, ?2, NULL, ?3, ?4, 'overwrite', 'running', 0, 150000, NULL, 0, 0)",
            params![id, source_item.path, destination_dir.to_string_lossy(), target]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        let mut done = 0;
        copy_item(
            app.handle(),
            &id,
            &AtomicBool::new(true),
            &mut source,
            &mut destination,
            false,
            &source_item,
            &target,
            contents.len() as u64,
            &mut done,
            &mut Instant::now(),
            &mut RateLimit::new(0),
        )
        .unwrap();
        assert_eq!(done, contents.len() as u64);
        assert_eq!(
            fs::read(destination_dir.join("data.bin")).unwrap(),
            contents
        );
        assert_eq!(
            destination.stat(&target).unwrap().modified,
            Some(1_600_000_000)
        );
        if matches!(&destination, Backend::Remote(client) if client.supports_copy_data()) {
            let server_copy = destination_dir.join("server-copy.bin");
            let server_copy_path = server_copy.to_string_lossy().to_string();
            done = 0;
            copy_item(
                app.handle(),
                &id,
                &AtomicBool::new(true),
                &mut source,
                &mut destination,
                true,
                &source_item,
                &server_copy_path,
                contents.len() as u64,
                &mut done,
                &mut Instant::now(),
                &mut RateLimit::new(0),
            )
            .unwrap();
            assert_eq!(done, contents.len() as u64);
            assert_eq!(fs::read(server_copy).unwrap(), contents);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copies_nested_folder_and_preserves_empty_folders() {
        let root = std::env::temp_dir().join(format!("weave-transfer-{}", uuid::Uuid::new_v4()));
        let source_dir = root.join("source");
        let destination_dir = root.join("destination");
        fs::create_dir_all(source_dir.join("nested/empty")).unwrap();
        fs::create_dir_all(&destination_dir).unwrap();
        fs::write(source_dir.join("nested/data.txt"), "nested content").unwrap();
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER, total_bytes INTEGER, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER);").unwrap();
        db.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        db.execute("INSERT INTO transfers (id, source_profile, source_path, destination_profile, destination_dir, target_path, conflict, status, bytes_done, total_bytes, error, created_at, updated_at) VALUES (?1, NULL, ?2, NULL, ?3, NULL, 'overwrite', 'queued', 0, 0, NULL, 0, 0)",
            params![id, source_dir.to_string_lossy(), destination_dir.to_string_lossy()]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        run_transfer(app.handle(), &id, &AtomicBool::new(true)).unwrap();
        assert_eq!(
            fs::read_to_string(destination_dir.join("source/nested/data.txt")).unwrap(),
            "nested content"
        );
        assert!(destination_dir.join("source/nested/empty").is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_verification_keeps_existing_target() {
        let root = std::env::temp_dir().join(format!("weave-transfer-{}", uuid::Uuid::new_v4()));
        let source_dir = root.join("source");
        let destination_dir = root.join("destination");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&destination_dir).unwrap();
        fs::write(source_dir.join("data.txt"), "correct content").unwrap();
        fs::write(destination_dir.join("data.txt"), "old content").unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        fs::write(
            destination_dir.join(format!(".weave-part-{id}")),
            "broken content!",
        )
        .unwrap();
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER, total_bytes INTEGER, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER);").unwrap();
        db.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        db.execute("INSERT INTO transfers (id, source_profile, source_path, destination_profile, destination_dir, target_path, conflict, status, bytes_done, total_bytes, error, created_at, updated_at) VALUES (?1, NULL, ?2, NULL, ?3, NULL, 'overwrite', 'paused', 15, 15, NULL, 0, 0)",
            params![id, source_dir.join("data.txt").to_string_lossy(), destination_dir.to_string_lossy()]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        assert!(run_transfer(app.handle(), &id, &AtomicBool::new(true))
            .unwrap_err()
            .contains("Checksum verification failed"));
        assert_eq!(
            fs::read_to_string(destination_dir.join("data.txt")).unwrap(),
            "old content"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queued_transfers_start_when_a_worker_becomes_available() {
        let root = std::env::temp_dir().join(format!("weave-transfer-{}", uuid::Uuid::new_v4()));
        let source_dir = root.join("source");
        let destination_dir = root.join("destination");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&destination_dir).unwrap();
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER, total_bytes INTEGER, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO settings VALUES ('transfer.parallelism', '1');").unwrap();
        let ids = [
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
        ];
        for (index, id) in ids.iter().enumerate() {
            let name = format!("file-{index}.txt");
            fs::write(source_dir.join(&name), format!("contents-{index}")).unwrap();
            db.execute("INSERT INTO transfers (id, source_profile, source_path, destination_profile, destination_dir, target_path, conflict, status, bytes_done, total_bytes, error, created_at, updated_at) VALUES (?1, NULL, ?2, NULL, ?3, NULL, 'overwrite', 'queued', 0, 0, NULL, 0, 0)",
                params![id, source_dir.join(name).to_string_lossy(), destination_dir.to_string_lossy()]).unwrap();
        }
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        let state = app.state::<AppState>();
        state
            .transfers
            .lock()
            .unwrap()
            .insert("busy".into(), Arc::new(AtomicBool::new(true)));
        pump_queue(app.handle()).unwrap();
        assert!(ids
            .iter()
            .all(|id| read_transfer(&state, id).unwrap().status == "queued"));
        state.transfers.lock().unwrap().remove("busy");
        pump_queue(app.handle()).unwrap();
        for _ in 0..100 {
            if ids
                .iter()
                .all(|id| read_transfer(&state, id).unwrap().status == "done")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(ids
            .iter()
            .all(|id| read_transfer(&state, id).unwrap().status == "done"));
        assert_eq!(
            fs::read_to_string(destination_dir.join("file-1.txt")).unwrap(),
            "contents-1"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
