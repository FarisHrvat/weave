//! Copies between this Mac and an SSH host with the system's scp or rsync,
//! run from the transfer queue.

use crate::profiles::Profile;
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

fn safe_remote_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/')
        || path.split('/').any(|part| part == "." || part == "..")
        || path.chars().any(char::is_control)
    {
        return Err(
            "SCP and rsync need a full path on the SSH host without line breaks or .. parts."
                .into(),
        );
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(crate) fn ssh_options(profile: &Profile) -> Result<Vec<String>, String> {
    if profile.protocol != "ssh" {
        return Err("SCP and rsync require an SSH host profile.".into());
    }
    let mut options = vec![
        "-o".into(),
        "StrictHostKeyChecking=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
    ];
    options.extend(crate::ssh::sharing_args(
        "auto",
        "300",
        &crate::ssh::control_path(profile)?,
    ));
    if let Ok(known_hosts) = std::env::var("WEAVE_SSH_TEST_KNOWN_HOSTS") {
        options.extend(["-o".into(), format!("UserKnownHostsFile={known_hosts}")]);
    }
    if let Some(key) = &profile.key_path {
        let expanded = if let Some(rest) = key.strip_prefix("~/") {
            dirs::home_dir()
                .ok_or("Your home folder is unavailable for this SSH key.")?
                .join(rest)
                .to_string_lossy()
                .into_owned()
        } else {
            key.clone()
        };
        options.extend(["-i".into(), expanded]);
    }
    if let Some(jump) = &profile.jump_host {
        options.extend(["-J".into(), jump.clone()]);
    }
    options.extend(crate::host_options::prepare(profile));
    Ok(options)
}

fn command_for(
    method: &str,
    profile: &Profile,
    source: &str,
    destination: &str,
) -> Result<Command, String> {
    let options = ssh_options(profile)?;
    match method {
        "scp" => {
            let mut command = crate::platform::command("scp");
            command.args(["-p", "-r", "-P", &profile.port.to_string()]);
            command.args(options);
            command.args(["--", source, destination]);
            crate::ssh_prompt::apply(&mut command, profile);
            Ok(command)
        }
        "rsync" => {
            let ssh = std::iter::once(crate::platform::system_tool("ssh").map_or_else(
                || "ssh".to_string(),
                |path| path.to_string_lossy().into_owned(),
            ))
            .chain(std::iter::once("-p".into()))
            .chain(std::iter::once(profile.port.to_string()))
            .chain(options)
            .map(|arg| shell_quote(&arg))
            .collect::<Vec<_>>()
            .join(" ");
            let mut command = crate::platform::command("rsync");
            command.args(["-a", "--partial", "-e", &ssh, "--", source, destination]);
            crate::ssh_prompt::apply(&mut command, profile);
            Ok(command)
        }
        _ => Err("Choose SCP or rsync for a direct transfer.".into()),
    }
}

/// Runs one queued scp or rsync copy to an exact target path. Pausing stops
/// the process; rsync keeps partial files so the next run continues them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn copy(
    method: &str,
    profile: &Profile,
    remote_source: bool,
    source_path: &str,
    target: &str,
    is_dir: bool,
    replacing_folder: bool,
    flag: &AtomicBool,
) -> Result<(), String> {
    safe_remote_path(if remote_source { source_path } else { target })?;
    if method == "scp" && replacing_folder {
        return Err(
            "SCP cannot replace a folder that already exists. Choose rsync, or Keep both.".into(),
        );
    }
    let slash = |path: &str| {
        if is_dir && method == "rsync" {
            format!("{}/", path.trim_end_matches('/'))
        } else {
            path.to_string()
        }
    };
    // scp uses the SFTP protocol and takes paths literally; rsync hands the
    // path to the remote shell, so it is quoted there.
    let remote = |path: &str| {
        let path = slash(path);
        let path = if method == "rsync" {
            shell_quote(&path)
        } else {
            path
        };
        format!("{}@{}:{path}", profile.username, profile.host)
    };
    let (from, to) = if remote_source {
        (remote(source_path), slash(target))
    } else {
        (slash(source_path), remote(target))
    };
    run(command_for(method, profile, &from, &to)?, method, flag)
}

/// One ssh_config block, so each host keeps its own port, key and jump host.
fn config_block(alias: &str, profile: &Profile) -> Result<String, String> {
    if profile.protocol != "ssh" {
        return Err(format!("{} is not an SSH host.", profile.name));
    }
    let quote = |value: &str| format!("\"{}\"", value.replace('"', ""));
    let mut lines = vec![
        format!("Host {alias}"),
        format!("  HostName {}", profile.host),
        format!("  Port {}", profile.port),
        format!("  User {}", quote(&profile.username)),
        "  BatchMode yes".into(),
        "  StrictHostKeyChecking yes".into(),
        "  ConnectTimeout 10".into(),
    ];
    if cfg!(unix) {
        lines.push("  ControlMaster auto".into());
        lines.push(format!(
            "  ControlPath {}",
            quote(&crate::ssh::control_path(profile)?.to_string_lossy())
        ));
    }
    if let Ok(known_hosts) = std::env::var("WEAVE_SSH_TEST_KNOWN_HOSTS") {
        lines.push(format!("  UserKnownHostsFile {}", quote(&known_hosts)));
    }
    if let Some(key) = &profile.key_path {
        let key = match key.strip_prefix("~/") {
            Some(rest) => dirs::home_dir()
                .ok_or("Your home folder is unavailable for this SSH key.")?
                .join(rest)
                .to_string_lossy()
                .into_owned(),
            None => key.clone(),
        };
        lines.push(format!("  IdentityFile {}", quote(&key)));
    }
    if let Some(jump) = &profile.jump_host {
        lines.push(format!("  ProxyJump {jump}"));
    }
    for option in crate::host_options::prepare(profile)
        .iter()
        .filter(|arg| *arg != "-o")
    {
        if let Some((key, value)) = option.split_once('=') {
            lines.push(format!("  {key} {value}"));
        }
    }
    Ok(lines.join("\n") + "\n")
}

/// Copies between two SSH hosts with `scp -3`: the data streams through this
/// Mac without a temporary copy, and neither host needs access to the other.
pub(crate) fn copy_between(
    source: &Profile,
    destination: &Profile,
    source_path: &str,
    target: &str,
    replacing_folder: bool,
    flag: &AtomicBool,
) -> Result<(), String> {
    safe_remote_path(source_path)?;
    safe_remote_path(target)?;
    if replacing_folder {
        return Err("SCP cannot replace a folder that already exists. Choose Keep both, or copy with the built-in engine.".into());
    }
    let mut config = tempfile::NamedTempFile::new()
        .map_err(|error| format!("Could not prepare SCP: {error}"))?;
    let text =
        config_block("weave-source", source)? + &config_block("weave-destination", destination)?;
    config
        .write_all(text.as_bytes())
        .map_err(|error| format!("Could not prepare SCP: {error}"))?;
    let mut command = crate::platform::command("scp");
    command
        .args(["-3", "-B", "-p", "-r", "-F"])
        .arg(config.path())
        .arg("--")
        .arg(format!("weave-source:{source_path}"))
        .arg(format!("weave-destination:{target}"));
    run(command, "scp", flag)
}

fn run(mut command: Command, method: &str, flag: &AtomicBool) -> Result<(), String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start {method}: {error}"))?;
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(stderr) = stderr.as_mut() {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    });
    let status = loop {
        if !flag.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Transfer paused.".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(error) => return Err(format!("Lost track of {method}: {error}")),
        }
    };
    let detail = reader.join().unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    let lines: Vec<&str> = detail
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let detail = lines[lines.len().saturating_sub(3)..]
        .join(" ")
        .chars()
        .take(400)
        .collect::<String>();
    let detail = if detail.is_empty() {
        "no details".to_string()
    } else {
        detail
    };
    Err(format!("{method} could not copy the item: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Profile {
        Profile {
            id: "test".into(),
            name: "SSH test".into(),
            host: "127.0.0.1".into(),
            protocol: "ssh".into(),
            port: 22,
            username: "test".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn validates_remote_paths_and_builds_commands_without_shell_interpolation() {
        assert!(safe_remote_path("/home/test/project/a.txt").is_ok());
        assert!(safe_remote_path("/home/test/has space (2)").is_ok());
        assert!(safe_remote_path("/home/test/line\nbreak").is_err());
        assert!(safe_remote_path("/home/test/../other").is_err());
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        let command = command_for("scp", &profile(), "/tmp/a", "test@127.0.0.1:/tmp/b").unwrap();
        assert_eq!(command.get_program(), "/usr/bin/scp");
        assert!(command_for("not-a-method", &profile(), "/tmp/a", "/tmp/b").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn stops_the_copy_when_paused() {
        let paused = AtomicBool::new(false);
        let profile = Profile {
            port: 1,
            ..profile()
        };
        let error = copy(
            "scp", &profile, false, "/tmp/a", "/tmp/b", false, false, &paused,
        )
        .unwrap_err();
        assert_eq!(error, "Transfer paused.");
        assert!(copy(
            "scp",
            &profile,
            false,
            "/tmp/a",
            "/tmp/../b",
            false,
            false,
            &paused
        )
        .is_err());
        assert!(
            copy("scp", &profile, false, "/tmp/a", "/tmp/b", true, true, &paused)
                .unwrap_err()
                .contains("cannot replace a folder")
        );
    }

    #[test]
    #[ignore = "requires the local SSH test server"]
    fn copies_to_and_from_real_ssh_with_scp_and_rsync() {
        let port = std::env::var("WEAVE_SSH_TEST_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let profile = Profile {
            port,
            username: std::env::var("WEAVE_SSH_TEST_USER").unwrap(),
            key_path: Some(std::env::var("WEAVE_SSH_TEST_KEY").unwrap()),
            ..profile()
        };
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("site");
        std::fs::create_dir_all(source.join("css")).unwrap();
        std::fs::write(source.join("css/main.css"), "body{}").unwrap();
        let remote = root.path().join("remote-site");
        std::fs::create_dir_all(&remote).unwrap();
        std::fs::write(remote.join("old.txt"), "kept").unwrap();
        let running = AtomicBool::new(true);
        copy(
            "rsync",
            &profile,
            false,
            source.to_str().unwrap(),
            remote.to_str().unwrap(),
            true,
            true,
            &running,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(remote.join("css/main.css")).unwrap(),
            "body{}"
        );
        assert!(remote.join("old.txt").exists());
        let back = root.path().join("back");
        copy(
            "scp",
            &profile,
            true,
            remote.to_str().unwrap(),
            back.to_str().unwrap(),
            true,
            false,
            &running,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(back.join("css/main.css")).unwrap(),
            "body{}"
        );
        let note = root.path().join("it's a note (2).txt");
        std::fs::write(&note, "single file").unwrap();
        let uploaded = remote.join("it's a note (2).txt");
        copy(
            "scp",
            &profile,
            false,
            note.to_str().unwrap(),
            uploaded.to_str().unwrap(),
            false,
            false,
            &running,
        )
        .unwrap();
        let downloaded = root.path().join("back copy.txt");
        copy(
            "rsync",
            &profile,
            true,
            uploaded.to_str().unwrap(),
            downloaded.to_str().unwrap(),
            false,
            false,
            &running,
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(downloaded).unwrap(), "single file");
        let spaced = root.path().join("spaced folder");
        copy(
            "rsync",
            &profile,
            false,
            source.to_str().unwrap(),
            spaced.to_str().unwrap(),
            true,
            false,
            &running,
        )
        .unwrap();
        assert!(spaced.join("css/main.css").exists());
    }

    #[test]
    fn writes_a_separate_config_block_per_host() {
        let block = config_block(
            "weave-source",
            &Profile {
                jump_host: Some("bastion".into()),
                key_path: Some("/keys/a b".into()),
                ..profile()
            },
        )
        .unwrap();
        assert!(block.starts_with("Host weave-source\n  HostName 127.0.0.1\n  Port 22\n"));
        assert!(
            block.contains("  IdentityFile \"/keys/a b\"\n")
                && block.contains("  ProxyJump bastion\n")
        );
        assert!(config_block(
            "x",
            &Profile {
                protocol: "ftp".into(),
                ..profile()
            }
        )
        .is_err());
    }

    #[test]
    #[ignore = "requires the local SSH test server"]
    fn copies_between_two_ssh_hosts_through_this_mac() {
        let port = std::env::var("WEAVE_SSH_TEST_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let host = Profile {
            port,
            username: std::env::var("WEAVE_SSH_TEST_USER").unwrap(),
            key_path: Some(std::env::var("WEAVE_SSH_TEST_KEY").unwrap()),
            ..profile()
        };
        let other = Profile {
            id: "other".into(),
            ..host.clone()
        };
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        std::fs::create_dir_all(base.join("from/inner")).unwrap();
        std::fs::write(base.join("from/inner/a file.txt"), "between hosts").unwrap();
        let running = AtomicBool::new(true);
        let target = base.join("to copy");
        copy_between(
            &host,
            &other,
            base.join("from").to_str().unwrap(),
            target.to_str().unwrap(),
            false,
            &running,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("inner/a file.txt")).unwrap(),
            "between hosts"
        );
    }
}
