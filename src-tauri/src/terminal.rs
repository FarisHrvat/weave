use crate::{profiles, ssh, AppState};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{
    fs,
    io::{Read, Seek, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

pub struct TerminalSession {
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Mutex<Box<dyn Write + Send>>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
}

fn remote_shell_command(path: &str) -> String {
    let quoted = format!("'{}'", path.replace('\'', "'\\''"));
    format!("cd -- {quoted} && exec \"${{SHELL:-/bin/sh}}\" -l")
}

/// Runs the host's startup command, then the usual login shell.
fn startup_shell_command(path: Option<&str>, startup: &str) -> String {
    let enter = path.map_or(String::new(), |path| {
        format!("cd -- '{}' && ", path.replace('\'', "'\\''"))
    });
    format!("{enter}{startup}; exec \"${{SHELL:-/bin/sh}}\" -l")
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalOutput {
    id: String,
    data: Vec<u8>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRunResult {
    pub(crate) exit_code: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

const MAX_SCRIPT_BYTES: usize = 64 * 1024;
const MAX_SCRIPT_OUTPUT_BYTES: u64 = 1024 * 1024;
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(120);

fn script_output_size(stdout: &fs::File, stderr: &fs::File) -> Result<u64, String> {
    let out = stdout
        .metadata()
        .map_err(|error| format!("Could not inspect script output: {error}"))?;
    let err = stderr
        .metadata()
        .map_err(|error| format!("Could not inspect script errors: {error}"))?;
    Ok(out.len().saturating_add(err.len()))
}

fn stop_script(child: &mut Child) {
    #[cfg(unix)]
    let stopped = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) } == 0;
    #[cfg(windows)]
    let stopped = crate::platform::command("taskkill")
        .args(["/F", "/T", "/PID", &child.id().to_string()])
        .output()
        .is_ok_and(|output| output.status.success());
    if !stopped {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn script_command(
    profile: Option<&profiles::Profile>,
    cwd: Option<&str>,
) -> Result<Command, String> {
    if let Some(profile) = profile {
        if profile.protocol != "ssh" {
            return Err("Choose an SSH host or a local terminal to run a script.".into());
        }
        let mut command = crate::platform::command("ssh");
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=10",
            "-p",
            &profile.port.to_string(),
        ]);
        command.args(crate::host_options::prepare(profile));
        command.args(ssh::sharing_args(
            "auto",
            "300",
            &ssh::control_path(profile)?,
        ));
        if let Some(key) = profile.key_path.as_deref().filter(|key| !key.is_empty()) {
            let expanded = if let Some(rest) = key.strip_prefix("~/") {
                dirs::home_dir()
                    .ok_or("Home folder is unavailable; use an absolute SSH key path.")?
                    .join(rest)
                    .to_string_lossy()
                    .into_owned()
            } else {
                key.to_string()
            };
            command.arg("-i").arg(expanded);
        }
        if let Some(jump) = profile.jump_host.as_deref().filter(|jump| !jump.is_empty()) {
            command.arg("-J").arg(jump);
        }
        command.arg("-l").arg(&profile.username).arg(&profile.host);
        let remote = match cwd.filter(|path| !path.is_empty()) {
            Some(path) => {
                let quoted = format!("'{}'", path.replace('\'', "'\\''"));
                format!("cd -- {quoted} && exec sh -s")
            }
            None => "exec sh -s".into(),
        };
        command.arg(remote);
        Ok(command)
    } else {
        let mut command = crate::platform::command(crate::platform::local_shell()?);
        command.arg("-s");
        if let Some(path) = cwd.filter(|path| !path.is_empty()) {
            if !Path::new(path).is_dir() {
                return Err(format!("Script folder does not exist: {path}"));
            }
            command.current_dir(path);
        }
        Ok(command)
    }
}

pub(crate) fn run_script(
    profile: Option<&profiles::Profile>,
    cwd: Option<&str>,
    script: &str,
) -> Result<ScriptRunResult, String> {
    run_script_with_limits(
        profile,
        cwd,
        script,
        SCRIPT_TIMEOUT,
        MAX_SCRIPT_OUTPUT_BYTES,
    )
}

fn run_script_with_limits(
    profile: Option<&profiles::Profile>,
    cwd: Option<&str>,
    script: &str,
    timeout: Duration,
    output_limit: u64,
) -> Result<ScriptRunResult, String> {
    if script.trim().is_empty() {
        return Err("Enter a script before running it.".into());
    }
    if script.len() > MAX_SCRIPT_BYTES {
        return Err("Script is larger than 64 KB. Split it into smaller runs.".into());
    }
    if script.contains('\0') {
        return Err("Script contains a NUL byte. Remove it before running the script.".into());
    }

    let mut input = tempfile::tempfile()
        .map_err(|error| format!("Could not prepare private script input: {error}"))?;
    input
        .write_all(script.as_bytes())
        .and_then(|()| input.rewind())
        .map_err(|error| format!("Could not prepare script input: {error}"))?;
    let mut stdout = tempfile::tempfile()
        .map_err(|error| format!("Could not prepare private script output: {error}"))?;
    let mut stderr = tempfile::tempfile()
        .map_err(|error| format!("Could not prepare private script errors: {error}"))?;

    let mut command = script_command(profile, cwd)?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.stdin(Stdio::from(input));
    command.stdout(Stdio::from(stdout.try_clone().map_err(|error| {
        format!("Could not prepare script output: {error}")
    })?));
    command.stderr(Stdio::from(stderr.try_clone().map_err(|error| {
        format!("Could not prepare script errors: {error}")
    })?));
    let mut child = command.spawn().map_err(|error| {
        format!(
            "Could not start {} script: {error}",
            if profile.is_some() { "SSH" } else { "local" }
        )
    })?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                stop_script(&mut child);
                return Err(format!("Could not check script status: {error}"));
            }
        }
        let output_size = match script_output_size(&stdout, &stderr) {
            Ok(size) => size,
            Err(error) => {
                stop_script(&mut child);
                return Err(error);
            }
        };
        if output_size > output_limit || started.elapsed() >= timeout {
            stop_script(&mut child);
            return Err(if output_size > output_limit {
                "Script output exceeded 1 MB, so the script was stopped. Reduce its output and run it again.".into()
            } else {
                "Script ran for more than 2 minutes and was stopped. Use a terminal for long-running commands.".into()
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if script_output_size(&stdout, &stderr)? > output_limit {
        return Err("Script output exceeded 1 MB. Reduce its output and run it again.".into());
    }
    stdout
        .rewind()
        .and_then(|()| stderr.rewind())
        .map_err(|error| format!("Could not read script output: {error}"))?;
    let mut out = Vec::new();
    let mut err = Vec::new();
    stdout
        .read_to_end(&mut out)
        .and_then(|_| stderr.read_to_end(&mut err))
        .map_err(|error| format!("Could not read script output: {error}"))?;
    Ok(ScriptRunResult {
        exit_code: status.code(),
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    })
}

#[tauri::command]
pub async fn term_run_script(
    app: AppHandle,
    profile_id: Option<String>,
    cwd: Option<String>,
    script: String,
) -> Result<ScriptRunResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let profile = profile_id
            .as_deref()
            .map(|id| profiles::get_profile(&app.state::<AppState>(), id))
            .transpose()?;
        run_script(profile.as_ref(), cwd.as_deref(), &script)
    })
    .await
    .map_err(|error| format!("Script stopped unexpectedly: {error}"))?
}

fn notification_text(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}

/// Shows a system notification. On macOS the text is passed as script
/// arguments, never as AppleScript source, so terminal output cannot inject
/// commands; elsewhere it goes through Tauri's notification support.
#[tauri::command]
pub fn system_notify<R: Runtime>(
    app: AppHandle<R>,
    title: String,
    body: String,
) -> Result<(), String> {
    let title = notification_text(&title, 80);
    let body = notification_text(&body, 240);
    if !cfg!(target_os = "macos") {
        use tauri_plugin_notification::NotificationExt;
        return app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|error| format!("Could not show a notification: {error}"));
    }
    let _ = app;
    crate::platform::command("/usr/bin/osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            &title,
            &body,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not show a notification: {error}"))
}

#[tauri::command]
pub fn term_open(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: Option<String>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    open_terminal(app, state, profile_id, cwd, cols, rows)
}

#[tauri::command]
pub fn term_record_save(path: String, content: String) -> Result<(), String> {
    let destination = Path::new(&path);
    if !destination.is_absolute() || destination.extension().is_none_or(|value| value != "cast") {
        return Err("Choose an absolute file name ending in .cast for the recording.".into());
    }
    if content.len() > 30 * 1024 * 1024 {
        return Err("This recording is larger than 30 MB. Record a shorter session.".into());
    }
    if !content.starts_with("{\"version\":2,") {
        return Err("The recording has an invalid asciicast header.".into());
    }
    write_terminal_export(destination, &content)
}

#[tauri::command]
pub fn term_text_log_save(path: String, content: String) -> Result<(), String> {
    let destination = Path::new(&path);
    if !destination.is_absolute()
        || destination
            .extension()
            .is_none_or(|value| value != "txt" && value != "log")
    {
        return Err("Choose an absolute file name ending in .txt or .log.".into());
    }
    if content.len() > 8 * 1024 * 1024 {
        return Err("This text log is larger than 8 MB. Record a shorter session.".into());
    }
    if content.contains('\x1b') || content.contains('\0') {
        return Err("The text log still contains terminal control characters.".into());
    }
    write_terminal_export(destination, &content)
}

fn write_terminal_export(destination: &Path, content: &str) -> Result<(), String> {
    if fs::symlink_metadata(destination).is_ok_and(|metadata| !metadata.file_type().is_file()) {
        return Err("Choose a regular file, not a folder or symbolic link.".into());
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
        options.mode(0o600);
    }
    let mut file = options.open(destination).map_err(|error| {
        format!(
            "Could not save terminal export to {}: {error}",
            destination.display()
        )
    })?;
    file.write_all(content.as_bytes())
        .map_err(|error| format!("Could not finish terminal export: {error}"))
}

/// The shell a local terminal opens: the user's login shell, or PowerShell
/// on Windows.
fn local_shell_command() -> CommandBuilder {
    #[cfg(windows)]
    {
        for shell in ["pwsh", "powershell"] {
            if let Some(path) = crate::platform::find_tool(shell) {
                let mut command = CommandBuilder::new(path);
                command.arg("-NoLogo");
                return command;
            }
        }
    }
    CommandBuilder::new_default_prog()
}

pub(crate) fn open_terminal<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    profile_id: Option<String>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    let remote = profile_id.is_some();
    let mut command = if let Some(profile_id) = profile_id {
        let profile = profiles::get_profile(&state, &profile_id)?;
        if profile.protocol != "ssh" {
            return Err(
                "FTP, FTPS, and WebDAV profiles only support Files. Choose an SSH host for Terminal.".into(),
            );
        }
        let mut ssh = CommandBuilder::new(
            crate::platform::system_tool("ssh").unwrap_or_else(|| "ssh".into()),
        );
        ssh.args(["-tt", "-p", &profile.port.to_string()]);
        let options = crate::host_options::load(&state, &profile.id);
        options.knock(&profile.host);
        ssh.args(options.ssh_args());
        let control_path = ssh::control_path(&profile)?;
        // A terminal always signs in on its own connection, as `ssh` in
        // Terminal does: some servers refuse a shell on a connection Files
        // opened for SFTP. If no shared connection exists yet, this one
        // becomes it, so Files can reuse the sign-in.
        let shared_open = cfg!(unix)
            && crate::platform::command("ssh")
                .arg("-o")
                .arg(format!("ControlPath={}", control_path.display()))
                .args(["-O", "check", "weave-check"])
                .stdin(std::process::Stdio::null())
                .output()
                .is_ok_and(|output| output.status.success());
        if let Ok(known_hosts) = std::env::var("WEAVE_SSH_TEST_KNOWN_HOSTS") {
            ssh.arg("-o");
            ssh.arg(format!("UserKnownHostsFile={known_hosts}"));
        }
        if shared_open {
            ssh.args(["-o", "ControlPath=none"]);
        } else {
            ssh.args(ssh::sharing_args("yes", "300", &control_path));
        }
        if let Some(key) = profile.key_path.filter(|key| !key.is_empty()) {
            let expanded = if let Some(rest) = key.strip_prefix("~/") {
                dirs::home_dir()
                    .map(|home| home.join(rest).to_string_lossy().to_string())
                    .unwrap_or(key)
            } else {
                key
            };
            ssh.arg("-i");
            ssh.arg(expanded);
        }
        if let Some(jump) = profile.jump_host.filter(|jump| !jump.is_empty()) {
            ssh.arg("-J");
            ssh.arg(jump);
        }
        ssh.arg("-l");
        ssh.arg(profile.username);
        ssh.arg(profile.host);
        let startup = options.startup_command.trim();
        match (cwd.as_deref(), startup.is_empty()) {
            (Some(path), true) => ssh.arg(remote_shell_command(path)),
            (cwd, false) => ssh.arg(startup_shell_command(cwd, startup)),
            (None, true) => {}
        }
        ssh
    } else {
        local_shell_command()
    };
    if let Some(cwd) = cwd.filter(|_| !remote) {
        if !std::path::Path::new(&cwd).is_dir() {
            return Err(format!("Terminal folder does not exist: {cwd}"));
        }
        command.cwd(cwd);
    }
    spawn_session(app, state, command, cols, rows)
}

/// Runs `command` in a new pseudo-terminal and streams its output to the
/// frontend as `terminal-output` events until it exits.
/// The system's language as a UTF-8 locale, for example en_US.UTF-8.
fn system_locale() -> String {
    if !cfg!(target_os = "macos") {
        return "en_US.UTF-8".into();
    }
    let region = crate::platform::command("/usr/bin/defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default();
    locale_from(&region)
}

pub(crate) fn locale_from(apple_locale: &str) -> String {
    let base = apple_locale.split('@').next().unwrap_or("");
    let mut parts = base.split('_');
    match (parts.next(), parts.next()) {
        (Some(language), Some(region))
            if language.len() == 2
                && language.chars().all(|c| c.is_ascii_lowercase())
                && region.len() == 2
                && region.chars().all(|c| c.is_ascii_uppercase()) =>
        {
            format!("{language}_{region}.UTF-8")
        }
        _ => "en_US.UTF-8".into(),
    }
}

pub(crate) fn spawn_session<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    mut command: CommandBuilder,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "Weave");
    // Apps opened from the Dock get no locale, unlike Terminal; without it
    // shells mangle non-English text and ssh sends the server no LANG.
    if std::env::var_os("LANG").is_none() {
        command.env("LANG", system_locale());
    }
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: rows.max(2),
            cols: cols.max(2),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("Could not open terminal: {error}"))?;
    let child = pty
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Could not start terminal: {error}"))?;
    drop(pty.slave);
    let reader = pty
        .master
        .try_clone_reader()
        .map_err(|error| format!("Could not read terminal: {error}"))?;
    let writer = pty
        .master
        .take_writer()
        .map_err(|error| format!("Could not write terminal: {error}"))?;
    let id = uuid::Uuid::new_v4().to_string();
    let event_id = id.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0_u8; 8192];
        loop {
            match std::io::Read::read(&mut reader, &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if app
                        .emit(
                            "terminal-output",
                            TerminalOutput {
                                id: event_id.clone(),
                                data: buffer[..count].to_vec(),
                            },
                        )
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = app.emit("terminal-exit", event_id);
    });
    state
        .terminals
        .lock()
        .map_err(|_| "Terminal manager is busy")?
        .insert(
            id.clone(),
            TerminalSession {
                master: pty.master,
                writer: Mutex::new(writer),
                child,
            },
        );
    Ok(id)
}

#[cfg(test)]
#[cfg_attr(windows, allow(unused_imports, dead_code))]
mod tests {
    #[test]
    fn turns_the_mac_region_into_a_locale() {
        use super::locale_from;
        assert_eq!(locale_from("en_US@rg=itzzzz"), "en_US.UTF-8");
        assert_eq!(locale_from("de_DE@currency=EUR"), "de_DE.UTF-8");
        assert_eq!(locale_from("bs_BA"), "bs_BA.UTF-8");
        assert_eq!(locale_from("en"), "en_US.UTF-8");
        assert_eq!(locale_from(""), "en_US.UTF-8");
    }

    use super::{
        open_terminal, remote_shell_command, run_script, run_script_with_limits, script_command,
        startup_shell_command, term_close, term_record_save, term_resize, term_text_log_save,
        term_write,
    };
    use crate::AppState;
    use rusqlite::Connection;
    use std::{collections::HashMap, sync::Mutex};
    use tauri::Manager;

    #[test]
    fn opens_writes_resizes_and_closes_a_local_terminal() {
        let root = std::env::temp_dir().join(format!("weave-terminal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
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
        let state = app.state::<AppState>();
        let id = open_terminal(
            app.handle().clone(),
            state.clone(),
            None,
            Some(root.to_string_lossy().to_string()),
            80,
            24,
        )
        .unwrap();
        assert!(state.terminals.lock().unwrap().contains_key(&id));
        term_resize(state.clone(), id.clone(), 100, 30).unwrap();
        term_write(state.clone(), id.clone(), "printf 'ready\\n'\n".into()).unwrap();
        term_close(state, id).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn strips_control_characters_from_notifications() {
        assert_eq!(super::notification_text("a\nb\u{1b}[31mc", 80), "ab[31mc");
        assert_eq!(super::notification_text("abcdef", 3), "abc");
    }

    #[test]
    fn quotes_remote_terminal_folder() {
        assert_eq!(
            remote_shell_command("/home/alice/my 'work'"),
            "cd -- '/home/alice/my '\\''work'\\''' && exec \"${SHELL:-/bin/sh}\" -l"
        );
    }

    #[test]
    fn runs_the_startup_command_before_the_shell() {
        assert_eq!(
            startup_shell_command(None, "tmux attach || tmux"),
            "tmux attach || tmux; exec \"${SHELL:-/bin/sh}\" -l"
        );
        assert_eq!(
            startup_shell_command(Some("/srv/app"), "git status"),
            "cd -- '/srv/app' && git status; exec \"${SHELL:-/bin/sh}\" -l"
        );
    }

    #[test]
    fn saves_a_recording_and_rejects_wrong_extension() {
        let root = std::env::temp_dir().join(format!("weave-cast-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("session.cast");
        let content = "{\"version\":2,\"width\":80,\"height\":24}\n[0.1,\"o\",\"hello\"]\n";
        term_record_save(path.to_string_lossy().into_owned(), content.into()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert!(term_record_save(
            root.join("bad.txt").to_string_lossy().into_owned(),
            content.into()
        )
        .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn saves_plain_text_logs_and_rejects_control_sequences() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session.log");
        term_text_log_save(path.to_string_lossy().into_owned(), "café\n".into()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "café\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(term_text_log_save(
            root.path()
                .join("session.cast")
                .to_string_lossy()
                .into_owned(),
            "text".into(),
        )
        .is_err());
        assert!(
            term_text_log_save(path.to_string_lossy().into_owned(), "\x1b[31mred".into()).is_err()
        );
    }

    #[test]
    fn runs_a_local_script_and_reports_its_exit_status_and_output() {
        let folder = tempfile::tempdir().unwrap();
        let result = run_script(
            None,
            folder.path().to_str(),
            "pwd\nprintf 'ready\\n'\nprintf 'warning\\n' >&2\nexit 7\n",
        )
        .unwrap();
        assert_eq!(result.exit_code, Some(7));
        // Git's shell on Windows prints C:\Users\x as /c/Users/x.
        let name = folder.path().file_name().unwrap().to_str().unwrap();
        assert!(result.stdout.contains(name), "{}", result.stdout);
        assert!(result.stdout.contains("ready"));
        assert_eq!(result.stderr, "warning\n");
    }

    #[test]
    fn rejects_empty_oversized_and_invalid_script_input() {
        assert!(run_script(None, None, "  \n")
            .unwrap_err()
            .contains("Enter a script"));
        assert!(run_script(None, None, &"x".repeat(65_537))
            .unwrap_err()
            .contains("64 KB"));
        assert!(run_script(None, None, "echo\0oops")
            .unwrap_err()
            .contains("NUL"));
        assert!(run_script(None, Some("/this/folder/does/not/exist"), "pwd")
            .unwrap_err()
            .contains("does not exist"));
    }

    #[cfg(unix)]
    #[test]
    fn stops_scripts_that_run_too_long_or_produce_too_much_output() {
        use std::time::{Duration, Instant};

        let started = Instant::now();
        let timeout =
            run_script_with_limits(None, None, "sleep 2", Duration::from_millis(100), 1024)
                .unwrap_err();
        assert!(timeout.contains("2 minutes"));
        assert!(started.elapsed() < Duration::from_secs(1));

        let output = run_script_with_limits(
            None,
            None,
            "head -c 300 /dev/zero",
            Duration::from_secs(5),
            100,
        )
        .unwrap_err();
        assert!(output.contains("1 MB"));
    }

    #[test]
    fn quotes_a_remote_script_folder_without_executing_it_as_a_command() {
        let profile = crate::profiles::Profile {
            id: "host".into(),
            name: "Test".into(),
            host: "example.test".into(),
            protocol: "ssh".into(),
            port: 2222,
            username: "alice".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let command = script_command(Some(&profile), Some("/home/alice/my 'work'")).unwrap();
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args.last().unwrap(),
            "cd -- '/home/alice/my '\\''work'\\''' && exec sh -s"
        );
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"StrictHostKeyChecking=yes".to_string()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires the local SSH test server"]
    fn runs_a_script_on_a_real_ssh_host() {
        use std::fs;

        let port = std::env::var("WEAVE_SSH_TEST_PORT").unwrap();
        let key = std::env::var("WEAVE_SSH_TEST_KEY").unwrap();
        let known_hosts = std::env::var("WEAVE_SSH_TEST_KNOWN_HOSTS").unwrap();
        let username = std::env::var("WEAVE_SSH_TEST_USER").unwrap();
        let profile = crate::profiles::Profile {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Local SSH test".into(),
            host: "127.0.0.1".into(),
            protocol: "ssh".into(),
            port: port.parse().unwrap(),
            username: username.clone(),
            key_path: Some(key.clone()),
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let socket = crate::ssh::control_path(&profile).unwrap();
        let master = crate::platform::command("ssh")
            .args([
                "-MNf",
                "-o",
                "ControlMaster=yes",
                "-o",
                "ControlPersist=10",
                "-o",
                &format!("ControlPath={}", socket.display()),
                "-o",
                &format!("UserKnownHostsFile={known_hosts}"),
                "-o",
                "StrictHostKeyChecking=yes",
                "-p",
                &port,
                "-i",
                &key,
                "-l",
                &username,
                "127.0.0.1",
            ])
            .output()
            .unwrap();
        assert!(
            master.status.success(),
            "{}",
            String::from_utf8_lossy(&master.stderr)
        );

        let root = tempfile::tempdir_in("/private/tmp").unwrap();
        let folder = root.path().join("odd 'folder'");
        fs::create_dir(&folder).unwrap();
        let result = run_script(
            Some(&profile),
            folder.to_str(),
            "pwd\nprintf 'remote-ready\\n'\n",
        )
        .unwrap();
        assert_eq!(result.exit_code, Some(0), "{}", result.stderr);
        assert!(result.stdout.contains(folder.to_str().unwrap()));
        assert!(result.stdout.contains("remote-ready"));

        let _ = crate::platform::command("ssh")
            .args([
                "-O",
                "exit",
                "-o",
                &format!("ControlPath={}", socket.display()),
                "-p",
                &port,
                "-l",
                &username,
                "127.0.0.1",
            ])
            .output();
    }
}

#[tauri::command]
pub fn term_write(state: State<'_, AppState>, id: String, data: String) -> Result<(), String> {
    let terminals = state
        .terminals
        .lock()
        .map_err(|_| "Terminal manager is busy")?;
    let session = terminals.get(&id).ok_or("Terminal session has ended.")?;
    let result = session
        .writer
        .lock()
        .map_err(|_| "Terminal is busy")?
        .write_all(data.as_bytes())
        .map_err(|error| format!("Could not send input to terminal: {error}"));
    result
}

/// Types the connection's saved password at a prompt, without it passing
/// through the web view.
#[tauri::command]
pub fn term_fill_password(
    state: State<'_, AppState>,
    id: String,
    profile_id: String,
) -> Result<(), String> {
    let profile = crate::profiles::get_profile(&state, &profile_id)?;
    let password = crate::vault::password_for(&state, &profile)
        .ok()
        .filter(|password| !password.is_empty())
        .ok_or("No password is saved for this connection. Edit the connection to add one.")?;
    let mut input = zeroize::Zeroizing::new(password.as_bytes().to_vec());
    input.push(b'\r');
    let terminals = state
        .terminals
        .lock()
        .map_err(|_| "Terminal manager is busy")?;
    let session = terminals.get(&id).ok_or("Terminal session has ended.")?;
    let result = session
        .writer
        .lock()
        .map_err(|_| "Terminal is busy")?
        .write_all(&input)
        .map_err(|error| format!("Could not send the password: {error}"));
    result
}

/// Sends raw bytes, base64 encoded, such as a ZMODEM transfer.
#[tauri::command]
pub fn term_write_base64(
    state: State<'_, AppState>,
    id: String,
    data: String,
) -> Result<(), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| "The data to send was damaged.")?;
    let terminals = state
        .terminals
        .lock()
        .map_err(|_| "Terminal manager is busy")?;
    let session = terminals.get(&id).ok_or("Terminal session has ended.")?;
    let result = session
        .writer
        .lock()
        .map_err(|_| "Terminal is busy")?
        .write_all(&bytes)
        .map_err(|error| format!("Could not send data to the terminal: {error}"));
    result
}

#[tauri::command]
pub fn term_resize(
    state: State<'_, AppState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let terminals = state
        .terminals
        .lock()
        .map_err(|_| "Terminal manager is busy")?;
    let session = terminals.get(&id).ok_or("Terminal session has ended.")?;
    session
        .master
        .resize(PtySize {
            rows: rows.max(2),
            cols: cols.max(2),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("Could not resize terminal: {error}"))
}

#[tauri::command]
pub fn term_close(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut terminals = state
        .terminals
        .lock()
        .map_err(|_| "Terminal manager is busy")?;
    if let Some(mut session) = terminals.remove(&id) {
        session
            .child
            .kill()
            .map_err(|error| format!("Could not close terminal: {error}"))?;
    }
    Ok(())
}
