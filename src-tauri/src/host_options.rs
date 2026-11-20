//! Extra SSH settings per host: keepalive, agent forwarding, compression,
//! a proxy command, environment variables, a startup command, and a port
//! knock sequence sent before connecting. Stored in settings by profile.

use crate::{profiles::Profile, AppState};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::{
    net::{TcpStream, ToSocketAddrs, UdpSocket},
    time::Duration,
};
use tauri::{Manager, State};

#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HostOptions {
    /// Seconds between keepalive messages; 0 leaves SSH's default.
    pub keepalive_seconds: u32,
    pub agent_forwarding: bool,
    pub compression: bool,
    pub proxy_command: String,
    /// `NAME=value` lines sent with SetEnv (the server must accept them).
    pub environment: String,
    pub startup_command: String,
    /// Ports to knock, such as `7000, 8000, 9000/udp`.
    pub knock_ports: String,
    pub knock_delay_ms: u32,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Knock {
    port: u16,
    udp: bool,
}

pub(crate) fn parse_knocks(text: &str) -> Result<Vec<Knock>, String> {
    text.split([',', ' ', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (port, udp) = match part.to_ascii_lowercase().split_once('/') {
                Some((port, "udp")) => (port.to_string(), true),
                Some((port, "tcp")) => (port.to_string(), false),
                None => (part.to_string(), false),
                Some(_) => {
                    return Err(format!(
                        "{part}: write ports as 7000, 7000/tcp or 7000/udp."
                    ))
                }
            };
            match port.parse::<u16>() {
                Ok(port) if port > 0 => Ok(Knock { port, udp }),
                _ => Err(format!("{part} is not a port between 1 and 65535.")),
            }
        })
        .collect()
}

fn environment(text: &str) -> Result<Vec<(String, String)>, String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (name, value) = line
                .split_once('=')
                .ok_or_else(|| format!("{line}: write variables as NAME=value."))?;
            let valid = name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid || value.contains(['"', '\\']) || value.chars().any(char::is_control) {
                return Err(format!(
                    "{line}: use a plain variable name and a value without quotes or backslashes."
                ));
            }
            Ok((name.to_string(), value.to_string()))
        })
        .collect()
}

impl HostOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.keepalive_seconds > 3600 {
            return Err("Keep the keepalive interval at one hour or less.".into());
        }
        if self.proxy_command.chars().any(|c| c == '\n' || c == '\r')
            || self.startup_command.contains('\n')
        {
            return Err("Proxy and startup commands must be a single line.".into());
        }
        if self.knock_delay_ms > 10_000 {
            return Err("Keep the delay between knocks at 10 seconds or less.".into());
        }
        environment(&self.environment)?;
        parse_knocks(&self.knock_ports).map(drop)
    }

    /// `-o` options for every ssh command started for this host.
    pub fn ssh_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let mut option = |value: String| args.extend(["-o".to_string(), value]);
        if self.keepalive_seconds > 0 {
            option(format!("ServerAliveInterval={}", self.keepalive_seconds));
            option("ServerAliveCountMax=3".into());
        }
        if self.agent_forwarding {
            option("ForwardAgent=yes".into());
        }
        if self.compression {
            option("Compression=yes".into());
        }
        if !self.proxy_command.trim().is_empty() {
            option(format!("ProxyCommand={}", self.proxy_command.trim()));
        }
        let variables = environment(&self.environment).unwrap_or_default();
        if !variables.is_empty() {
            option(format!(
                "SetEnv={}",
                variables
                    .iter()
                    .map(|(name, value)| format!("{name}=\"{value}\""))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        args
    }

    /// Sends the knock sequence. Closed or filtered ports are expected.
    pub fn knock(&self, host: &str) {
        let Ok(knocks) = parse_knocks(&self.knock_ports) else {
            return;
        };
        if knocks.is_empty() {
            return;
        }
        let Some(ip) = (host, 0)
            .to_socket_addrs()
            .ok()
            .and_then(|mut addresses| addresses.next())
            .map(|address| address.ip())
        else {
            return;
        };
        let delay = Duration::from_millis(u64::from(if self.knock_delay_ms == 0 {
            300
        } else {
            self.knock_delay_ms
        }));
        for knock in knocks {
            let address = (ip, knock.port).into();
            if knock.udp {
                let local = if ip.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
                if let Ok(socket) = UdpSocket::bind(local) {
                    let _ = socket.send_to(b"", address);
                }
            } else {
                let _ = TcpStream::connect_timeout(&address, Duration::from_millis(250));
            }
            std::thread::sleep(delay);
        }
    }
}

fn key(profile_id: &str) -> String {
    format!("host_options:{profile_id}")
}

pub fn load(state: &AppState, profile_id: &str) -> HostOptions {
    state
        .db
        .lock()
        .ok()
        .and_then(|db| {
            db.query_row(
                "SELECT value FROM settings WHERE key=?1",
                [key(profile_id)],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
        })
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Options for a profile, looked up from anywhere that only has the profile.
pub fn for_profile(profile: &Profile) -> HostOptions {
    crate::APP
        .get()
        .map(|app| load(&app.state::<AppState>(), &profile.id))
        .unwrap_or_default()
}

/// The ssh `-o` options for this host, after knocking if it has a sequence.
pub fn prepare(profile: &Profile) -> Vec<String> {
    let options = for_profile(profile);
    options.knock(&profile.host);
    options.ssh_args()
}

#[tauri::command]
pub fn host_options_get(state: State<'_, AppState>, profile_id: String) -> HostOptions {
    load(&state, &profile_id)
}

#[tauri::command]
pub fn host_options_set(
    state: State<'_, AppState>,
    profile_id: String,
    options: HostOptions,
) -> Result<(), String> {
    options.validate()?;
    let db = state
        .db
        .lock()
        .map_err(|_| "Settings are busy. Try again.")?;
    if options == HostOptions::default() {
        db.execute("DELETE FROM settings WHERE key=?1", [key(&profile_id)])
    } else {
        db.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [key(&profile_id), serde_json::to_string(&options).map_err(|error| error.to_string())?],
        )
    }
    .map(drop)
    .map_err(|error| format!("Could not save the host options: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn builds_ssh_options() {
        let options = HostOptions {
            keepalive_seconds: 30,
            agent_forwarding: true,
            compression: true,
            proxy_command: "nc -X 5 -x proxy:1080 %h %p".into(),
            environment: "LANG=en_US.UTF-8\n# comment\nTEAM=ops".into(),
            ..HostOptions::default()
        };
        options.validate().unwrap();
        assert_eq!(
            options.ssh_args(),
            [
                "-o",
                "ServerAliveInterval=30",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "ForwardAgent=yes",
                "-o",
                "Compression=yes",
                "-o",
                "ProxyCommand=nc -X 5 -x proxy:1080 %h %p",
                "-o",
                "SetEnv=LANG=\"en_US.UTF-8\" TEAM=\"ops\""
            ]
        );
        assert!(HostOptions::default().ssh_args().is_empty());
        assert!(HostOptions {
            environment: "1BAD=x".into(),
            ..HostOptions::default()
        }
        .validate()
        .is_err());
        assert!(HostOptions {
            environment: "A=\"x\"".into(),
            ..HostOptions::default()
        }
        .validate()
        .is_err());
        assert!(HostOptions {
            proxy_command: "a\nb".into(),
            ..HostOptions::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn parses_and_sends_knocks_in_order() {
        assert_eq!(
            parse_knocks("7000, 8000/udp 9000/TCP").unwrap(),
            [
                Knock {
                    port: 7000,
                    udp: false
                },
                Knock {
                    port: 8000,
                    udp: true
                },
                Knock {
                    port: 9000,
                    udp: false
                }
            ]
        );
        assert!(parse_knocks("70000").is_err());
        assert!(parse_knocks("7000/icmp").is_err());
        let first = TcpListener::bind("127.0.0.1:0").unwrap();
        let second = UdpSocket::bind("127.0.0.1:0").unwrap();
        second
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let options = HostOptions {
            knock_ports: format!(
                "{}, {}/udp",
                first.local_addr().unwrap().port(),
                second.local_addr().unwrap().port()
            ),
            knock_delay_ms: 10,
            ..HostOptions::default()
        };
        options.knock("127.0.0.1");
        first.set_nonblocking(true).unwrap();
        assert!(first.accept().is_ok());
        assert!(second.recv_from(&mut [0_u8; 8]).is_ok());
    }
}
