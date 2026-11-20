use crate::{profiles, ssh, AppState};
use serde::Serialize;
use std::path::PathBuf;
use tauri::State;
use uuid::Uuid;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tunnel {
    pub id: String,
    pub profile_id: String,
    pub profile_name: String,
    pub kind: String,
    pub listen_port: u16,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
    #[serde(skip)]
    pub socket: PathBuf,
    #[serde(skip)]
    pub ssh_host: String,
    #[serde(skip)]
    pub ssh_port: u16,
    #[serde(skip)]
    pub ssh_user: String,
}

fn validate(
    kind: &str,
    listen_port: u16,
    target_host: &str,
    target_port: u16,
) -> Result<(), String> {
    if listen_port == 0 {
        return Err("Choose a listening port between 1 and 65535.".into());
    }
    if !["local", "remote", "dynamic"].contains(&kind) {
        return Err("Choose a local, remote, or dynamic SSH tunnel.".into());
    }
    if kind != "dynamic" && (target_host.trim().is_empty() || target_port == 0) {
        return Err("Enter a destination host and port for this tunnel.".into());
    }
    let bracketed_ipv6 = target_host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .is_some_and(|host| host.parse::<std::net::Ipv6Addr>().is_ok());
    if target_host.chars().any(char::is_whitespace)
        || (target_host.contains(':') && !bracketed_ipv6)
    {
        return Err(
            "Enter a destination hostname, IPv4 address, or bracketed IPv6 address without a port."
                .into(),
        );
    }
    Ok(())
}

fn forwarded_port(kind: &str, listen_port: u16, target_host: &str, target_port: u16) -> String {
    match kind {
        "dynamic" => format!("127.0.0.1:{listen_port}"),
        _ => format!("127.0.0.1:{listen_port}:{target_host}:{target_port}"),
    }
}

#[cfg(unix)]
fn control_command(tunnel: &Tunnel, operation: &str) -> Result<std::process::Output, String> {
    crate::platform::command("ssh")
        .args(["-S", &tunnel.socket.to_string_lossy(), "-O", operation])
        .args([
            "-p",
            &tunnel.ssh_port.to_string(),
            "-l",
            &tunnel.ssh_user,
            &tunnel.ssh_host,
        ])
        .output()
        .map_err(|error| format!("Could not run SSH to inspect the tunnel: {error}"))
}

/// Windows' OpenSSH cannot run in the background or share a connection, so
/// there each tunnel is an `ssh` process Weave keeps until it is stopped.
#[cfg(windows)]
fn processes() -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, std::process::Child>>
{
    static PROCESSES: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, std::process::Child>>,
    > = std::sync::OnceLock::new();
    PROCESSES.get_or_init(Default::default)
}

fn running(tunnel: &Tunnel) -> bool {
    #[cfg(unix)]
    {
        control_command(tunnel, "check").is_ok_and(|output| output.status.success())
    }
    #[cfg(windows)]
    {
        processes().lock().is_ok_and(|mut children| {
            children
                .get_mut(&tunnel.socket)
                .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
        })
    }
}

fn stop(tunnel: &Tunnel) -> Result<(), String> {
    #[cfg(unix)]
    {
        let output = control_command(tunnel, "exit")?;
        if !output.status.success() && tunnel.socket.exists() {
            return Err(format!(
                "Could not stop SSH tunnel: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        if let Some(mut child) = processes()
            .lock()
            .map_err(|_| "SSH tunnel list is busy")?
            .remove(&tunnel.socket)
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }
}

fn start_failed(reason: &str) -> String {
    format!(
        "SSH tunnel could not start: {}. Confirm the host key, SSH key or agent, destination, and listening port.",
        if reason.is_empty() { "SSH returned no details" } else { reason }
    )
}

/// Starts `ssh` for the tunnel and returns once the forward is up.
#[cfg(unix)]
fn launch(
    mut command: std::process::Command,
    _socket: &std::path::Path,
    _kind: &str,
    _port: u16,
) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("Could not start SSH tunnel: {error}"))?;
    if !output.status.success() {
        return Err(start_failed(String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(())
}

#[cfg(windows)]
fn launch(
    mut command: std::process::Command,
    socket: &std::path::Path,
    kind: &str,
    port: u16,
) -> Result<(), String> {
    use std::{
        io::Read,
        process::Stdio,
        time::{Duration, Instant},
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start SSH tunnel: {error}"))?;
    // Leave time to answer a password or code prompt.
    let started = Instant::now();
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            let mut reason = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = stderr.read_to_string(&mut reason);
            }
            return Err(start_failed(reason.trim()));
        }
        let listening = kind != "remote"
            && std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
                Duration::from_millis(200),
            )
            .is_ok();
        if listening || (kind == "remote" && started.elapsed() > Duration::from_secs(5)) {
            break;
        }
        if started.elapsed() > Duration::from_secs(180) {
            let _ = child.kill();
            return Err(start_failed("the tunnel did not open within three minutes"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    processes()
        .lock()
        .map_err(|_| "SSH tunnel list is busy")?
        .insert(socket.to_path_buf(), child);
    Ok(())
}

#[tauri::command]
pub fn tunnels_list(state: State<'_, AppState>) -> Result<Vec<Tunnel>, String> {
    let mut tunnels = state
        .tunnels
        .lock()
        .map_err(|_| "SSH tunnel list is busy")?;
    tunnels.retain(running);
    Ok(tunnels.clone())
}

#[tauri::command]
pub fn tunnels_start(
    state: State<'_, AppState>,
    profile_id: String,
    kind: String,
    listen_port: u16,
    target_host: String,
    target_port: u16,
) -> Result<Tunnel, String> {
    validate(&kind, listen_port, &target_host, target_port)?;
    let profile = profiles::get_profile(&state, &profile_id)?;
    if profile.protocol != "ssh" {
        return Err("SSH tunnels require an SSH host profile.".into());
    }
    let socket_dir = ssh::control_path(&profile)?;
    let id = Uuid::new_v4().simple().to_string();
    let socket = socket_dir.with_file_name(format!("t-{}", &id[..16]));
    let forward = forwarded_port(&kind, listen_port, target_host.trim(), target_port);
    let mut command = crate::platform::command("ssh");
    if cfg!(unix) {
        command.args(["-M", "-f", "-S"]);
        command.arg(&socket);
        command.args(["-o", "ControlMaster=yes", "-o", "ControlPersist=no"]);
    }
    command.args([
        "-N",
        "-T",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ConnectTimeout=10",
        "-p",
        &profile.port.to_string(),
    ]);
    command.args(crate::host_options::prepare(&profile));
    crate::ssh_prompt::apply(&mut command, &profile);
    #[cfg(test)]
    if let Ok(known_hosts) = std::env::var("WEAVE_SSH_TEST_KNOWN_HOSTS") {
        command
            .arg("-o")
            .arg(format!("UserKnownHostsFile={known_hosts}"));
        command.args(["-o", "StrictHostKeyChecking=yes"]);
    }
    if let Some(key) = profile.key_path.as_deref().filter(|key| !key.is_empty()) {
        let expanded = if let Some(rest) = key.strip_prefix("~/") {
            dirs::home_dir()
                .ok_or("Could not find your home folder for the SSH key.")?
                .join(rest)
        } else {
            PathBuf::from(key)
        };
        command.arg("-i").arg(expanded);
    }
    if let Some(jump) = profile.jump_host.as_deref().filter(|jump| !jump.is_empty()) {
        command.arg("-J").arg(jump);
    }
    let flag = match kind.as_str() {
        "local" => "-L",
        "remote" => "-R",
        _ => "-D",
    };
    command.args([flag, &forward, "-l", &profile.username, &profile.host]);
    launch(command, &socket, &kind, listen_port)?;
    let tunnel = Tunnel {
        id: Uuid::new_v4().to_string(),
        profile_id,
        profile_name: profile.name,
        kind,
        listen_port,
        target_host: if target_host.is_empty() {
            None
        } else {
            Some(target_host.trim().into())
        },
        target_port: if target_port == 0 {
            None
        } else {
            Some(target_port)
        },
        socket,
        ssh_host: profile.host,
        ssh_port: profile.port,
        ssh_user: profile.username,
    };
    if !running(&tunnel) {
        return Err("SSH reported success, but the tunnel closed immediately. Check the server's forwarding policy.".into());
    }
    state
        .tunnels
        .lock()
        .map_err(|_| "SSH tunnel list is busy")?
        .push(tunnel.clone());
    Ok(tunnel)
}

#[tauri::command]
pub fn tunnels_stop(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut tunnels = state
        .tunnels
        .lock()
        .map_err(|_| "SSH tunnel list is busy")?;
    let index = tunnels
        .iter()
        .position(|tunnel| tunnel.id == id)
        .ok_or("This SSH tunnel is no longer active.")?;
    stop(&tunnels[index])?;
    tunnels.remove(index);
    Ok(())
}

/// Stops the tunnels of a host that is being deleted.
pub fn stop_for_profile(state: &AppState, profile_id: &str) {
    let Ok(mut tunnels) = state.tunnels.lock() else {
        return;
    };
    tunnels.retain(|tunnel| {
        if tunnel.profile_id != profile_id {
            return true;
        }
        let _ = stop(tunnel);
        false
    });
}

pub fn stop_all(state: &AppState) {
    if let Ok(mut tunnels) = state.tunnels.lock() {
        for tunnel in tunnels.drain(..) {
            let _ = stop(&tunnel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{forwarded_port, validate};

    #[test]
    fn validates_tunnel_inputs_and_loopback_binding() {
        assert!(validate("local", 0, "localhost", 80).is_err());
        assert!(validate("local", 8080, "", 80).is_err());
        assert!(validate("remote", 8080, "localhost", 0).is_err());
        assert!(validate("dynamic", 1080, "", 0).is_ok());
        assert!(validate("local", 8080, "bad host", 80).is_err());
        assert!(validate("local", 8080, "[::1]", 80).is_ok());
        assert!(validate("local", 8080, "::1", 80).is_err());
        assert_eq!(
            forwarded_port("local", 8080, "localhost", 80),
            "127.0.0.1:8080:localhost:80"
        );
        assert_eq!(forwarded_port("dynamic", 1080, "", 0), "127.0.0.1:1080");
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires the local SSH test server"]
    fn opens_and_stops_a_real_local_ssh_tunnel() {
        use crate::AppState;
        use rusqlite::{params, Connection};
        use std::{
            collections::HashMap,
            io::{Read, Write},
            net::{TcpListener, TcpStream},
            sync::Mutex,
            time::Duration,
        };
        use tauri::Manager;

        let ssh_port: u16 = std::env::var("WEAVE_SSH_TEST_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let key = std::env::var("WEAVE_SSH_TEST_KEY").unwrap();
        let username = std::env::var("WEAVE_SSH_TEST_USER").unwrap();
        let destination = TcpListener::bind("127.0.0.1:0").unwrap();
        let destination_port = destination.local_addr().unwrap().port();
        let free_port = || {
            let available = TcpListener::bind("127.0.0.1:0").unwrap();
            available.local_addr().unwrap().port()
        };
        let listen_port = free_port();
        let remote_port = free_port();
        let socks_port = free_port();
        let receiver = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut connection, _) = destination.accept().unwrap();
                let mut data = [0; 4];
                connection.read_exact(&mut data).unwrap();
                assert_eq!(&data, b"ping");
                connection.write_all(b"pong").unwrap();
            }
        });
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, protocol TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT, color TEXT, favorite INTEGER, tls_pin_sha256 TEXT, s3_region TEXT);").unwrap();
        let profile_id = uuid::Uuid::new_v4().to_string();
        db.execute("INSERT INTO profiles VALUES (?1, 'Tunnel test', '127.0.0.1', 'ssh', ?2, ?3, ?4, NULL, '#9d91ec', 0, NULL, NULL)", params![profile_id, ssh_port, username, key]).unwrap();
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
        let state = app.state::<AppState>();
        let tunnel = super::tunnels_start(
            state.clone(),
            profile_id,
            "local".into(),
            listen_port,
            "127.0.0.1".into(),
            destination_port,
        )
        .unwrap();
        assert_eq!(super::tunnels_list(state.clone()).unwrap().len(), 1);
        let mut connection = TcpStream::connect(("127.0.0.1", listen_port)).unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        connection.write_all(b"ping").unwrap();
        let mut response = [0; 4];
        connection.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        let remote = super::tunnels_start(
            state.clone(),
            tunnel.profile_id.clone(),
            "remote".into(),
            remote_port,
            "127.0.0.1".into(),
            destination_port,
        )
        .unwrap();
        let mut remote_connection = TcpStream::connect(("127.0.0.1", remote_port)).unwrap();
        remote_connection
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        remote_connection.write_all(b"ping").unwrap();
        remote_connection.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        let dynamic = super::tunnels_start(
            state.clone(),
            tunnel.profile_id.clone(),
            "dynamic".into(),
            socks_port,
            "".into(),
            0,
        )
        .unwrap();
        let mut socks = TcpStream::connect(("127.0.0.1", socks_port)).unwrap();
        socks
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socks.write_all(&[5, 1, 0]).unwrap();
        let mut greeting = [0; 2];
        socks.read_exact(&mut greeting).unwrap();
        assert_eq!(greeting, [5, 0]);
        let mut request = vec![5, 1, 0, 1, 127, 0, 0, 1];
        request.extend(destination_port.to_be_bytes());
        socks.write_all(&request).unwrap();
        let mut socks_reply = [0; 10];
        socks.read_exact(&mut socks_reply).unwrap();
        assert_eq!(socks_reply[1], 0);
        socks.write_all(b"ping").unwrap();
        socks.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        receiver.join().unwrap();
        super::tunnels_stop(state.clone(), tunnel.id).unwrap();
        super::tunnels_stop(state.clone(), remote.id).unwrap();
        super::tunnels_stop(state.clone(), dynamic.id).unwrap();
        assert!(super::tunnels_list(state).unwrap().is_empty());
    }
}
