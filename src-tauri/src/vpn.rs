use crate::AppState;
use serde::{Deserialize, Serialize};
use std::{
    net::{TcpStream, ToSocketAddrs},
    path::Path,
    time::Duration,
};
use tauri::State;

#[cfg(target_os = "macos")]
const SCUTIL: &str = "/usr/sbin/scutil";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnService {
    id: String,
    name: String,
    kind: String,
    state: String,
    enabled: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnCommandProfile {
    id: String,
    name: String,
    executable: String,
    connect_args: Vec<String>,
    disconnect_args: Vec<String>,
    status_args: Vec<String>,
    connected_text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnCommandStatus {
    profile: VpnCommandProfile,
    state: String,
}

fn read_command_profiles(db: &rusqlite::Connection) -> Result<Vec<VpnCommandProfile>, String> {
    use rusqlite::OptionalExtension;
    let value: Option<String> = db
        .query_row(
            "SELECT value FROM settings WHERE key='vpn_command_profiles'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("Could not load VPN commands: {error}"))?;
    value
        .map(|value| {
            serde_json::from_str(&value)
                .map_err(|error| format!("Saved VPN commands could not be read: {error}"))
        })
        .unwrap_or_else(|| Ok(Vec::new()))
}

fn write_command_profiles(
    db: &rusqlite::Connection,
    profiles: &[VpnCommandProfile],
) -> Result<(), String> {
    let value = serde_json::to_string(profiles)
        .map_err(|error| format!("Could not save VPN commands: {error}"))?;
    db.execute(
        "INSERT INTO settings (key, value) VALUES ('vpn_command_profiles', ?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [value],
    )
    .map_err(|error| format!("Could not save VPN commands: {error}"))?;
    Ok(())
}

fn validate_command_profile(profile: &VpnCommandProfile) -> Result<(), String> {
    if profile.name.trim().is_empty() {
        return Err("Give this VPN command a name.".into());
    }
    let path = Path::new(&profile.executable);
    if !path.is_absolute() || !path.is_file() {
        return Err(
            "Choose the absolute path of an installed VPN command or executable script.".into(),
        );
    }
    if profile
        .connect_args
        .iter()
        .chain(profile.disconnect_args.iter())
        .chain(profile.status_args.iter())
        .any(|arg| arg.contains('\0') || arg.contains('\n'))
    {
        return Err("VPN command arguments cannot contain line breaks or NUL characters.".into());
    }
    Ok(())
}

fn command_state(profile: &VpnCommandProfile) -> String {
    if profile.status_args.is_empty() {
        return "Unknown".into();
    }
    match crate::platform::command(&profile.executable)
        .args(&profile.status_args)
        .output()
    {
        Ok(output) if output.status.success() => {
            if profile.connected_text.is_empty()
                || String::from_utf8_lossy(&output.stdout).contains(&profile.connected_text)
            {
                "Connected".into()
            } else {
                "Disconnected".into()
            }
        }
        Ok(output) if output.status.code() == Some(1) => "Disconnected".into(),
        Ok(output) => format!("Unavailable: status command exited with {}", output.status),
        Err(error) => format!("Unavailable: {error}"),
    }
}

fn run_command(profile: &VpnCommandProfile, connect: bool) -> Result<(), String> {
    validate_command_profile(profile)?;
    let args = if connect {
        &profile.connect_args
    } else {
        &profile.disconnect_args
    };
    let output = crate::platform::command(&profile.executable)
        .args(args)
        .output()
        .map_err(|error| format!("Could not start {}: {error}", profile.name))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(if detail.trim().is_empty() {
            format!(
                "{} command exited with {}. Check its arguments and VPN configuration.",
                profile.name, output.status
            )
        } else {
            format!("{}: {}", profile.name, detail.trim())
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn vpn_commands_list(
    state: State<'_, AppState>,
) -> Result<Vec<VpnCommandStatus>, String> {
    let profiles = {
        let db = state.db.lock().map_err(|_| "Profile database is busy")?;
        read_command_profiles(&db)?
    };
    tauri::async_runtime::spawn_blocking(move || {
        profiles
            .into_iter()
            .map(|profile| VpnCommandStatus {
                state: command_state(&profile),
                profile,
            })
            .collect()
    })
    .await
    .map_err(|error| format!("VPN command status stopped unexpectedly: {error}"))
}

#[tauri::command]
pub fn vpn_commands_save(
    state: State<'_, AppState>,
    mut profile: VpnCommandProfile,
) -> Result<VpnCommandProfile, String> {
    validate_command_profile(&profile)?;
    let db = state.db.lock().map_err(|_| "Profile database is busy")?;
    let mut profiles = read_command_profiles(&db)?;
    if profile.id.is_empty() {
        profile.id = uuid::Uuid::new_v4().to_string();
    } else {
        uuid::Uuid::parse_str(&profile.id).map_err(|_| "Invalid VPN command profile ID.")?;
    }
    if let Some(existing) = profiles.iter_mut().find(|item| item.id == profile.id) {
        *existing = profile.clone();
    } else {
        profiles.push(profile.clone());
    }
    write_command_profiles(&db, &profiles)?;
    Ok(profile)
}

#[tauri::command]
pub fn vpn_commands_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "Profile database is busy")?;
    let mut profiles = read_command_profiles(&db)?;
    let before = profiles.len();
    profiles.retain(|profile| profile.id != id);
    if before == profiles.len() {
        return Err("This VPN command profile no longer exists.".into());
    }
    write_command_profiles(&db, &profiles)
}

#[tauri::command]
pub async fn vpn_commands_run(
    state: State<'_, AppState>,
    id: String,
    connect: bool,
) -> Result<VpnCommandStatus, String> {
    let profile = {
        let db = state.db.lock().map_err(|_| "Profile database is busy")?;
        read_command_profiles(&db)?
            .into_iter()
            .find(|profile| profile.id == id)
            .ok_or("This VPN command profile no longer exists.")?
    };
    tauri::async_runtime::spawn_blocking(move || {
        run_command(&profile, connect)?;
        Ok(VpnCommandStatus {
            state: command_state(&profile),
            profile,
        })
    })
    .await
    .map_err(|error| format!("VPN command stopped unexpectedly: {error}"))?
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_services(output: &str) -> Vec<VpnService> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let enabled = line.starts_with('*');
            let line = line.trim_start_matches('*').trim();
            let (state, rest) = line.strip_prefix('(')?.split_once(')')?;
            let mut parts = rest.trim().splitn(2, char::is_whitespace);
            let id = parts.next()?.trim();
            uuid::Uuid::parse_str(id).ok()?;
            let detail = parts.next()?.trim();
            let start = detail.find('"')?;
            let end = detail.rfind('"')?;
            if end <= start {
                return None;
            }
            let name = &detail[start + 1..end];
            let kind = detail[end + 1..]
                .trim()
                .strip_prefix('[')?
                .strip_suffix(']')?;
            let kind_lower = kind.to_ascii_lowercase();
            let is_vpn = ["vpn", "ipsec", "l2tp", "ikev2", "pptp"]
                .iter()
                .any(|protocol| kind_lower.contains(protocol));
            if !is_vpn || name.is_empty() {
                return None;
            }
            Some(VpnService {
                id: id.to_owned(),
                name: name.to_owned(),
                kind: kind.to_owned(),
                state: state.trim().to_owned(),
                enabled,
            })
        })
        .collect()
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn run_scutil(executable: &Path, args: &[&str]) -> Result<String, String> {
    let output = crate::platform::command(executable)
        .arg("--nc")
        .args(args)
        .output()
        .map_err(|error| format!("Could not run macOS VPN controls: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            "macOS could not complete the VPN request. Check the VPN service in System Settings."
                .into()
        } else {
            format!("macOS could not complete the VPN request: {detail}")
        });
    }
    String::from_utf8(output.stdout)
        .map_err(|_| "macOS returned VPN information Weave could not read.".into())
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn list_with(executable: &Path) -> Result<Vec<VpnService>, String> {
    Ok(parse_services(&run_scutil(executable, &["list"])?))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn change_with(executable: &Path, id: &str, connect: bool) -> Result<Vec<VpnService>, String> {
    let services = list_with(executable)?;
    let service = services
        .iter()
        .find(|service| service.id == id)
        .ok_or("This VPN is no longer configured in macOS. Refresh the list.")?;
    if connect && !service.enabled {
        return Err(format!(
            "{} is disabled in macOS Network Settings. Enable it there first.",
            service.name
        ));
    }
    run_scutil(executable, &[if connect { "start" } else { "stop" }, id])?;
    list_with(executable)
}

fn check_ssh(host: &str, port: u16) -> Result<String, String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("Enter the NAS host or VPN address before checking SSH.".into());
    }
    if port == 0 {
        return Err("Choose an SSH port between 1 and 65535.".into());
    }
    if host.chars().any(char::is_whitespace) || host.contains(['/', '@', '\\']) {
        return Err("Enter only a host name or IP address, without a URL or username.".into());
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let addresses: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|error| {
            format!("Could not find {host}. Check the address and VPN connection: {error}")
        })?
        .collect();
    if addresses.is_empty() {
        return Err(format!("No network address was found for {host}."));
    }
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, Duration::from_secs(4)) {
            Ok(_) => {
                return Ok(format!(
                    "{host} is reachable on SSH port {port}. Connect to verify your credentials."
                ));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(format!(
        "SSH is not reachable at {host}:{port}: {}. Check the VPN route, NAS SSH service, and firewall.",
        last_error.expect("at least one address was checked")
    ))
}

/// VPNs set up in Windows, from PowerShell's Get-VpnConnection: id, name,
/// tunnel type and status separated by tabs.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_windows_vpns(output: &str) -> Vec<VpnService> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.trim_end_matches('\r').split('\t');
            let id = parts.next()?.trim().trim_matches(|c| c == '{' || c == '}');
            uuid::Uuid::parse_str(id).ok()?;
            let name = parts.next()?.trim();
            let kind = parts.next().unwrap_or("VPN").trim();
            let state = match parts.next().unwrap_or("").trim() {
                "Connected" => "Connected",
                "Connecting" => "Connecting",
                _ => "Disconnected",
            };
            (!name.is_empty()).then(|| VpnService {
                id: id.to_ascii_lowercase(),
                name: name.into(),
                kind: if kind.is_empty() {
                    "VPN".into()
                } else {
                    kind.into()
                },
                state: state.into(),
                enabled: true,
            })
        })
        .collect()
}

/// VPN and WireGuard connections from `nmcli -t -f UUID,NAME,TYPE,STATE`.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn parse_nmcli(output: &str) -> Vec<VpnService> {
    output
        .lines()
        .filter_map(|line| {
            // nmcli escapes ':' in values as '\:'.
            let fields: Vec<String> = {
                let mut fields = vec![String::new()];
                let mut chars = line.chars();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => fields.last_mut()?.extend(chars.next()),
                        ':' => fields.push(String::new()),
                        _ => fields.last_mut()?.push(c),
                    }
                }
                fields
            };
            let [id, name, kind, state] = fields.as_slice() else {
                return None;
            };
            uuid::Uuid::parse_str(id).ok()?;
            if !matches!(kind.as_str(), "vpn" | "wireguard") {
                return None;
            }
            Some(VpnService {
                id: id.clone(),
                name: name.clone(),
                kind: if kind == "wireguard" {
                    "WireGuard".into()
                } else {
                    "VPN".into()
                },
                state: match state.as_str() {
                    "activated" => "Connected",
                    "activating" => "Connecting",
                    _ => "Disconnected",
                }
                .into(),
                enabled: true,
            })
        })
        .collect()
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn output_text(program: &str, args: &[&str], what: &str) -> Result<String, String> {
    let output = crate::platform::command(program)
        .args(args)
        .output()
        .map_err(|error| format!("Could not run the system VPN controls: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Could not {what}: {}",
            detail
                .trim()
                .lines()
                .last()
                .unwrap_or("the system gave no details")
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn system_list() -> Result<Vec<VpnService>, String> {
    #[cfg(target_os = "macos")]
    {
        list_with(Path::new(SCUTIL))
    }
    #[cfg(windows)]
    {
        Ok(parse_windows_vpns(&output_text(
            "powershell.exe",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-VpnConnection | ForEach-Object { [string]$_.Guid + \"`t\" + $_.Name + \"`t\" + $_.TunnelType + \"`t\" + $_.ConnectionStatus }",
            ],
            "list VPNs",
        )?))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if crate::platform::find_tool("nmcli").is_none() {
            return Ok(Vec::new());
        }
        Ok(parse_nmcli(&output_text(
            "nmcli",
            &["-t", "-f", "UUID,NAME,TYPE,STATE", "connection", "show"],
            "list VPNs",
        )?))
    }
}

fn system_change(id: &str, connect: bool) -> Result<Vec<VpnService>, String> {
    #[cfg(target_os = "macos")]
    {
        change_with(Path::new(SCUTIL), id, connect)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let service = system_list()?
            .into_iter()
            .find(|service| service.id == id)
            .ok_or("This VPN is no longer set up on this computer. Refresh the list.")?;
        #[cfg(windows)]
        {
            if connect {
                // The Windows dialog asks for the sign-in when none is saved.
                crate::platform::command("rasphone.exe")
                    .args(["-d", &service.name])
                    .status()
                    .map_err(|error| format!("Could not start {}: {error}", service.name))?;
            } else {
                output_text("rasdial", &[&service.name, "/disconnect"], "disconnect")?;
            }
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            output_text(
                "nmcli",
                &[
                    "connection",
                    if connect { "up" } else { "down" },
                    "uuid",
                    &service.id,
                ],
                if connect { "connect" } else { "disconnect" },
            )?;
        }
        system_list()
    }
}

#[tauri::command]
pub async fn vpn_list() -> Result<Vec<VpnService>, String> {
    tauri::async_runtime::spawn_blocking(system_list)
        .await
        .map_err(|error| format!("VPN status stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn vpn_connect(id: String) -> Result<Vec<VpnService>, String> {
    tauri::async_runtime::spawn_blocking(move || system_change(&id, true))
        .await
        .map_err(|error| format!("VPN connection stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn vpn_disconnect(id: String) -> Result<Vec<VpnService>, String> {
    tauri::async_runtime::spawn_blocking(move || system_change(&id, false))
        .await
        .map_err(|error| format!("VPN disconnection stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn vpn_check_ssh(host: String, port: u16) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || check_ssh(&host, port))
        .await
        .map_err(|error| format!("SSH reachability check stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    #[cfg(unix)]
    use std::{fs, os::unix::fs::PermissionsExt};

    const SAMPLE: &str = "Available network connection services in the current set (*=enabled):\n* (Connected)  F618B796-5DA3-471C-93DE-7C8CBACF03AA IPSec \"Office VPN\" [IPSec]\n* (Disconnected) 8C1564BE-508B-43F7-8B31-C56B4F570AE1 VPN (com.wireguard.macos) \"Home WireGuard\" [VPN:com.wireguard.macos]\n  (Disconnected) 9EC220EC-8F27-4D3E-A8D8-1BF5BE916751 VPN (com.example.vpn) \"Disabled VPN\" [VPN:com.example.vpn]\n  (Disconnected) 22EE536E-E5A9-40B8-85F1-65D9E9B6A778 PPP \"Old modem\" [PPP:Modem]\n";

    #[test]
    fn reads_registered_vpns_and_skips_modems() {
        let services = parse_services(SAMPLE);
        assert_eq!(services.len(), 3);
        assert_eq!(services[0].name, "Office VPN");
        assert_eq!(services[0].state, "Connected");
        assert_eq!(services[1].kind, "VPN:com.wireguard.macos");
        assert!(services[1].enabled);
        assert!(!services[2].enabled);
    }

    #[test]
    fn reads_windows_and_networkmanager_vpns() {
        let windows = parse_windows_vpns(
            "{8C1564BE-508B-43F7-8B31-C56B4F570AE1}\tOffice\tIkev2\tConnected\r\nbad\tline\n",
        );
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "8c1564be-508b-43f7-8b31-c56b4f570ae1");
        assert_eq!(windows[0].state, "Connected");
        let linux = parse_nmcli(
            "8C1564BE-508B-43F7-8B31-C56B4F570AE1:Home\\: WG:wireguard:activated\n9EC220EC-8F27-4D3E-A8D8-1BF5BE916751:Wired:802-3-ethernet:activated\n",
        );
        assert_eq!(linux.len(), 1);
        assert_eq!(linux[0].name, "Home: WG");
        assert_eq!(linux[0].state, "Connected");
    }

    #[test]
    fn handles_empty_and_malformed_lines() {
        assert!(parse_services("Available network connection services:\n").is_empty());
        assert!(parse_services("* (Connected) fake \"VPN\" [IPSec]").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn actions_use_validated_service_id() {
        let folder = std::env::temp_dir().join(format!("weave-vpn-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&folder).unwrap();
        let executable = folder.join("scutil");
        let calls = folder.join("calls");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$2\" = list ]; then cat <<'EOF'\n{SAMPLE}EOF\nfi\n",
            calls.display()
        );
        fs::write(&executable, script).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let id = "F618B796-5DA3-471C-93DE-7C8CBACF03AA";
        change_with(&executable, id, true).unwrap();
        assert!(fs::read_to_string(&calls)
            .unwrap()
            .contains(&format!("--nc start {id}")));
        assert!(change_with(&executable, "bad; rm -rf /", true).is_err());
        assert!(
            change_with(&executable, "9EC220EC-8F27-4D3E-A8D8-1BF5BE916751", true)
                .unwrap_err()
                .contains("disabled")
        );
        assert!(!fs::read_to_string(&calls).unwrap().contains("rm -rf"));
        assert!(!fs::read_to_string(&calls)
            .unwrap()
            .contains("start 9EC220EC-8F27-4D3E-A8D8-1BF5BE916751"));
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn checks_host_validation_and_reachability() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(check_ssh("127.0.0.1", port).unwrap().contains("reachable"));
        assert!(check_ssh("", port).is_err());
        assert!(check_ssh("http://localhost", port).is_err());
        assert!(check_ssh("user@localhost", port).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn saves_and_controls_a_custom_vpn_command() {
        let folder = std::env::temp_dir().join(format!("weave-vpn-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&folder).unwrap();
        let executable = folder.join("vpnctl");
        let marker = folder.join("connected");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  connect) touch '{}' ;;\n  disconnect) rm -f '{}' ;;\n  status) test -f '{}' && echo Connected ;;\nesac\n",
                marker.display(),
                marker.display(),
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let profile = VpnCommandProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Other VPN".into(),
            executable: executable.to_string_lossy().into_owned(),
            connect_args: vec!["connect".into()],
            disconnect_args: vec!["disconnect".into()],
            status_args: vec!["status".into()],
            connected_text: "Connected".into(),
        };
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        validate_command_profile(&profile).unwrap();
        write_command_profiles(&db, std::slice::from_ref(&profile)).unwrap();
        let saved = read_command_profiles(&db).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(command_state(&saved[0]), "Disconnected");
        run_command(&saved[0], true).unwrap();
        assert_eq!(command_state(&saved[0]), "Connected");
        run_command(&saved[0], false).unwrap();
        assert_eq!(command_state(&saved[0]), "Disconnected");
        fs::remove_dir_all(folder).unwrap();
    }
}
