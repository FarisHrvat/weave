use crate::{transfer, AppState};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, Runtime, State};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncJob {
    id: String,
    name: String,
    left_profile: Option<String>,
    left_path: String,
    right_profile: Option<String>,
    right_path: String,
    mode: String,
    interval_minutes: i64,
    enabled: bool,
    running: bool,
    last_run: Option<i64>,
    next_run: i64,
    last_queued: i64,
    last_error: Option<String>,
    watch_local: bool,
    last_fingerprint: Option<String>,
    exclude: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncJobDraft {
    name: String,
    left_profile: Option<String>,
    left_path: String,
    right_profile: Option<String>,
    right_path: String,
    mode: String,
    interval_minutes: i64,
    watch_local: bool,
    #[serde(default)]
    exclude: String,
}

pub fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_jobs (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            left_profile TEXT,
            left_path TEXT NOT NULL,
            right_profile TEXT,
            right_path TEXT NOT NULL,
            mode TEXT NOT NULL,
            interval_minutes INTEGER NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            running INTEGER NOT NULL DEFAULT 0,
            last_run INTEGER,
            next_run INTEGER NOT NULL,
            last_queued INTEGER NOT NULL DEFAULT 0,
            last_error TEXT
        );
        CREATE TABLE IF NOT EXISTS sync_job_transfers (
            job_id TEXT NOT NULL,
            transfer_id TEXT NOT NULL UNIQUE,
            PRIMARY KEY (job_id, transfer_id)
        );
        UPDATE sync_jobs SET running=0 WHERE running=1;",
    )?;
    let mut columns = db.prepare("PRAGMA table_info(sync_jobs)")?;
    let names = columns
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !names.iter().any(|name| name == "watch_local") {
        db.execute(
            "ALTER TABLE sync_jobs ADD COLUMN watch_local INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !names.iter().any(|name| name == "last_fingerprint") {
        db.execute("ALTER TABLE sync_jobs ADD COLUMN last_fingerprint TEXT", [])?;
    }
    if !names.iter().any(|name| name == "exclude") {
        db.execute(
            "ALTER TABLE sync_jobs ADD COLUMN exclude TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    Ok(())
}

fn read_job(db: &Connection, id: &str) -> Result<SyncJob, String> {
    db.query_row(
        "SELECT id, name, left_profile, left_path, right_profile, right_path, mode,
                interval_minutes, enabled, running, last_run, next_run, last_queued, last_error,
                watch_local, last_fingerprint, exclude
         FROM sync_jobs WHERE id=?1",
        [id],
        |row| {
            Ok(SyncJob {
                id: row.get(0)?,
                name: row.get(1)?,
                left_profile: row.get(2)?,
                left_path: row.get(3)?,
                right_profile: row.get(4)?,
                right_path: row.get(5)?,
                mode: row.get(6)?,
                interval_minutes: row.get(7)?,
                enabled: row.get(8)?,
                running: row.get(9)?,
                last_run: row.get(10)?,
                next_run: row.get(11)?,
                last_queued: row.get(12)?,
                last_error: row.get(13)?,
                watch_local: row.get(14)?,
                last_fingerprint: row.get(15)?,
                exclude: row.get(16)?,
            })
        },
    )
    .optional()
    .map_err(|error| format!("Could not load sync job: {error}"))?
    .ok_or_else(|| "This sync job no longer exists.".into())
}

fn list_jobs(db: &Connection) -> Result<Vec<SyncJob>, String> {
    let mut statement = db
        .prepare("SELECT id FROM sync_jobs ORDER BY name COLLATE NOCASE, id")
        .map_err(|error| format!("Could not list sync jobs: {error}"))?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Could not list sync jobs: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("Could not read sync jobs: {error}"))?;
    ids.into_iter().map(|id| read_job(db, &id)).collect()
}

fn reconcile_finished_transfers(db: &Connection) -> Result<(), String> {
    let mut statement = db
        .prepare(
            "SELECT tracked.job_id, transfer.source_path, transfer.error
             FROM sync_job_transfers tracked
             JOIN transfers transfer ON transfer.id=tracked.transfer_id
             WHERE transfer.status='failed'
             ORDER BY transfer.updated_at, transfer.rowid",
        )
        .map_err(|error| format!("Could not inspect sync transfer results: {error}"))?;
    let failures = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|error| format!("Could not read sync transfer results: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("Could not read sync transfer results: {error}"))?;
    drop(statement);

    let mut by_job: HashMap<String, (usize, String)> = HashMap::new();
    for (job_id, path, error) in failures {
        let detail = error
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                "The transfer stopped without an error message. Check Activity for details.".into()
            });
        let entry = by_job.entry(job_id).or_insert((0, String::new()));
        entry.0 += 1;
        entry.1 = format!("{}: {detail}", path);
    }

    let transaction = db
        .unchecked_transaction()
        .map_err(|error| format!("Could not update sync transfer results: {error}"))?;
    for (job_id, (count, latest)) in by_job {
        transaction
            .execute(
                "UPDATE sync_jobs SET last_error=?2 WHERE id=?1",
                params![job_id, format!("{count} sync transfer(s) failed. {latest}")],
            )
            .map_err(|error| format!("Could not record sync transfer failure: {error}"))?;
    }
    transaction
        .execute(
            "DELETE FROM sync_job_transfers WHERE transfer_id NOT IN
             (SELECT id FROM transfers WHERE status IN ('queued', 'running', 'paused'))",
            [],
        )
        .map_err(|error| format!("Could not clear finished sync transfers: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("Could not save sync transfer results: {error}"))
}

/// The source side of a one-way job: `(profile, folder)`.
fn watched_source(job: &SyncJob) -> Option<(&Option<String>, &str)> {
    source_of(
        &job.mode,
        &job.left_profile,
        &job.left_path,
        &job.right_profile,
        &job.right_path,
    )
}

fn source_of<'a>(
    mode: &str,
    left_profile: &'a Option<String>,
    left_path: &'a str,
    right_profile: &'a Option<String>,
    right_path: &'a str,
) -> Option<(&'a Option<String>, &'a str)> {
    match mode {
        "left-to-right" | "mirror-left-to-right" => Some((left_profile, left_path)),
        "right-to-left" | "mirror-right-to-left" => Some((right_profile, right_path)),
        _ => None,
    }
}

fn source_fingerprint(
    state: &AppState,
    profile: &Option<String>,
    folder: &str,
) -> Result<String, String> {
    match profile {
        None => fingerprint(folder),
        Some(_) => transfer::folder_fingerprint(state, profile, folder),
    }
}

fn fingerprint(folder: &str) -> Result<String, String> {
    let root = Path::new(folder);
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("Could not watch folder {folder}: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "The watched source is no longer a regular folder: {folder}"
        ));
    }
    let mut digest = Sha256::new();
    let mut stack = vec![root.to_path_buf()];
    let mut visited = 0;
    while let Some(directory) = stack.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| format!("Could not watch {}: {error}", directory.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Could not read a watched folder: {error}"))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            visited += 1;
            if visited > 20_000 {
                return Err("This watched folder has more than 20,000 items. Choose a smaller source folder.".into());
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "Could not read watched path.")?;
            digest.update(relative.to_string_lossy().as_bytes());
            digest.update([0]);
            digest.update([
                metadata.is_dir() as u8,
                metadata.file_type().is_symlink() as u8,
            ]);
            if !metadata.is_dir() {
                digest.update(metadata.len().to_le_bytes());
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok());
                digest.update(modified.map_or(0, |time| time.as_nanos()).to_le_bytes());
            }
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                stack.push(path);
            }
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[tauri::command]
pub fn sync_jobs_list(state: State<'_, AppState>) -> Result<Vec<SyncJob>, String> {
    let db = state.db.lock().map_err(|_| "Sync job database is busy")?;
    reconcile_finished_transfers(&db)?;
    list_jobs(&db)
}

#[tauri::command]
pub async fn sync_job_save(app: AppHandle, job: SyncJobDraft) -> Result<SyncJob, String> {
    let name = job.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 100 {
        return Err("Enter a sync job name of 1–100 characters.".into());
    }
    if !(15..=10_080).contains(&job.interval_minutes) {
        return Err("Choose a sync interval between 15 minutes and 7 days.".into());
    }
    if ![
        "left-to-right",
        "right-to-left",
        "two-way",
        "mirror-left-to-right",
        "mirror-right-to-left",
    ]
    .contains(&job.mode.as_str())
    {
        return Err("Choose a copy direction, two-way, or a mirror.".into());
    }
    if job.watch_local && job.mode == "two-way" {
        return Err("Change checks work with one-way jobs. Choose a copy direction.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let plan = transfer::plan_sync_excluding(
            &state,
            &job.left_profile,
            &job.left_path,
            &job.right_profile,
            &job.right_path,
            &job.mode,
            &job.exclude,
        )?;
        if !plan.issues.is_empty() {
            return Err(format!(
                "This folder pair has {} item(s) needing manual review. Resolve them in Compare folders before saving a job.",
                plan.issues.len()
            ));
        }
        let last_fingerprint = match source_of(
            &job.mode,
            &job.left_profile,
            &job.left_path,
            &job.right_profile,
            &job.right_path,
        ) {
            Some((profile, folder)) if job.watch_local => {
                Some(source_fingerprint(&state, profile, folder)?)
            }
            _ => None,
        };
        let id = uuid::Uuid::new_v4().to_string();
        let db = state.db.lock().map_err(|_| "Sync job database is busy")?;
        db.execute(
            "INSERT INTO sync_jobs
             (id, name, left_profile, left_path, right_profile, right_path, mode, interval_minutes, next_run, watch_local, last_fingerprint, exclude)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch() + ?8 * 60, ?9, ?10, ?11)",
            params![id, name, job.left_profile, job.left_path, job.right_profile, job.right_path, job.mode, job.interval_minutes, job.watch_local, last_fingerprint, job.exclude.trim()],
        )
        .map_err(|error| format!("Could not save sync job: {error}"))?;
        read_job(&db, &id)
    })
    .await
    .map_err(|error| format!("Sync job setup stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub fn sync_job_set_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<SyncJob, String> {
    let db = state.db.lock().map_err(|_| "Sync job database is busy")?;
    let count = db
        .execute(
            "UPDATE sync_jobs SET enabled=?2, next_run=CASE WHEN ?2=1 THEN unixepoch() + interval_minutes * 60 ELSE next_run END WHERE id=?1",
            params![id, enabled],
        )
        .map_err(|error| format!("Could not update sync job: {error}"))?;
    if count == 0 {
        return Err("This sync job no longer exists.".into());
    }
    read_job(&db, &id)
}

#[tauri::command]
pub fn sync_job_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "Sync job database is busy")?;
    let count = db
        .execute(
            "DELETE FROM sync_jobs WHERE id=?1 AND running=0",
            [id.as_str()],
        )
        .map_err(|error| format!("Could not delete sync job: {error}"))?;
    if count == 0 {
        return Err(
            "This sync job is running or no longer exists. Wait for it to finish and try again."
                .into(),
        );
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum RunReason {
    Manual,
    Scheduled,
    Watched,
}

impl RunReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Scheduled => "scheduled",
            Self::Watched => "watched",
        }
    }
}

fn claim_job(db: &Connection, id: &str, reason: RunReason) -> Result<SyncJob, String> {
    let count = db
        .execute(
            "UPDATE sync_jobs SET running=1 WHERE id=?1 AND running=0
             AND (?2='manual' OR (enabled=1 AND ((?2='watched' AND watch_local=1) OR next_run<=unixepoch())))",
            params![id, reason.as_str()],
        )
        .map_err(|error| format!("Could not start sync job: {error}"))?;
    if count == 0 {
        return Err("This sync job is already running, disabled, or not due yet.".into());
    }
    read_job(db, id)
}

fn finish_job(
    db: &Connection,
    id: &str,
    queued: usize,
    error: Option<&str>,
    fingerprint: Option<&str>,
) -> Result<SyncJob, String> {
    db.execute(
        "UPDATE sync_jobs SET running=0, last_run=unixepoch(),
         next_run=unixepoch() + interval_minutes * 60,
         last_queued=?2, last_error=?3,
         last_fingerprint=COALESCE(?4, last_fingerprint) WHERE id=?1",
        params![id, queued as i64, error, fingerprint],
    )
    .map_err(|cause| format!("Could not save sync job result: {cause}"))?;
    read_job(db, id)
}

/// Most items a mirror deletes in one unattended run.
const MAX_UNATTENDED_DELETES: usize = 1000;

/// Refuses deletions that look like a mistake rather than a change.
fn check_unattended_delete(state: &AppState, job: &SyncJob, count: usize) -> Result<(), String> {
    if count > MAX_UNATTENDED_DELETES {
        return Err(format!(
            "The mirror would delete {count} items, more than {MAX_UNATTENDED_DELETES} at once. Review them in Compare folders."
        ));
    }
    let (profile, folder) = watched_source(job).ok_or("Only mirror jobs delete items.")?;
    if transfer::folder_is_empty(state, profile, folder)? {
        return Err("The source folder is empty, so nothing was deleted. Check that it is mounted or connected.".into());
    }
    Ok(())
}

fn run_job<R: Runtime>(app: &AppHandle<R>, id: &str, reason: RunReason) -> Result<SyncJob, String> {
    let state = app.state::<AppState>();
    let job = {
        let db = state.db.lock().map_err(|_| "Sync job database is busy")?;
        reconcile_finished_transfers(&db)?;
        claim_job(&db, id, reason)?
    };
    let mut queued = 0;
    let mut seen_fingerprint = None;
    let result = (|| {
        if job.watch_local {
            let (profile, folder) =
                watched_source(&job).ok_or("Change checks work with one-way jobs only.")?;
            seen_fingerprint = Some(source_fingerprint(&state, profile, folder)?);
        }
        {
            let db = state.db.lock().map_err(|_| "Sync job database is busy")?;
            let pending: i64 = db
                .query_row(
                    "SELECT COUNT(*) FROM sync_job_transfers WHERE job_id=?1",
                    [id],
                    |row| row.get(0),
                )
                .map_err(|error| format!("Could not check earlier sync transfers: {error}"))?;
            if pending > 0 {
                seen_fingerprint = None;
                return Err(format!(
                    "{pending} transfer(s) from this job are still queued, running, or paused. Complete them in Activity before another comparison."
                ));
            }
        }
        let plan = transfer::plan_sync_excluding(
            &state,
            &job.left_profile,
            &job.left_path,
            &job.right_profile,
            &job.right_path,
            &job.mode,
            &job.exclude,
        )?;
        if !plan.issues.is_empty() {
            return Err(format!(
                "No transfers queued: {} item(s) need review. Open Compare folders to inspect them.",
                plan.issues.len()
            ));
        }
        // Mirrors delete extras only once everything is copied, on a later run.
        if plan.copies.is_empty() && !plan.deletes.is_empty() {
            check_unattended_delete(&state, &job, plan.deletes.len())?;
            let expected = plan.deletes.iter().map(|item| item.path.clone()).collect();
            transfer::mirror_delete(
                app,
                &job.left_profile,
                &job.left_path,
                &job.right_profile,
                &job.right_path,
                &job.mode,
                expected,
                &job.exclude,
            )?;
            return Ok(());
        }
        for copy in plan.copies {
            let record = transfer::enqueue_transfer(
                app,
                &state,
                copy.source_profile,
                copy.source_path,
                copy.destination_profile,
                copy.destination_dir,
                "newer".into(),
            )?;
            let db = state.db.lock().map_err(|_| "Sync job database is busy")?;
            db.execute(
                "INSERT INTO sync_job_transfers (job_id, transfer_id) VALUES (?1, ?2)",
                params![id, record.id],
            )
            .map_err(|error| format!("Could not track the queued sync transfer: {error}"))?;
            queued += 1;
        }
        Ok::<(), String>(())
    })();
    let db = state.db.lock().map_err(|_| "Sync job database is busy")?;
    let saved = finish_job(
        &db,
        id,
        queued,
        result.as_ref().err().map(String::as_str),
        seen_fingerprint.as_deref(),
    )?;
    result.map(|()| saved)
}

#[tauri::command]
pub async fn sync_job_run(app: AppHandle, id: String) -> Result<SyncJob, String> {
    tauri::async_runtime::spawn_blocking(move || run_job(&app, &id, RunReason::Manual))
        .await
        .map_err(|error| format!("Sync job stopped unexpectedly: {error}"))?
}

/// Remote sources are listed at most this often to keep load on servers low.
const REMOTE_CHECK_INTERVAL: Duration = Duration::from_secs(120);

/// Jobs that should run now: scheduled ones that are due, and watched ones
/// whose source changed.
fn due_jobs<R: Runtime>(
    app: &AppHandle<R>,
    remote_checks: &mut HashMap<String, std::time::Instant>,
) -> Vec<(String, RunReason)> {
    let mut due = Vec::new();
    {
        let jobs = {
            let state = app.state::<AppState>();
            state
                .db
                .lock()
                .map_err(|_| "Sync job database is busy".to_string())
                .and_then(|db| {
                    reconcile_finished_transfers(&db)?;
                    list_jobs(&db)
                })
        };
        if let Ok(jobs) = jobs {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs() as i64);
            for job in jobs.into_iter().filter(|job| job.enabled && !job.running) {
                let reason = if job.next_run <= now {
                    Some(RunReason::Scheduled)
                } else if job.watch_local
                    && watched_source(&job).is_some_and(|(profile, folder)| {
                        if profile.is_some() {
                            let due = remote_checks
                                .get(&job.id)
                                .is_none_or(|last| last.elapsed() >= REMOTE_CHECK_INTERVAL);
                            if !due {
                                return false;
                            }
                            remote_checks.insert(job.id.clone(), std::time::Instant::now());
                        }
                        let state = app.state::<AppState>();
                        source_fingerprint(&state, profile, folder)
                            .is_ok_and(|current| Some(current) != job.last_fingerprint)
                    })
                {
                    Some(RunReason::Watched)
                } else {
                    None
                };
                if let Some(reason) = reason {
                    if matches!(reason, RunReason::Watched) {
                        let state = app.state::<AppState>();
                        let waiting = state.db.lock().ok().and_then(|db| {
                            db.query_row(
                                "SELECT COUNT(*) FROM sync_job_transfers tracked
                                 JOIN transfers transfer ON transfer.id=tracked.transfer_id
                                 WHERE tracked.job_id=?1 AND transfer.status IN ('queued', 'running', 'paused')",
                                [job.id.as_str()],
                                |row| row.get::<_, i64>(0),
                            )
                            .ok()
                        });
                        if waiting != Some(0) {
                            continue;
                        }
                    }
                    due.push((job.id.clone(), reason));
                }
            }
        }
    }
    due
}

pub fn start_scheduler<R: Runtime>(app: AppHandle<R>) {
    let mut remote_checks = HashMap::new();
    thread::spawn(move || loop {
        for (id, reason) in due_jobs(&app, &mut remote_checks) {
            let app = app.clone();
            thread::spawn(move || {
                let _ = run_job(&app, &id, reason);
            });
        }
        thread::sleep(Duration::from_secs(30));
    });
}

/// For the sync agent: runs what is due, waits for the copies, then quits.
pub fn run_due_and_exit<R: Runtime>(app: AppHandle<R>) {
    thread::spawn(move || {
        for (id, reason) in due_jobs(&app, &mut HashMap::new()) {
            let _ = run_job(&app, &id, reason);
        }
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(6 * 3600) {
            let state = app.state::<AppState>();
            let busy = state
                .transfers
                .lock()
                .map_or(true, |active| !active.is_empty())
                || state.db.lock().map_or(true, |db| {
                    db.query_row(
                        "SELECT COUNT(*) FROM transfers WHERE status IN ('queued', 'running')",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_or(true, |count| count > 0)
                });
            if !busy {
                break;
            }
            thread::sleep(Duration::from_secs(5));
        }
        if let Ok(db) = app.state::<AppState>().db.lock() {
            let _ = reconcile_finished_transfers(&db);
        }
        app.exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, fs, sync::Mutex};

    #[test]
    fn claiming_and_finishing_jobs_prevents_overlap() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        db.execute("INSERT INTO sync_jobs (id, name, left_path, right_path, mode, interval_minutes, next_run) VALUES ('one', 'One', '/tmp/a', '/tmp/b', 'left-to-right', 15, unixepoch()-1)", []).unwrap();
        assert!(claim_job(&db, "one", RunReason::Scheduled).unwrap().running);
        assert!(claim_job(&db, "one", RunReason::Manual).is_err());
        let finished = finish_job(&db, "one", 2, Some("Connection failed"), None).unwrap();
        assert!(!finished.running);
        assert_eq!(finished.last_queued, 2);
        assert_eq!(finished.last_error.as_deref(), Some("Connection failed"));
        assert!(claim_job(&db, "one", RunReason::Scheduled).is_err());
        assert!(claim_job(&db, "one", RunReason::Manual).is_ok());
    }

    #[test]
    fn completed_sync_failures_remain_visible_on_the_job() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        db.execute_batch(
            "CREATE TABLE transfers (
                id TEXT PRIMARY KEY, source_path TEXT NOT NULL, status TEXT NOT NULL,
                error TEXT, updated_at INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO sync_jobs (id, name, left_path, right_path, mode, interval_minutes, next_run)
            VALUES ('job', 'Backup', '/source', '/target', 'left-to-right', 15, unixepoch()+900);
            INSERT INTO transfers (id, source_path, status, error) VALUES
                ('done', '/source/ok', 'done', NULL),
                ('failed', '/source/lost', 'failed', 'Disk is full'),
                ('paused', '/source/later', 'paused', NULL);
            INSERT INTO sync_job_transfers (job_id, transfer_id) VALUES
                ('job', 'done'), ('job', 'failed'), ('job', 'paused');",
        )
        .unwrap();

        reconcile_finished_transfers(&db).unwrap();
        let job = read_job(&db, "job").unwrap();
        assert_eq!(
            job.last_error.as_deref(),
            Some("1 sync transfer(s) failed. /source/lost: Disk is full")
        );
        let tracked: Vec<String> = db
            .prepare("SELECT transfer_id FROM sync_job_transfers")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(tracked, vec!["paused"]);
        reconcile_finished_transfers(&db).unwrap();
        assert_eq!(read_job(&db, "job").unwrap().last_error, job.last_error);
    }

    #[test]
    fn successful_sync_transfers_do_not_create_an_error() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        db.execute_batch(
            "CREATE TABLE transfers (
                id TEXT PRIMARY KEY, source_path TEXT NOT NULL, status TEXT NOT NULL,
                error TEXT, updated_at INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO sync_jobs (id, name, left_path, right_path, mode, interval_minutes, next_run)
            VALUES ('job', 'Backup', '/source', '/target', 'left-to-right', 15, unixepoch()+900);
            INSERT INTO transfers (id, source_path, status) VALUES ('done', '/source/ok', 'done');
            INSERT INTO sync_job_transfers (job_id, transfer_id) VALUES ('job', 'done');",
        )
        .unwrap();

        reconcile_finished_transfers(&db).unwrap();
        assert!(read_job(&db, "job").unwrap().last_error.is_none());
        let tracked: i64 = db
            .query_row("SELECT COUNT(*) FROM sync_job_transfers", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(tracked, 0);
    }

    #[test]
    fn migrates_existing_jobs_and_allows_a_watched_run_before_its_timer() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE sync_jobs (id TEXT PRIMARY KEY, name TEXT NOT NULL, left_profile TEXT, left_path TEXT NOT NULL, right_profile TEXT, right_path TEXT NOT NULL, mode TEXT NOT NULL, interval_minutes INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1, running INTEGER NOT NULL DEFAULT 0, last_run INTEGER, next_run INTEGER NOT NULL, last_queued INTEGER NOT NULL DEFAULT 0, last_error TEXT); INSERT INTO sync_jobs (id, name, left_path, right_path, mode, interval_minutes, next_run) VALUES ('old', 'Old', '/tmp/left', '/tmp/right', 'left-to-right', 60, unixepoch()+3600);").unwrap();
        migrate(&db).unwrap();
        let old = read_job(&db, "old").unwrap();
        assert!(!old.watch_local);
        assert!(old.last_fingerprint.is_none());
        assert!(claim_job(&db, "old", RunReason::Scheduled).is_err());
        assert!(claim_job(&db, "old", RunReason::Watched).is_err());
        db.execute("UPDATE sync_jobs SET watch_local=1 WHERE id='old'", [])
            .unwrap();
        assert!(claim_job(&db, "old", RunReason::Watched).is_ok());
    }

    #[test]
    fn local_job_queues_missing_file_and_records_result() {
        let root = std::env::temp_dir().join(format!("weave-sync-job-{}", uuid::Uuid::new_v4()));
        let left = root.join("left");
        let right = root.join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir(&right).unwrap();
        fs::write(left.join("photo.txt"), "from left").unwrap();

        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        db.execute_batch("CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, protocol TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT, color TEXT, favorite INTEGER, tls_pin_sha256 TEXT, s3_region TEXT); CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER DEFAULT 0, total_bytes INTEGER DEFAULT 0, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER DEFAULT (unixepoch()), updated_at INTEGER DEFAULT (unixepoch())); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        db.execute("INSERT INTO sync_jobs (id, name, left_path, right_path, mode, interval_minutes, next_run, watch_local, last_fingerprint) VALUES ('local', 'Local', ?1, ?2, 'left-to-right', 15, unixepoch()-1, 1, ?3)", params![left.to_string_lossy(), right.to_string_lossy(), fingerprint(left.to_str().unwrap()).unwrap()]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
        });
        let result = run_job(app.handle(), "local", RunReason::Scheduled).unwrap();
        assert_eq!(result.last_queued, 1);
        assert!(result.last_error.is_none());
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM transfers WHERE source_path=?1",
                [left.join("photo.txt").to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let previous_fingerprint: String = db
            .query_row(
                "SELECT last_fingerprint FROM sync_jobs WHERE id='local'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        db.execute("INSERT INTO transfers (id, source_path, destination_dir, conflict, status) VALUES ('paused-old', '/tmp/old', '/tmp', 'newer', 'paused')", []).unwrap();
        db.execute(
            "INSERT INTO sync_job_transfers (job_id, transfer_id) VALUES ('local', 'paused-old')",
            [],
        )
        .unwrap();
        drop(db);
        fs::write(left.join("later.txt"), "later").unwrap();
        assert!(run_job(app.handle(), "local", RunReason::Manual)
            .unwrap_err()
            .contains("still queued, running, or paused"));
        assert!(app
            .state::<AppState>()
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT last_error FROM sync_jobs WHERE id='local'",
                [],
                |row| row.get::<_, Option<String>>(0)
            )
            .unwrap()
            .unwrap()
            .contains("paused"));
        let stored_fingerprint: String = app
            .state::<AppState>()
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT last_fingerprint FROM sync_jobs WHERE id='local'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_fingerprint, previous_fingerprint);
        assert_ne!(
            stored_fingerprint,
            fingerprint(left.to_str().unwrap()).unwrap()
        );
        let mut completed = false;
        for _ in 0..200 {
            let status: String = app
                .state::<AppState>()
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT status FROM transfers WHERE source_path=?1",
                    [left.join("photo.txt").to_string_lossy().as_ref()],
                    |row| row.get(0),
                )
                .unwrap();
            if status == "done" {
                completed = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(completed, "local sync transfer did not finish");
        assert_eq!(
            fs::read_to_string(right.join("photo.txt")).unwrap(),
            "from left"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_fingerprint_detects_additions_changes_and_removals() {
        let root = std::env::temp_dir().join(format!("weave-watch-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let empty = fingerprint(root.to_str().unwrap()).unwrap();
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        let added = fingerprint(root.to_str().unwrap()).unwrap();
        assert_ne!(empty, added);
        fs::write(nested.join("item.txt"), "one").unwrap();
        let first = fingerprint(root.to_str().unwrap()).unwrap();
        assert_ne!(added, first);
        fs::write(nested.join("item.txt"), "another value").unwrap();
        let changed = fingerprint(root.to_str().unwrap()).unwrap();
        assert_ne!(first, changed);
        fs::remove_file(nested.join("item.txt")).unwrap();
        assert_eq!(fingerprint(root.to_str().unwrap()).unwrap(), added);
        fs::remove_dir_all(root).unwrap();
    }
}
