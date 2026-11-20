use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

const INSTALL_URL: &str = "https://tailscale.com/download/mac";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailnetStatus {
    installed: bool,
    state: String,
    device_name: String,
    peers: Vec<TailnetPeer>,
    install_url: &'static str,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailnetPeer {
    name: String,
    dns_name: String,
    ip: String,
    online: bool,
}

#[derive(Deserialize)]
struct CliStatus {
    #[serde(rename = "BackendState", default)]
    backend_state: String,
    #[serde(rename = "Self")]
    self_node: Option<CliNode>,
    #[serde(rename = "Peer", default)]
    peers: Option<HashMap<String, CliNode>>,
}

#[derive(Deserialize)]
struct CliNode {
    #[serde(rename = "HostName", default)]
    host_name: String,
    #[serde(rename = "DNSName", default)]
    dns_name: String,
    #[serde(rename = "TailscaleIPs", default)]
    ips: Vec<String>,
    #[serde(rename = "Online", default)]
    online: bool,
}

fn cli_path() -> Option<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale"),
        PathBuf::from("/usr/local/bin/tailscale"),
        PathBuf::from("/opt/homebrew/bin/tailscale"),
        PathBuf::from("/usr/bin/tailscale"),
    ];
    candidates.extend(
        dirs::home_dir()
            .map(|home| home.join("Applications/Tailscale.app/Contents/MacOS/Tailscale")),
    );
    for base in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(base) {
            candidates.push(PathBuf::from(base).join("Tailscale").join("tailscale.exe"));
        }
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| crate::platform::find_tool("tailscale"))
}

fn run_cli(executable: &Path, args: &[&str]) -> Result<String, String> {
    let output = crate::platform::command(executable)
        .args(args)
        .env("TAILSCALE_BE_CLI", "1")
        .output()
        .map_err(|error| format!("Could not start Tailscale: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        return Err(if detail.is_empty() {
            "Tailscale could not complete the request. Open Tailscale and check its connection."
                .into()
        } else {
            format!("Tailscale could not complete the request: {detail}")
        });
    }
    String::from_utf8(output.stdout)
        .map_err(|_| "Tailscale returned a response Weave could not read.".into())
}

fn peer(node: CliNode) -> Option<TailnetPeer> {
    let ip = node
        .ips
        .iter()
        .find(|ip| ip.parse::<IpAddr>().is_ok_and(|addr| addr.is_ipv4()))
        .or_else(|| node.ips.first())?
        .to_owned();
    let dns_name = node.dns_name.trim_end_matches('.').to_owned();
    let name = if node.host_name.is_empty() {
        dns_name.split('.').next().unwrap_or(&ip).to_owned()
    } else {
        node.host_name
    };
    Some(TailnetPeer {
        name,
        dns_name,
        ip,
        online: node.online,
    })
}

fn parse_status(output: &str) -> Result<TailnetStatus, String> {
    let status: CliStatus = serde_json::from_str(output).map_err(|_| {
        "Tailscale returned a status Weave could not read. Update Tailscale and try again."
    })?;
    let mut peers: Vec<_> = status
        .peers
        .unwrap_or_default()
        .into_values()
        .filter_map(peer)
        .collect();
    peers.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(TailnetStatus {
        installed: true,
        state: if status.backend_state.is_empty() {
            "Unknown".into()
        } else {
            status.backend_state
        },
        device_name: status
            .self_node
            .map(|node| node.host_name)
            .unwrap_or_default(),
        peers,
        install_url: INSTALL_URL,
    })
}

fn status_with(executable: Option<&Path>) -> Result<TailnetStatus, String> {
    let Some(executable) = executable else {
        return Ok(TailnetStatus {
            installed: false,
            state: "Not installed".into(),
            device_name: String::new(),
            peers: Vec::new(),
            install_url: INSTALL_URL,
        });
    };
    parse_status(&run_cli(executable, &["status", "--json"])?)
}

fn check_ssh(status: &TailnetStatus, ip: &str, port: u16) -> Result<String, String> {
    if port == 0 {
        return Err("Choose an SSH port between 1 and 65535.".into());
    }
    let address: IpAddr = ip
        .parse()
        .map_err(|_| "Choose a Tailscale device from the list.".to_string())?;
    if status.state != "Running" {
        return Err("Connect Tailscale on this computer before checking the SSH port.".into());
    }
    let selected = status
        .peers
        .iter()
        .find(|peer| peer.ip == ip)
        .ok_or("This device is not in your current tailnet. Refresh the device list.")?;
    if !selected.online {
        return Err(format!("{} is offline in Tailscale.", selected.name));
    }
    TcpStream::connect_timeout(&SocketAddr::new(address, port), Duration::from_secs(4))
        .map_err(|error| {
            format!(
                "{} is on your tailnet, but SSH is not reachable on port {port}: {error}. Check that SSH is enabled on the NAS and allowed by your tailnet policy.",
                selected.name
            )
        })?;
    Ok(format!(
        "{} is reachable on SSH port {port}. Connect to verify your SSH credentials.",
        selected.name
    ))
}

#[tauri::command]
pub async fn tailnet_status() -> Result<TailnetStatus, String> {
    tauri::async_runtime::spawn_blocking(|| status_with(cli_path().as_deref()))
        .await
        .map_err(|error| format!("Tailscale status stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn tailnet_connect() -> Result<TailnetStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let executable = cli_path()
            .ok_or("Install Tailscale from tailscale.com, then sign in before connecting.")?;
        run_cli(&executable, &["up", "--timeout=15s"]).map_err(|error| {
            format!("{error} If sign-in is needed, finish it in Tailscale and refresh here.")
        })?;
        status_with(Some(&executable))
    })
    .await
    .map_err(|error| format!("Tailscale connection stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn tailnet_disconnect() -> Result<TailnetStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let executable = cli_path().ok_or("Tailscale is not installed on this computer.")?;
        run_cli(&executable, &["down"])?;
        status_with(Some(&executable))
    })
    .await
    .map_err(|error| format!("Tailscale disconnection stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn tailnet_check_ssh(ip: String, port: u16) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let status = status_with(cli_path().as_deref())?;
        check_ssh(&status, &ip, port)
    })
    .await
    .map_err(|error| format!("SSH reachability check stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    #[cfg(unix)]
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn parses_device_list_and_chooses_usable_ips() {
        let status = parse_status(r#"{
            "BackendState": "Running",
            "Self": {"HostName": "macbook", "TailscaleIPs": ["100.80.0.1"]},
            "Peer": {
                "node-a": {"HostName": "nas", "DNSName": "nas.tailnet.ts.net.", "TailscaleIPs": ["100.80.0.2", "fd7a::2"], "Online": true},
                "node-b": {"HostName": "backup", "TailscaleIPs": ["100.80.0.3"], "Online": false}
            }
        }"#).unwrap();
        assert_eq!(status.state, "Running");
        assert_eq!(status.device_name, "macbook");
        assert_eq!(status.peers[0].name, "backup");
        assert_eq!(status.peers[1].ip, "100.80.0.2");
        assert_eq!(status.peers[1].dns_name, "nas.tailnet.ts.net");
        assert!(status.peers[1].online);
    }

    #[test]
    fn explains_missing_client() {
        let status = status_with(None).unwrap();
        assert!(!status.installed);
        assert_eq!(status.state, "Not installed");
    }

    #[test]
    fn accepts_a_tailnet_without_peers() {
        let status = parse_status(r#"{"BackendState":"Running","Peer":null}"#).unwrap();
        assert!(status.peers.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn reads_status_from_the_macos_cli() {
        let script = std::env::temp_dir().join(format!("weave-tailnet-{}", uuid::Uuid::new_v4()));
        fs::write(
            &script,
            "#!/bin/sh\n[ \"$TAILSCALE_BE_CLI\" = 1 ] && [ \"$1\" = status ] && [ \"$2\" = --json ] || exit 2\nprintf '{\"BackendState\":\"Running\",\"Peer\":{}}'\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let status = status_with(Some(&script)).unwrap();
        fs::remove_file(script).unwrap();
        assert_eq!(status.state, "Running");
    }

    #[test]
    fn checks_only_visible_online_peers_and_the_requested_port() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut status = parse_status(
            r#"{"BackendState":"Running","Peer":{"nas":{"HostName":"nas","TailscaleIPs":["127.0.0.1"],"Online":true}}}"#,
        )
        .unwrap();
        assert!(check_ssh(&status, "127.0.0.1", port)
            .unwrap()
            .contains("reachable"));
        assert!(check_ssh(&status, "127.0.0.2", port)
            .unwrap_err()
            .contains("not in your current tailnet"));
        status.peers[0].online = false;
        assert!(check_ssh(&status, "127.0.0.1", port)
            .unwrap_err()
            .contains("offline"));
    }
}
