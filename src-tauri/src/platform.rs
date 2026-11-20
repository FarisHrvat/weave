//! The few places where macOS, Windows and Linux differ: finding the
//! system's SSH tools, starting programs without a console window, opening
//! links, and keeping files private to the current user.

use std::{
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

/// Tools Weave prefers to take from the system rather than from `PATH`, so a
/// stray copy earlier in `PATH` is not used for SSH sign-ins.
const SYSTEM_TOOLS: [&str; 9] = [
    "curl",
    "ssh",
    "scp",
    "sftp",
    "ssh-keygen",
    "ssh-add",
    "ssh-copy-id",
    "rsync",
    "openssl",
];

fn executable_name(name: &str) -> String {
    if cfg!(windows) && Path::new(name).extension().is_none() {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Folders searched after `PATH`. Apps opened from the Dock, Start menu or a
/// desktop launcher often get a short `PATH`.
pub fn extra_tool_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "macos")]
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"] {
        dirs.push(PathBuf::from(dir));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    for dir in ["/usr/local/bin", "/usr/bin", "/bin", "/snap/bin"] {
        dirs.push(PathBuf::from(dir));
    }
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            dirs.push(PathBuf::from(&root).join("System32").join("OpenSSH"));
        }
        for base in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(base) {
                let base = PathBuf::from(base);
                dirs.push(base.join("Git").join("cmd"));
                dirs.push(base.join("Git").join("usr").join("bin"));
                dirs.push(base.join("Programs").join("Git").join("usr").join("bin"));
            }
        }
    }
    dirs
}

/// Finds a program on `PATH` or in the usual install folders.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let file = executable_name(name);
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    dirs.extend(extra_tool_dirs());
    dirs.into_iter()
        .map(|dir| dir.join(&file))
        .find(|candidate| candidate.is_file())
}

/// The system's own copy of an SSH tool, if there is one.
pub fn system_tool(name: &str) -> Option<PathBuf> {
    if !SYSTEM_TOOLS.contains(&name) {
        return None;
    }
    #[cfg(unix)]
    {
        let system = Path::new("/usr/bin").join(name);
        if system.is_file() {
            return Some(system);
        }
    }
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        let system32 = PathBuf::from(root).join("System32");
        for system in [
            system32.join("OpenSSH").join(executable_name(name)),
            system32.join(executable_name(name)),
        ] {
            if system.is_file() {
                return Some(system);
            }
        }
    }
    find_tool(name)
}

/// A command for `program` that opens no console window on Windows. Bare SSH
/// tool names resolve to the system's copy.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let program = program.as_ref();
    let resolved = program
        .to_str()
        .filter(|name| !name.contains(['/', '\\']))
        .and_then(system_tool);
    #[allow(unused_mut)]
    let mut command = Command::new(resolved.as_deref().map_or(program, Path::as_os_str));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// A command that copies its input to the clipboard.
pub fn clipboard_command() -> Result<Command, String> {
    if cfg!(target_os = "macos") {
        return Ok(command("/usr/bin/pbcopy"));
    }
    if cfg!(windows) {
        return Ok(command("clip"));
    }
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let choices: [(&str, &[&str]); 3] = if wayland {
        [
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    } else {
        [
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
            ("wl-copy", &[]),
        ]
    };
    choices
        .into_iter()
        .find_map(|(tool, args)| {
            find_tool(tool).map(|path| {
                let mut command = command(path);
                command.args(args);
                command
            })
        })
        .ok_or_else(|| {
            "Copying needs wl-copy, xclip or xsel. Install one of them, then try again.".into()
        })
}

/// A POSIX shell for running Weave's own scripts on this computer. Windows
/// has none built in, so the one that comes with Git for Windows is used.
pub fn local_shell() -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        Ok(PathBuf::from("/bin/sh"))
    }
    #[cfg(windows)]
    {
        for base in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(base) {
                let base = PathBuf::from(base);
                for sh in [
                    base.join("Git").join("bin").join("sh.exe"),
                    base.join("Programs").join("Git").join("bin").join("sh.exe"),
                ] {
                    if sh.is_file() {
                        return Ok(sh);
                    }
                }
            }
        }
        find_tool("sh").ok_or_else(|| {
            "This needs Git for Windows, which includes the shell Weave uses. Install it from git-scm.com, then try again.".into()
        })
    }
}

/// A path written so a POSIX shell (including Git's on Windows) reads it as
/// one word.
pub fn shell_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    let text = text.replace('\\', "/");
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Opens a web address in the default browser or app for its scheme.
pub fn open_url(url: &str) -> Result<(), String> {
    tauri_plugin_opener::open_url(url, None::<&str>)
        .map_err(|error| format!("Could not open {url}: {error}"))
}

/// Opens a file or folder in its default app.
pub fn open_path(path: &Path) -> Result<(), String> {
    tauri_plugin_opener::open_path(path, None::<&str>)
        .map_err(|error| format!("Could not open {}: {error}", path.display()))
}

/// Creates a folder (and its parents) that only the current user can open.
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        // Folders under the user's profile are already private to them.
        std::fs::create_dir_all(path)
    }
}

/// Writes a file only the current user can read, replacing what was there.
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(data)
}

/// Lets the current user run a script Weave wrote.
pub fn make_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Creates a symbolic link at `link` pointing to `target`.
pub fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        let resolved = link
            .parent()
            .map_or(target.to_path_buf(), |dir| dir.join(target));
        if resolved.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    }
}

/// What the file manager is called here, for messages and menu labels.
pub fn file_manager_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "Finder"
    } else if cfg!(windows) {
        "File Explorer"
    } else {
        "Files"
    }
}

/// What this computer is called in messages ("this Mac", "this PC").
pub fn this_computer() -> &'static str {
    if cfg!(target_os = "macos") {
        "this Mac"
    } else if cfg!(windows) {
        "this PC"
    } else {
        "this computer"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_for_the_shell() {
        assert_eq!(shell_path(Path::new("/tmp/a b")), "'/tmp/a b'");
        assert_eq!(shell_path(Path::new("/tmp/it's")), "'/tmp/it'\\''s'");
    }

    #[test]
    fn keeps_private_files_private() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("private");
        create_private_dir(&folder).unwrap();
        let file = folder.join("secret");
        write_private(&file, b"one").unwrap();
        write_private(&file, b"two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&folder), 0o700);
            assert_eq!(mode(&file), 0o600);
        }
    }

    #[test]
    fn finds_the_system_ssh_and_leaves_other_names_alone() {
        assert!(system_tool("definitely-not-a-weave-tool").is_none());
        if let Some(ssh) = system_tool("ssh") {
            assert!(ssh.is_file());
        }
    }
}
