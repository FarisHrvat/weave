//! WinSCP-style scripts for file work: open a connection, move around,
//! copy with masks, sync folders. Copies go through the transfer queue so
//! they show progress and are retried like any other transfer.

use super::*;
use std::sync::Mutex;

pub(crate) fn split_line(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (Some(open), c) if c == open => quote = None,
            (Some('"'), '\\') | (None, '\\') => {
                current.push(chars.next().ok_or("A line ends with a lone backslash.")?);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (_, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("A quote is not closed.".into());
    }
    if started {
        words.push(current);
    }
    Ok(words)
}

/// The drive of a Windows path such as `C:/Users`, written as "C:".
fn drive(path: &str) -> Option<&str> {
    let bytes = path.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':').then(|| &path[..2])
}

/// Joins a path typed in a script onto the current folder of that side.
/// Paths use forward slashes; a Windows drive such as `C:/` counts as a root.
pub(crate) fn join(current: &str, path: &str) -> String {
    let combined = if path.starts_with('/') || drive(path).is_some() {
        path.to_string()
    } else {
        format!("{}/{path}", current.trim_end_matches('/'))
    };
    let root = drive(&combined).unwrap_or("");
    let mut parts: Vec<&str> = Vec::new();
    for part in combined[root.len()..].split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    format!("{root}/{}", parts.join("/"))
}

fn parent_and_name(path: &str) -> (String, String) {
    match path.rsplit_once('/') {
        Some(("", name)) => ("/".into(), name.into()),
        Some((parent, name)) if drive(parent) == Some(parent) => {
            (format!("{parent}/"), name.into())
        }
        Some((parent, name)) => (parent.into(), name.into()),
        None => ("/".into(), path.into()),
    }
}

fn expand(word: &str) -> String {
    let now = chrono::Local::now();
    word.replace("%DATE%", &now.format("%Y-%m-%d").to_string())
        .replace("%TIME%", &now.format("%H-%M-%S").to_string())
}

struct Side {
    profile: Option<String>,
    dir: String,
    backend: Option<Backend>,
}

impl Side {
    fn backend(&mut self, state: &AppState) -> Result<&mut Backend, String> {
        if self.backend.is_none() {
            self.backend = Some(open_backend(state, &self.profile)?);
        }
        Ok(self.backend.as_mut().expect("backend was just opened"))
    }
}

pub(crate) struct Run<'a, R: Runtime> {
    app: &'a AppHandle<R>,
    local: Side,
    remote: Option<Side>,
    conflict: String,
    keep_going: bool,
    stop: &'a AtomicBool,
    log: &'a mut dyn FnMut(&str),
}

impl<'a, R: Runtime> Run<'a, R> {
    pub(crate) fn new(
        app: &'a AppHandle<R>,
        local_dir: &str,
        stop: &'a AtomicBool,
        log: &'a mut dyn FnMut(&str),
    ) -> Self {
        Self {
            app,
            local: Side {
                profile: None,
                dir: local_dir.to_string(),
                backend: Some(Backend::Local),
            },
            remote: None,
            conflict: "overwrite".into(),
            keep_going: false,
            stop,
            log,
        }
    }

    fn remote(&mut self) -> Result<&mut Side, String> {
        self.remote
            .as_mut()
            .ok_or_else(|| "Open a connection first, for example: open \"My server\"".to_string())
    }

    /// Runs every line; returns how many commands failed.
    pub(crate) fn script(&mut self, text: &str) -> Result<usize, String> {
        let mut failures = 0;
        for (index, line) in text.lines().enumerate() {
            if self.stop.load(Ordering::Relaxed) {
                return Err("The script was stopped.".into());
            }
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            (self.log)(&format!("> {line}"));
            let words = split_line(line).map_err(|error| format!("Line {}: {error}", index + 1))?;
            let words: Vec<String> = words.iter().map(|word| expand(word)).collect();
            if words[0] == "exit" {
                break;
            }
            if let Err(error) = self.command(&words) {
                (self.log)(&format!("Error on line {}: {error}", index + 1));
                failures += 1;
                if !self.keep_going {
                    return Err(format!("Line {}: {error}", index + 1));
                }
            }
        }
        Ok(failures)
    }

    fn command(&mut self, words: &[String]) -> Result<(), String> {
        let state = self.app.state::<AppState>();
        let args = &words[1..];
        let one = |usage: &str| -> Result<&String, String> {
            match args {
                [value] => Ok(value),
                _ => Err(format!("Usage: {usage}")),
            }
        };
        match words[0].as_str() {
            "echo" => (self.log)(&args.join(" ")),
            "option" => match args {
                [name, value] if name == "conflict" && ["overwrite", "skip", "rename", "newer"].contains(&value.as_str()) => {
                    self.conflict = value.clone()
                }
                [name, value] if name == "errors" && (value == "stop" || value == "continue") => self.keep_going = value == "continue",
                _ => return Err("Usage: option conflict overwrite|skip|rename|newer, or option errors stop|continue".into()),
            },
            "open" => {
                let target = one("open <connection name>")?;
                let profile = if target == "local" { None } else { Some(find_profile(&state, target)?) };
                let mut side = Side {
                    profile,
                    dir: "/".into(),
                    backend: None,
                };
                let home = side.backend(&state)?.stat(".").map(|item| item.path).ok();
                side.dir = home
                    .map(|path| path.replace('\\', "/"))
                    .filter(|path| path.starts_with('/') || drive(path).is_some())
                    .unwrap_or_else(|| "/".into());
                (self.log)(&format!("Connected. Remote folder: {}", side.dir));
                self.remote = Some(side);
            }
            "close" => self.remote = None,
            "pwd" => {
                let dir = self.remote()?.dir.clone();
                (self.log)(&dir)
            }
            "lpwd" => (self.log)(&self.local.dir.clone()),
            "cd" | "lcd" => {
                let path = one(&format!("{} <folder>", words[0]))?;
                let side = if words[0] == "cd" { self.remote()? } else { &mut self.local };
                let target = join(&side.dir, path);
                if !side.backend(&state)?.stat(&target)?.is_dir {
                    return Err(format!("{target} is not a folder."));
                }
                side.dir = target;
            }
            "ls" | "lls" => {
                let side = if words[0] == "ls" { self.remote()? } else { &mut self.local };
                let path = join(&side.dir, args.first().map_or(".", String::as_str));
                let mut items = side.backend(&state)?.list(&path)?;
                items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
                let lines: Vec<String> = items
                    .iter()
                    .map(|item| if item.is_dir { format!("{}/", item.name) } else { format!("{}  {}", item.name, item.size) })
                    .collect();
                for line in lines {
                    (self.log)(&line);
                }
            }
            "mkdir" => {
                let path = one("mkdir <folder>")?;
                let side = self.remote()?;
                let target = join(&side.dir, path);
                side.backend(&state)?.mkdir(&target)?;
            }
            "rm" => {
                let (recursive, path) = match args {
                    [flag, path] if flag == "-r" => (true, path),
                    [path] => (false, path),
                    _ => return Err("Usage: rm [-r] <path>".into()),
                };
                let side = self.remote()?;
                let target = join(&side.dir, path);
                if target == "/" {
                    return Err("Refusing to delete the root folder.".into());
                }
                let backend = side.backend(&state)?;
                if backend.stat(&target)?.is_dir && !recursive {
                    return Err(format!("{target} is a folder. Use rm -r to delete it with its contents."));
                }
                backend.remove_tree(&target)?;
            }
            "mv" => {
                let [from, to] = args else {
                    return Err("Usage: mv <from> <to>".into());
                };
                let side = self.remote()?;
                let (from, to) = (join(&side.dir, from), join(&side.dir, to));
                side.backend(&state)?.rename(&from, &to)?;
            }
            "get" | "put" => {
                let (source, destination) = match args {
                    [source] => (source, None),
                    [source, destination] => (source, Some(destination)),
                    _ => return Err(format!("Usage: {} <file or mask> [destination folder]", words[0])),
                };
                let download = words[0] == "get";
                let remote_dir = self.remote()?.dir.clone();
                let remote_profile = self.remote()?.profile.clone();
                let (source_dir, destination_dir) = if download { (remote_dir, self.local.dir.clone()) } else { (self.local.dir.clone(), remote_dir) };
                let source_path = join(&source_dir, source);
                let destination_dir = destination.map_or(destination_dir.clone(), |path| join(&destination_dir, path));
                let (source_profile, destination_profile) = if download { (remote_profile, None) } else { (None, remote_profile) };
                let sources = {
                    let side = if download { self.remote()? } else { &mut self.local };
                    matching(side.backend(&state)?, &source_path)?
                };
                for path in sources {
                    self.copy(source_profile.clone(), path, destination_profile.clone(), destination_dir.clone())?;
                }
            }
            "sync" => self.sync(args)?,
            other => return Err(format!("Unknown command {other}. Use open, cd, lcd, ls, get, put, mkdir, rm, mv, sync, option, echo or exit.")),
        }
        Ok(())
    }

    fn sync(&mut self, args: &[String]) -> Result<(), String> {
        let usage =
            "sync up|down|both <local folder> <remote folder> [--delete] [--exclude <masks>]";
        let [direction, local, remote, rest @ ..] = args else {
            return Err(format!("Usage: {usage}"));
        };
        let mut delete = false;
        let mut exclude = String::new();
        let mut rest = rest.iter();
        while let Some(flag) = rest.next() {
            match flag.as_str() {
                "--delete" => delete = true,
                "--exclude" => exclude = rest.next().ok_or(format!("Usage: {usage}"))?.clone(),
                _ => return Err(format!("Usage: {usage}")),
            }
        }
        let mode = match (direction.as_str(), delete) {
            ("up", false) => "left-to-right",
            ("up", true) => "mirror-left-to-right",
            ("down", false) => "right-to-left",
            ("down", true) => "mirror-right-to-left",
            ("both", false) => "two-way",
            ("both", true) => return Err("Two-way sync never deletes. Remove --delete.".into()),
            _ => return Err(format!("Usage: {usage}")),
        };
        let local = join(&self.local.dir, local);
        let side = self.remote()?;
        let remote = join(&side.dir, remote);
        let profile = side.profile.clone();
        let state = self.app.state::<AppState>();
        let plan = plan_sync_excluding(&state, &None, &local, &profile, &remote, mode, &exclude)?;
        for issue in &plan.issues {
            (self.log)(&format!("Skipped: {issue}"));
        }
        (self.log)(&format!(
            "{} to copy, {} to delete.",
            plan.copies.len(),
            plan.deletes.len()
        ));
        for copy in plan.copies {
            self.copy(
                copy.source_profile,
                copy.source_path,
                copy.destination_profile,
                copy.destination_dir,
            )?;
        }
        if delete && !plan.deletes.is_empty() {
            let expected = plan.deletes.iter().map(|item| item.path.clone()).collect();
            let deleted = mirror_delete(
                self.app, &None, &local, &profile, &remote, mode, expected, &exclude,
            )?;
            (self.log)(&format!(
                "Deleted {deleted} items that are not in the source."
            ));
        }
        Ok(())
    }

    /// Queues one copy and waits for it, so later lines see the result.
    fn copy(
        &mut self,
        source_profile: Option<String>,
        source_path: String,
        destination_profile: Option<String>,
        destination_dir: String,
    ) -> Result<(), String> {
        let state = self.app.state::<AppState>();
        let record = enqueue_transfer(
            self.app,
            &state,
            source_profile,
            source_path.clone(),
            destination_profile,
            destination_dir,
            self.conflict.clone(),
        )?;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                let _ = pause_transfer(self.app, &state, &record.id);
                return Err("The script was stopped.".into());
            }
            let current = read_transfer(&state, &record.id)?;
            match current.status.as_str() {
                "done" => {
                    (self.log)(&format!("Copied {source_path}"));
                    return Ok(());
                }
                "skipped" => {
                    (self.log)(&format!("Skipped {source_path} (already there)"));
                    return Ok(());
                }
                "failed" => {
                    return Err(current
                        .error
                        .unwrap_or_else(|| format!("Could not copy {source_path}.")))
                }
                "paused" => return Err(format!("The copy of {source_path} was paused.")),
                _ => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    }
}

/// Expands `*` and `?` in the last part of a path.
fn matching(backend: &mut Backend, path: &str) -> Result<Vec<String>, String> {
    let (parent, name) = parent_and_name(path);
    if !name.contains(['*', '?']) {
        backend.stat(path)?;
        return Ok(vec![path.to_string()]);
    }
    let mut found: Vec<String> = backend
        .list(&parent)?
        .into_iter()
        .filter(|item| mask_matches(&name, &item.name))
        .map(|item| item.path)
        .collect();
    if found.is_empty() {
        return Err(format!("Nothing in {parent} matches {name}."));
    }
    found.sort();
    Ok(found)
}

fn find_profile(state: &AppState, name: &str) -> Result<String, String> {
    let db = state
        .db
        .lock()
        .map_err(|_| "Connections are busy. Try again.")?;
    let mut statement = db
        .prepare("SELECT id FROM profiles WHERE id=?1 OR name=?1 COLLATE NOCASE")
        .map_err(|error| format!("Could not read connections: {error}"))?;
    let ids = statement
        .query_map([name], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Could not read connections: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Could not read connections: {error}"))?;
    match ids.as_slice() {
        [id] => Ok(id.clone()),
        [] => Err(format!("No saved connection is named {name}.")),
        _ => Err(format!(
            "Several connections are named {name}. Rename one, or use its ID."
        )),
    }
}

fn running() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static RUNNING: std::sync::OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> =
        std::sync::OnceLock::new();
    RUNNING.get_or_init(Default::default)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScriptLine {
    run_id: String,
    line: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptOutcome {
    failures: usize,
    error: Option<String>,
}

/// Runs a script, sending each output line as a `file-script-output` event.
#[tauri::command]
pub async fn files_script_run(
    app: AppHandle,
    run_id: String,
    local_dir: String,
    script: String,
) -> Result<ScriptOutcome, String> {
    if script.len() > 256 * 1024 {
        return Err("The script is too long (256 KB at most).".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    running()
        .lock()
        .map_err(|_| "Scripts are busy. Try again.")?
        .insert(run_id.clone(), stop.clone());
    let id = run_id.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let emitter = app.clone();
        let mut log = |line: &str| {
            let _ = emitter.emit(
                "file-script-output",
                ScriptLine {
                    run_id: id.clone(),
                    line: line.to_string(),
                },
            );
        };
        let start = if local_dir.trim().is_empty() {
            dirs::home_dir()
                .map(|home| home.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into())
        } else {
            local_dir
        };
        // Scripts write paths with forward slashes on every system.
        let start = start.replace('\\', "/");
        let mut run = Run::new(&app, &start, &stop, &mut log);
        match run.script(&script) {
            Ok(failures) => ScriptOutcome {
                failures,
                error: None,
            },
            Err(error) => ScriptOutcome {
                failures: 1,
                error: Some(error),
            },
        }
    })
    .await
    .map_err(|error| format!("The script stopped unexpectedly: {error}"));
    if let Ok(mut runs) = running().lock() {
        runs.remove(&run_id);
    }
    outcome
}

#[tauri::command]
pub fn files_script_stop(run_id: String) {
    if let Some(flag) = running()
        .lock()
        .ok()
        .and_then(|runs| runs.get(&run_id).cloned())
    {
        flag.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_words_and_joins_paths() {
        assert_eq!(
            split_line(r#"put "my file.txt" 'it''s' a\ b"#).unwrap(),
            ["put", "my file.txt", "its", "a b"]
        );
        assert_eq!(split_line(r#"get """#).unwrap(), ["get", ""]);
        assert!(split_line(r#"get "open"#).is_err());
        assert_eq!(join("/home/ada", "logs/../www"), "/home/ada/www");
        assert_eq!(join("/home/ada", "/etc"), "/etc");
        assert_eq!(join("/", ".."), "/");
        assert_eq!(join("C:/Users/ada", "../bob/x"), "C:/Users/bob/x");
        assert_eq!(join("/home", "D:/data"), "D:/data");
        assert_eq!(join("C:/", ".."), "C:/");
        assert_eq!(parent_and_name("C:/a.txt"), ("C:/".into(), "a.txt".into()));
    }

    #[test]
    fn runs_a_local_script_through_the_queue() {
        let root = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(root.path()).unwrap();
        let local = base.join("local");
        let remote = base.join("remote");
        fs::create_dir_all(local.join("sub")).unwrap();
        fs::create_dir_all(&remote).unwrap();
        fs::write(local.join("a.txt"), "alpha").unwrap();
        fs::write(local.join("b.txt"), "beta").unwrap();
        fs::write(local.join("c.log"), "log").unwrap();
        fs::write(local.join("sub/keep.txt"), "keep").unwrap();
        let mirror = base.join("mirror");
        fs::create_dir_all(&mirror).unwrap();
        fs::write(mirror.join("stale.txt"), "old").unwrap();
        let state = AppState::for_tests();
        state
            .db
            .lock()
            .unwrap()
            .execute_batch("CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, protocol TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT, color TEXT, favorite INTEGER, tls_pin_sha256 TEXT, s3_region TEXT); CREATE TABLE transfers (id TEXT PRIMARY KEY, source_profile TEXT, source_path TEXT, destination_profile TEXT, destination_dir TEXT, target_path TEXT, conflict TEXT, status TEXT, bytes_done INTEGER DEFAULT 0, total_bytes INTEGER DEFAULT 0, error TEXT, queue_order INTEGER NOT NULL DEFAULT 0, method TEXT NOT NULL DEFAULT '', created_at INTEGER DEFAULT (unixepoch()), updated_at INTEGER DEFAULT (unixepoch())); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        let app = tauri::test::mock_app();
        app.manage(state);
        let script = format!(
            "# copy and tidy up\nopen local\ncd \"{remote}\"\nput *.txt\nmkdir archive\nmv a.txt \"archive/a copy.txt\"\nls\nsync up sub \"{mirror}\" --delete\noption errors continue\nrm archive\nrm -r archive\nexit\necho never",
            remote = remote.to_string_lossy().replace('\\', "/"),
            mirror = mirror.to_string_lossy().replace('\\', "/")
        );
        let stop = AtomicBool::new(false);
        let mut lines = Vec::new();
        let mut log = |line: &str| lines.push(line.to_string());
        let local_dir = local.to_string_lossy().replace('\\', "/");
        let failures = Run::new(app.handle(), &local_dir, &stop, &mut log)
            .script(&script)
            .unwrap();
        assert_eq!(failures, 1, "{lines:?}");
        assert_eq!(fs::read_to_string(remote.join("b.txt")).unwrap(), "beta");
        assert!(!remote.join("c.log").exists());
        assert!(!remote.join("archive").exists());
        assert_eq!(fs::read_to_string(mirror.join("keep.txt")).unwrap(), "keep");
        assert!(!mirror.join("stale.txt").exists());
        assert!(lines.iter().any(|line| line == "archive/"));
        assert!(lines
            .iter()
            .any(|line| line.contains("is a folder. Use rm -r")));
        assert!(!lines.iter().any(|line| line == "never"));
        let mut quiet = |_: &str| {};
        let error = Run::new(app.handle(), "/", &stop, &mut quiet)
            .script("cd /tmp")
            .unwrap_err();
        assert!(error.contains("Open a connection first"));
    }
}
