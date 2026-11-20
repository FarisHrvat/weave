//! Starting Weave at login, and running sync jobs while it is quit. macOS
//! uses LaunchAgents in ~/Library/LaunchAgents, Windows the Run key and Task
//! Scheduler, and Linux an autostart entry and a systemd user timer. Only one
//! Weave process at a time owns the transfer queue and job scheduler, decided
//! by a lock file.

use serde::Serialize;
use std::{
    fs::File,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
};

pub const LOGIN_LABEL: &str = "app.weave.desktop.login";
pub const SYNC_LABEL: &str = "app.weave.desktop.sync";
/// The sync agent checks for due jobs this often while Weave is quit.
const SYNC_INTERVAL_SECONDS: u32 = 900;

static OWNS_QUEUE: AtomicBool = AtomicBool::new(false);
static LOCK: OnceLock<File> = OnceLock::new();

/// How this process was started.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Launch {
    Normal,
    /// At login: stay in the menu bar until opened.
    Background,
    /// From the sync agent: run due jobs, wait for their copies, then exit.
    SyncOnly,
}

pub fn launch_mode() -> Launch {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--sync-only") {
        Launch::SyncOnly
    } else if args.iter().any(|arg| arg == "--background") {
        Launch::Background
    } else {
        Launch::Normal
    }
}

pub fn owns_queue() -> bool {
    OWNS_QUEUE.load(Ordering::Relaxed)
}

/// Takes the scheduler lock. With `wait`, blocks until the sync agent exits.
pub fn take_lock(path: &Path, wait: bool) -> std::io::Result<bool> {
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    if wait {
        file.lock()?;
    } else {
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(false),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }
    let _ = LOCK.set(file);
    OWNS_QUEUE.store(true, Ordering::Relaxed);
    Ok(true)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn plist(label: &str, program: &Path, argument: &str, interval: Option<u32>) -> String {
    let schedule = match interval {
        Some(seconds) => format!("  <key>StartInterval</key>\n  <integer>{seconds}</integer>\n  <key>RunAtLoad</key>\n  <false/>\n"),
        None => "  <key>RunAtLoad</key>\n  <true/>\n".into(),
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>{argument}</string>\n  </array>\n{schedule}  <key>LimitLoadToSessionType</key>\n  <string>Aqua</string>\n  <key>ProcessType</key>\n  <string>Background</string>\n</dict>\n</plist>\n",
        escape(label),
        escape(&program.to_string_lossy()),
    )
}

#[cfg(target_os = "macos")]
mod agents {
    use super::plist;
    use std::path::PathBuf;

    fn agents_dir() -> Result<PathBuf, String> {
        Ok(dirs::home_dir()
            .ok_or("Your home folder is unavailable.")?
            .join("Library/LaunchAgents"))
    }

    fn agent_path(label: &str) -> Result<PathBuf, String> {
        Ok(agents_dir()?.join(format!("{label}.plist")))
    }

    fn launchctl(args: &[&str]) {
        let _ = crate::platform::command("/bin/launchctl")
            .args(args)
            .output();
    }

    fn domain() -> String {
        format!("gui/{}", unsafe { libc::getuid() })
    }

    pub fn installed(label: &str) -> bool {
        agent_path(label).is_ok_and(|path| path.exists())
    }

    pub fn install(
        label: &str,
        argument: &str,
        interval: Option<u32>,
        load_now: bool,
    ) -> Result<(), String> {
        let program = std::env::current_exe()
            .map_err(|error| format!("Could not find Weave's program file: {error}"))?;
        std::fs::create_dir_all(agents_dir()?)
            .map_err(|error| format!("Could not create ~/Library/LaunchAgents: {error}"))?;
        let path = agent_path(label)?;
        let text = plist(label, &program, argument, interval);
        if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
            return Ok(());
        }
        std::fs::write(&path, text)
            .map_err(|error| format!("Could not save the login setting: {error}"))?;
        if load_now {
            launchctl(&["bootout", &format!("{}/{label}", domain())]);
            launchctl(&["bootstrap", &domain(), &path.to_string_lossy()]);
        }
        Ok(())
    }

    pub fn uninstall(label: &str) -> Result<(), String> {
        let path = agent_path(label)?;
        if path.exists() {
            launchctl(&["bootout", &format!("{}/{label}", domain())]);
            std::fs::remove_file(&path)
                .map_err(|error| format!("Could not remove the login setting: {error}"))?;
        }
        Ok(())
    }
}

#[cfg(windows)]
mod agents {
    use super::LOGIN_LABEL;

    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const RUN_VALUE: &str = "Weave";
    const TASK: &str = "Weave background sync";

    fn run(program: &str, args: &[&str]) -> Result<String, String> {
        let output = crate::platform::command(program)
            .args(args)
            .output()
            .map_err(|error| format!("Could not change the startup setting: {error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(format!(
                "Windows did not accept the startup setting: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    fn launch_command(argument: &str) -> Result<String, String> {
        let program = std::env::current_exe()
            .map_err(|error| format!("Could not find Weave's program file: {error}"))?;
        Ok(format!("\"{}\" {argument}", program.display()))
    }

    pub fn installed(label: &str) -> bool {
        if label == LOGIN_LABEL {
            run("reg", &["query", RUN_KEY, "/v", RUN_VALUE]).is_ok()
        } else {
            run("schtasks", &["/Query", "/TN", TASK]).is_ok()
        }
    }

    pub fn install(
        label: &str,
        argument: &str,
        interval: Option<u32>,
        _load_now: bool,
    ) -> Result<(), String> {
        let command = launch_command(argument)?;
        if label == LOGIN_LABEL {
            run(
                "reg",
                &[
                    "add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &command, "/f",
                ],
            )?;
        } else {
            let minutes = (interval.unwrap_or(900) / 60).max(1).to_string();
            run(
                "schtasks",
                &[
                    "/Create", "/F", "/SC", "MINUTE", "/MO", &minutes, "/TN", TASK, "/TR", &command,
                ],
            )?;
        }
        Ok(())
    }

    pub fn uninstall(label: &str) -> Result<(), String> {
        if !installed(label) {
            return Ok(());
        }
        if label == LOGIN_LABEL {
            run("reg", &["delete", RUN_KEY, "/v", RUN_VALUE, "/f"])?;
        } else {
            run("schtasks", &["/Delete", "/F", "/TN", TASK])?;
        }
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod agents {
    use super::LOGIN_LABEL;
    use std::path::{Path, PathBuf};

    fn config_dir() -> Result<PathBuf, String> {
        dirs::config_dir().ok_or_else(|| "Your settings folder is unavailable.".to_string())
    }

    fn paths(label: &str) -> Result<Vec<PathBuf>, String> {
        let config = config_dir()?;
        Ok(if label == LOGIN_LABEL {
            vec![config.join("autostart").join("weave.desktop")]
        } else {
            let units = config.join("systemd").join("user");
            vec![
                units.join("weave-sync.service"),
                units.join("weave-sync.timer"),
            ]
        })
    }

    fn quote(path: &Path) -> String {
        format!(
            "\"{}\"",
            path.to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
        )
    }

    pub(crate) fn desktop_entry(program: &Path, argument: &str) -> String {
        format!(
            "[Desktop Entry]\nType=Application\nName=Weave\nExec={} {argument}\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
            quote(program)
        )
    }

    pub(crate) fn sync_units(program: &Path, argument: &str, seconds: u32) -> (String, String) {
        (
            format!(
                "[Unit]\nDescription=Weave background sync\n\n[Service]\nType=oneshot\nExecStart={} {argument}\n",
                quote(program)
            ),
            format!(
                "[Unit]\nDescription=Run Weave background sync\n\n[Timer]\nOnBootSec=5min\nOnUnitActiveSec={seconds}s\n\n[Install]\nWantedBy=timers.target\n"
            ),
        )
    }

    fn systemctl(args: &[&str]) -> Result<(), String> {
        let output = crate::platform::command("systemctl")
            .arg("--user")
            .args(args)
            .output()
            .map_err(|error| format!("Background sync needs systemd: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "systemd did not accept the sync timer: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    pub fn installed(label: &str) -> bool {
        paths(label).is_ok_and(|paths| paths.iter().all(|path| path.exists()))
    }

    fn write(path: &Path, text: &str) -> Result<bool, String> {
        if std::fs::read_to_string(path).ok().as_deref() == Some(text) {
            return Ok(false);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
        }
        std::fs::write(path, text)
            .map_err(|error| format!("Could not save the startup setting: {error}"))?;
        Ok(true)
    }

    pub fn install(
        label: &str,
        argument: &str,
        interval: Option<u32>,
        load_now: bool,
    ) -> Result<(), String> {
        let program = std::env::current_exe()
            .map_err(|error| format!("Could not find Weave's program file: {error}"))?;
        let paths = paths(label)?;
        if label == LOGIN_LABEL {
            write(&paths[0], &desktop_entry(&program, argument))?;
            return Ok(());
        }
        let (service, timer) = sync_units(&program, argument, interval.unwrap_or(900));
        let changed = write(&paths[0], &service)? | write(&paths[1], &timer)?;
        if changed || load_now {
            systemctl(&["daemon-reload"])?;
            systemctl(&["enable", "--now", "weave-sync.timer"])?;
        }
        Ok(())
    }

    pub fn uninstall(label: &str) -> Result<(), String> {
        if label != LOGIN_LABEL && installed(label) {
            let _ = systemctl(&["disable", "--now", "weave-sync.timer"]);
        }
        for path in paths(label)? {
            if path.exists() {
                std::fs::remove_file(&path)
                    .map_err(|error| format!("Could not remove the startup setting: {error}"))?;
            }
        }
        Ok(())
    }
}

use agents::{install, installed, uninstall};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupSettings {
    start_at_login: bool,
    sync_while_quit: bool,
}

fn current() -> Result<StartupSettings, String> {
    Ok(StartupSettings {
        start_at_login: installed(LOGIN_LABEL),
        sync_while_quit: installed(SYNC_LABEL),
    })
}

/// Rewrites enabled agents so they follow the app if it was moved.
pub fn refresh_agents() {
    if installed(LOGIN_LABEL) {
        let _ = install(LOGIN_LABEL, "--background", None, false);
    }
    if installed(SYNC_LABEL) {
        let _ = install(SYNC_LABEL, "--sync-only", Some(SYNC_INTERVAL_SECONDS), true);
    }
}

#[tauri::command]
pub fn startup_settings_get() -> Result<StartupSettings, String> {
    current()
}

#[tauri::command]
pub fn startup_settings_set(
    start_at_login: Option<bool>,
    sync_while_quit: Option<bool>,
) -> Result<StartupSettings, String> {
    match start_at_login {
        Some(true) => install(LOGIN_LABEL, "--background", None, false)?,
        Some(false) => uninstall(LOGIN_LABEL)?,
        None => {}
    }
    match sync_while_quit {
        Some(true) => install(SYNC_LABEL, "--sync-only", Some(SYNC_INTERVAL_SECONDS), true)?,
        Some(false) => uninstall(SYNC_LABEL)?,
        None => {}
    }
    current()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_launch_agents() {
        let login = plist(
            LOGIN_LABEL,
            Path::new("/Applications/Weave & Co.app/Contents/MacOS/weave"),
            "--background",
            None,
        );
        assert!(login
            .contains("<string>/Applications/Weave &amp; Co.app/Contents/MacOS/weave</string>"));
        assert!(login.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(!login.contains("StartInterval"));
        let sync = plist(SYNC_LABEL, Path::new("/x/weave"), "--sync-only", Some(900));
        assert!(sync.contains("<key>StartInterval</key>\n  <integer>900</integer>"));
        assert!(sync.contains("<string>--sync-only</string>"));
        assert!(sync.contains("<key>RunAtLoad</key>\n  <false/>"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn writes_linux_autostart_and_timer() {
        let entry = agents::desktop_entry(Path::new("/opt/Weave/weave"), "--background");
        assert!(entry.contains("Exec=\"/opt/Weave/weave\" --background"));
        let (service, timer) = agents::sync_units(Path::new("/opt/weave"), "--sync-only", 900);
        assert!(service.contains("ExecStart=\"/opt/weave\" --sync-only"));
        assert!(timer.contains("OnUnitActiveSec=900s"));
    }

    #[test]
    fn only_one_holder_gets_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduler.lock");
        let first = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .unwrap();
        first.try_lock().unwrap();
        assert!(!take_lock(&path, false).unwrap());
        drop(first);
        // A process forked by a parallel test may hold the descriptor briefly.
        let taken = (0..100).any(|_| {
            take_lock(&path, false).unwrap() || {
                std::thread::sleep(std::time::Duration::from_millis(20));
                false
            }
        });
        assert!(taken);
        assert!(owns_queue());
    }
}
