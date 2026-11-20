//! Imports saved sessions from WinSCP, PuTTY, MobaXterm, Cyberduck,
//! SecureCRT and Termius CSV files. Passwords are never imported; Weave asks
//! for them when connecting. Telnet, RDP and VNC sessions become saved
//! connections instead of hosts.

use crate::{
    profiles::{self, FileZillaImportReport, Profile},
    AppState,
};
use quick_xml::{events::Event, Reader};
use serde::Serialize;
use std::{collections::HashMap, fs};
use tauri::State;

const MAX_IMPORT_BYTES: u64 = 5 * 1024 * 1024;

/// A Telnet, RDP or VNC session, saved in the app's Connect list.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportedConnection {
    kind: String,
    name: String,
    host: String,
    port: u16,
    #[serde(skip_serializing_if = "String::is_empty")]
    username: String,
}

#[derive(Default)]
pub(crate) struct Found {
    profiles: Vec<Option<Profile>>,
    connections: Vec<ImportedConnection>,
}

impl Found {
    fn profiles(profiles: Vec<Option<Profile>>) -> Self {
        Self {
            profiles,
            connections: Vec::new(),
        }
    }

    /// Sorts a session by protocol into a host profile or a saved connection.
    fn add(
        &mut self,
        name: &str,
        protocol: &str,
        host: &str,
        port: Option<u16>,
        user: &str,
        key: Option<&str>,
    ) {
        let name = name.trim();
        let host = host.trim();
        let protocol = protocol.trim().to_ascii_lowercase();
        let entry = match protocol.as_str() {
            "ssh" | "ssh2" | "ssh1" | "sftp" | "mosh" => Some(("ssh", 22)),
            "ftp" => Some(("ftp", 21)),
            "ftps" => Some(("ftps", 21)),
            "telnet" | "rdp" | "vnc" => {
                let default = match protocol.as_str() {
                    "telnet" => 23,
                    "rdp" => 3389,
                    _ => 5900,
                };
                let valid = !name.is_empty()
                    && !host.is_empty()
                    && !host.starts_with('-')
                    && host
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || ".-_:[]".contains(c));
                if valid {
                    self.connections.push(ImportedConnection {
                        kind: protocol.clone(),
                        name: name.chars().take(80).collect(),
                        host: host.into(),
                        port: port.filter(|port| *port > 0).unwrap_or(default),
                        username: user.trim().into(),
                    });
                } else {
                    self.profiles.push(None);
                }
                None
            }
            _ => {
                self.profiles.push(None);
                None
            }
        };
        if let Some((kind, default)) = entry {
            self.profiles.push(profile(
                name,
                host,
                kind,
                port.filter(|port| *port > 0).unwrap_or(default),
                user,
                key,
            ));
        }
    }
}
fn profile(
    name: &str,
    host: &str,
    protocol: &str,
    port: u16,
    user: &str,
    key: Option<&str>,
) -> Option<Profile> {
    let username = match (protocol, user.trim()) {
        ("ftp" | "ftps" | "ftps-implicit", "") => "anonymous",
        (_, user) => user,
    };
    let key_path = key
        .map(str::trim)
        // Windows key paths from WinSCP or PuTTY cannot be used on a Mac.
        .filter(|key| !key.is_empty() && !key.contains('\\') && !key.contains(':'))
        .filter(|_| protocol == "ssh")
        .map(String::from);
    let profile = Profile {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.trim().chars().take(80).collect(),
        host: host.trim().into(),
        protocol: protocol.into(),
        port,
        username: username.into(),
        key_path,
        jump_host: None,
        color: "#9d91ec".into(),
        favorite: false,
        tls_pin_sha256: None,
        s3_region: None,
    };
    (!profile.name.is_empty()
        && !profile.username.is_empty()
        && profiles::validate_profile(&profile).is_ok())
    .then_some(profile)
}

fn percent_decode(value: &str) -> String {
    percent_encoding::percent_decode_str(value)
        .decode_utf8_lossy()
        .into_owned()
}

/// Splits an INI or .reg file into `(section, [(key, value)])` pairs.
fn ini_sections(text: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut sections: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            sections.push((name.to_string(), Vec::new()));
        } else if let (Some((key, value)), Some(section)) =
            (line.split_once('='), sections.last_mut())
        {
            section
                .1
                .push((key.trim().to_string(), value.trim().to_string()));
        }
    }
    sections
}

fn value<'a>(entries: &'a [(String, String)], key: &str) -> Option<&'a str> {
    entries
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

pub(crate) fn winscp_profiles(text: &str) -> Vec<Option<Profile>> {
    ini_sections(text)
        .into_iter()
        .filter_map(|(section, entries)| {
            let name = section.strip_prefix("Sessions\\")?;
            if name == "Default%20Settings" {
                return None;
            }
            let name = percent_decode(name.rsplit('/').next().unwrap_or(name));
            let host = percent_decode(value(&entries, "HostName").unwrap_or(""));
            let user = percent_decode(value(&entries, "UserName").unwrap_or(""));
            let key = value(&entries, "PublicKeyFile").map(percent_decode);
            let ftps = value(&entries, "Ftps").unwrap_or("0");
            let protocol = match value(&entries, "FSProtocol").unwrap_or("2") {
                "0" | "1" | "2" => "ssh",
                "5" if ftps == "1" => "ftps-implicit",
                "5" if ftps == "0" => "ftp",
                "5" => "ftps",
                _ => return Some(None),
            };
            let default_port = match protocol {
                "ssh" => 22,
                "ftps-implicit" => 990,
                _ => 21,
            };
            let port = value(&entries, "PortNumber")
                .and_then(|port| port.parse().ok())
                .unwrap_or(default_port);
            Some(profile(&name, &host, protocol, port, &user, key.as_deref()))
        })
        .collect()
}

fn reg_string(value: &str) -> String {
    value
        .trim_matches('"')
        .replace("\\\\", "\\")
        .replace("\\\"", "\"")
}

pub(crate) fn putty_profiles(text: &str) -> Vec<Option<Profile>> {
    ini_sections(text)
        .into_iter()
        .filter_map(|(section, entries)| {
            let name = section.split("\\PuTTY\\Sessions\\").nth(1)?;
            if name == "Default%20Settings" {
                return None;
            }
            let entry = |key: &str| value(&entries, &format!("\"{key}\"")).map(reg_string);
            if entry("Protocol").as_deref().unwrap_or("ssh") != "ssh" {
                return Some(None);
            }
            let mut host = entry("HostName").unwrap_or_default();
            let mut user = entry("UserName").unwrap_or_default();
            if let Some((login, address)) = host.clone().split_once('@') {
                user = login.to_string();
                host = address.to_string();
            }
            let port = value(&entries, "\"PortNumber\"")
                .and_then(|port| port.strip_prefix("dword:"))
                .and_then(|hex| u16::from_str_radix(hex, 16).ok())
                .unwrap_or(22);
            let key = entry("PublicKeyFile");
            Some(profile(
                &percent_decode(name),
                &host,
                "ssh",
                port,
                &user,
                key.as_deref(),
            ))
        })
        .collect()
}

/// MobaXterm stores sessions as `name=#icon#type%host%port%user%…`. Types:
/// 0 SSH, 1 Telnet, 4 RDP, 5 VNC, 6 FTP, 7 SFTP, 12 Mosh; others are skipped.
pub(crate) fn mobaxterm_sessions(text: &str) -> Found {
    let mut found = Found::default();
    let bookmarks = ini_sections(text)
        .into_iter()
        .filter(|(section, _)| section.starts_with("Bookmarks"))
        .flat_map(|(_, entries)| entries)
        .filter(|(key, _)| key != "SubRep" && key != "ImgNum");
    for (name, entry) in bookmarks {
        let fields: Vec<&str> = entry
            .splitn(3, '#')
            .nth(2)
            .unwrap_or("")
            .split('%')
            .collect();
        if fields.len() < 3 {
            found.profiles.push(None);
            continue;
        }
        let protocol = match fields[0] {
            "0" => "ssh",
            "1" => "telnet",
            "4" => "rdp",
            "5" => "vnc",
            "6" => "ftp",
            "7" => "sftp",
            "12" => "mosh",
            _ => "unsupported",
        };
        let user = fields.get(3).copied().unwrap_or("");
        found.add(
            &name,
            protocol,
            fields[1],
            fields[2].parse().ok(),
            user,
            None,
        );
    }
    found
}

/// Termius imports hosts from a CSV template; teams often keep host lists in
/// the same shape. Columns are recognized by name.
pub(crate) fn csv_sessions(text: &str) -> Result<Found, String> {
    let rows = parse_csv(text);
    let header: Vec<String> = rows
        .first()
        .ok_or("The CSV file is empty.")?
        .iter()
        .map(|cell| cell.trim().to_ascii_lowercase())
        .collect();
    let column = |names: &[&str]| {
        header
            .iter()
            .position(|cell| names.iter().any(|name| cell == name || cell.contains(name)))
    };
    let host = column(&["hostname", "host", "address", "ip"])
        .ok_or("The CSV file needs a Hostname or Address column.")?;
    let name = column(&["label", "name", "alias"]);
    let port = column(&["port"]);
    let user = column(&["username", "user", "login"]);
    let protocol = column(&["protocol", "type"]);
    let mut found = Found::default();
    for row in rows
        .iter()
        .skip(1)
        .filter(|row| row.iter().any(|cell| !cell.trim().is_empty()))
    {
        let cell = |index: Option<usize>| {
            index
                .and_then(|index| row.get(index))
                .map(String::as_str)
                .unwrap_or("")
        };
        let address = cell(Some(host));
        let label = if cell(name).trim().is_empty() {
            address
        } else {
            cell(name)
        };
        let kind = if cell(protocol).trim().is_empty() {
            "ssh"
        } else {
            cell(protocol)
        };
        found.add(
            label,
            kind,
            address,
            cell(port).trim().parse().ok(),
            cell(user),
            None,
        );
    }
    Ok(found)
}

fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (true, c) => cell.push(c),
            (false, '"') => quoted = true,
            (false, ',' | ';') => row.push(std::mem::take(&mut cell)),
            (false, '\n') => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            (false, '\r') => {}
            (false, c) => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// SecureCRT "Export Settings" XML: sessions are keys that hold a Hostname.
pub(crate) fn securecrt_xml(xml: &str) -> Found {
    let mut reader = Reader::from_str(xml);
    let mut keys: Vec<(String, HashMap<String, String>)> = Vec::new();
    let mut value_name: Option<String> = None;
    let mut text = String::new();
    let mut found = Found::default();
    let name_of = |event: &quick_xml::events::BytesStart| {
        event
            .attributes()
            .flatten()
            .find(|attribute| attribute.key.as_ref() == b"name")
            .and_then(|attribute| {
                attribute
                    .normalized_value(quick_xml::XmlVersion::default())
                    .ok()
                    .map(|value| value.into_owned())
            })
            .unwrap_or_default()
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if event.local_name().as_ref() == b"key" => {
                keys.push((name_of(&event), HashMap::new()))
            }
            Ok(Event::Start(event)) => {
                value_name = Some(name_of(&event));
                text.clear();
            }
            Ok(Event::Text(part)) => {
                if let Ok(decoded) = part.decode() {
                    text.push_str(
                        &quick_xml::escape::unescape(&decoded)
                            .map(|value| value.into_owned())
                            .unwrap_or_default(),
                    );
                }
            }
            Ok(Event::GeneralRef(entity)) => {
                if let Some(resolved) = entity
                    .decode()
                    .ok()
                    .and_then(|name| quick_xml::escape::resolve_xml_entity(&name))
                {
                    text.push_str(resolved);
                }
            }
            Ok(Event::End(event)) if event.local_name().as_ref() == b"key" => {
                let Some((name, values)) = keys.pop() else {
                    continue;
                };
                let in_sessions = keys.iter().any(|(parent, _)| parent == "Sessions");
                let host = values.get("Hostname").map(String::as_str).unwrap_or("");
                if in_sessions && !host.is_empty() && name != "Default" {
                    securecrt_session(&mut found, &name, &values);
                }
            }
            Ok(Event::End(_)) => {
                if let (Some(name), Some((_, values))) = (value_name.take(), keys.last_mut()) {
                    values.insert(name, std::mem::take(&mut text));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    found
}

fn securecrt_session(found: &mut Found, name: &str, values: &HashMap<String, String>) {
    let get = |key: &str| values.get(key).map(String::as_str).unwrap_or("");
    let protocol = get("Protocol Name");
    let port_key = match protocol.to_ascii_uppercase().as_str() {
        "SSH2" => "[SSH2] Port",
        "SSH1" => "[SSH1] Port",
        _ => "Port",
    };
    let port = u16::from_str_radix(
        get(port_key).trim_start_matches("0x"),
        if get(port_key).len() == 8 { 16 } else { 10 },
    )
    .ok()
    .or_else(|| get(port_key).parse().ok());
    let key = get("Identity Filename V2")
        .split('*')
        .next()
        .filter(|key| !key.is_empty());
    found.add(name, protocol, get("Hostname"), port, get("Username"), key);
}

/// A SecureCRT session .ini file, with lines like `S:"Hostname"=web` and
/// `D:"[SSH2] Port"=00000016` (hexadecimal).
pub(crate) fn securecrt_ini(name: &str, text: &str) -> Found {
    let mut values = HashMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(key) = key.get(2..).map(|key| key.trim_matches('"')) else {
            continue;
        };
        let value = if line.starts_with("D:") {
            u32::from_str_radix(value.trim(), 16)
                .map(|number| number.to_string())
                .unwrap_or_default()
        } else {
            value.trim().to_string()
        };
        values.insert(key.to_string(), value);
    }
    let mut found = Found::default();
    if values.get("Hostname").is_some_and(|host| !host.is_empty()) {
        securecrt_session(&mut found, name, &values);
    }
    found
}

pub(crate) fn cyberduck_profile(xml: &str) -> Option<Profile> {
    let mut reader = Reader::from_str(xml);
    let mut text = String::new();
    let mut key = String::new();
    let mut fields = std::collections::HashMap::new();
    loop {
        match reader.read_event().ok()? {
            Event::Start(_) => text.clear(),
            Event::Text(part) => {
                text.push_str(&quick_xml::escape::unescape(&part.decode().ok()?).ok()?)
            }
            Event::GeneralRef(entity) => {
                let name = entity.decode().ok()?;
                text.push_str(quick_xml::escape::resolve_xml_entity(&name)?);
            }
            Event::End(tag) if tag.local_name().as_ref() == b"key" => {
                key = std::mem::take(&mut text);
            }
            Event::End(tag) if tag.local_name().as_ref() == b"string" && !key.is_empty() => {
                fields.insert(std::mem::take(&mut key), std::mem::take(&mut text));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let field = |name: &str| fields.get(name).map(String::as_str).unwrap_or("");
    let host = field("Hostname");
    let name = if field("Nickname").is_empty() {
        host
    } else {
        field("Nickname")
    };
    let port = field("Port").parse().ok();
    let user = field("Username");
    match field("Protocol") {
        "sftp" => profile(
            name,
            host,
            "ssh",
            port.unwrap_or(22),
            user,
            Some(field("Private Key File")),
        ),
        "ftp" => profile(name, host, "ftp", port.unwrap_or(21), user, None),
        "ftps" => profile(name, host, "ftps", port.unwrap_or(21), user, None),
        "davs" => {
            let port = port.unwrap_or(443);
            let path = field("Path").trim_start_matches('/');
            let url = format!("https://{host}:{port}/{path}");
            profile(name, &url, "webdav", port, user, None)
        }
        _ => None,
    }
}

fn read_text(path: &str) -> Result<String, String> {
    let size = fs::metadata(path)
        .map_err(|error| format!("Could not open {path}: {error}"))?
        .len();
    if size > MAX_IMPORT_BYTES {
        return Err(format!("{path} is larger than 5 MB."));
    }
    let bytes = fs::read(path).map_err(|error| format!("Could not read {path}: {error}"))?;
    // Registry exports from Windows are usually UTF-16 with a byte order mark.
    if let Some(utf16) = bytes.strip_prefix(&[0xff, 0xfe]) {
        let units: Vec<u16> = utf16
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        return Ok(String::from_utf16_lossy(&units));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn parse_file(path: &str, text: &str) -> Result<Found, String> {
    let lower = path.to_lowercase();
    if lower.ends_with(".duck") {
        return Ok(Found::profiles(vec![cyberduck_profile(text)]));
    }
    if lower.ends_with(".mxtsessions") || lower.ends_with(".mobaconf") {
        return Ok(mobaxterm_sessions(text));
    }
    if lower.ends_with(".csv") {
        return csv_sessions(text);
    }
    if text.contains("<VanDyke") {
        return Ok(securecrt_xml(text));
    }
    if text.contains("S:\"Hostname\"=") {
        let name = std::path::Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("SecureCRT session");
        return Ok(securecrt_ini(name, text));
    }
    if text.contains("\\SimonTatham\\PuTTY\\Sessions\\") {
        return Ok(Found::profiles(putty_profiles(text)));
    }
    if text.contains("[Sessions\\") {
        return Ok(Found::profiles(winscp_profiles(text)));
    }
    Err(format!(
        "{path} is not a session file Weave can read. Use WinSCP.ini, a PuTTY .reg export, MobaXterm .mxtsessions or .mobaconf, a Cyberduck .duck, a SecureCRT .xml export or session .ini, or a Termius CSV template."
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionImportReport {
    imported: usize,
    skipped: usize,
    connections: Vec<ImportedConnection>,
}

#[tauri::command]
pub fn profiles_import_sessions(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<SessionImportReport, String> {
    if paths.is_empty() || paths.len() > 500 {
        return Err("Choose between 1 and 500 files to import.".into());
    }
    let mut found = Found::default();
    for path in &paths {
        let file = parse_file(path, &read_text(path)?)?;
        found.profiles.extend(file.profiles);
        found.connections.extend(file.connections);
    }
    if found.profiles.is_empty() && found.connections.is_empty() {
        return Err("No saved sessions were found in the chosen files.".into());
    }
    let report = if found.profiles.is_empty() {
        FileZillaImportReport {
            imported: 0,
            skipped: 0,
        }
    } else {
        let mut db = state.db.lock().map_err(|_| "Profile database is busy")?;
        profiles::insert_imported_profiles(&mut db, found.profiles)?
    };
    Ok(SessionImportReport {
        imported: report.imported,
        skipped: report.skipped,
        connections: found.connections,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(profiles: &[Option<Profile>]) -> Vec<String> {
        profiles
            .iter()
            .map(|item| match item {
                Some(profile) => format!(
                    "{}|{}|{}|{}|{}",
                    profile.name, profile.protocol, profile.host, profile.port, profile.username
                ),
                None => "skipped".into(),
            })
            .collect()
    }

    #[test]
    fn reads_winscp_sessions() {
        let ini = "[Sessions\\Default%20Settings]\nHostName=x\n\
[Sessions\\Work/My%20Server]\nHostName=example.test\nPortNumber=2222\nUserName=alice\nPublicKeyFile=C:%5Ckeys%5Cid.ppk\n\
[Sessions\\Files]\nHostName=ftp.example.test\nFSProtocol=5\nFtps=3\nUserName=bob\n\
[Sessions\\Bucket]\nHostName=s3.amazonaws.com\nFSProtocol=7\n";
        let profiles = winscp_profiles(ini);
        assert_eq!(
            names(&profiles),
            [
                "My Server|ssh|example.test|2222|alice",
                "Files|ftps|ftp.example.test|21|bob",
                "skipped"
            ]
        );
        assert_eq!(profiles[0].as_ref().unwrap().key_path, None);
    }

    #[test]
    fn reads_putty_registry_exports() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\Build%20box]\r\n\
\"HostName\"=\"deploy@build.example.test\"\r\n\"PortNumber\"=dword:00000016\r\n\"Protocol\"=\"ssh\"\r\n\r\n\
[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\Router]\r\n\"HostName\"=\"10.0.0.1\"\r\n\"Protocol\"=\"telnet\"\r\n";
        assert_eq!(
            names(&putty_profiles(reg)),
            ["Build box|ssh|build.example.test|22|deploy", "skipped"]
        );
    }

    #[test]
    fn reads_mobaxterm_bookmarks() {
        let text = "[Bookmarks]\nSubRep=\nImgNum=42\nWeb server=#109#0%web.example.test%2200%root%%-1%-1%%%22%%0%0%0%%%-1%0%0%0%%1080%%0%0%1#MobaFont%10\nDesk=#91#4%pc%3389%%\n";
        let found = mobaxterm_sessions(text);
        assert_eq!(
            names(&found.profiles),
            ["Web server|ssh|web.example.test|2200|root"]
        );
        assert_eq!(
            found.connections,
            [ImportedConnection {
                kind: "rdp".into(),
                name: "Desk".into(),
                host: "pc".into(),
                port: 3389,
                username: String::new()
            }]
        );
    }

    #[test]
    fn reads_cyberduck_bookmarks() {
        let duck = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Protocol</key><string>davs</string>
<key>Nickname</key><string>Cloud &amp; files</string>
<key>Hostname</key><string>cloud.example.test</string>
<key>Port</key><string>443</string>
<key>Username</key><string>alice</string>
<key>Path</key><string>/remote.php/dav/files/alice</string>
</dict></plist>"#;
        let profile = cyberduck_profile(duck).unwrap();
        assert_eq!(profile.name, "Cloud & files");
        assert_eq!(profile.protocol, "webdav");
        assert_eq!(
            profile.host,
            "https://cloud.example.test:443/remote.php/dav/files/alice"
        );
        assert!(parse_file("x.txt", "hello").is_err());
    }

    #[test]
    fn reads_utf16_registry_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("putty.reg");
        let text = "[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\A]\r\n\"HostName\"=\"a.test\"\r\n\"UserName\"=\"me\"\r\n";
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        fs::write(&path, bytes).unwrap();
        let path = path.to_string_lossy();
        let parsed = parse_file(&path, &read_text(&path).unwrap()).unwrap();
        assert_eq!(names(&parsed.profiles), ["A|ssh|a.test|22|me"]);
    }

    #[test]
    fn reads_more_mobaxterm_session_types() {
        let text = "[Bookmarks_1]\nSubRep=Lab\nFiles=#140#6%ftp.example.test%21%ada%\nCopy=#140#7%sftp.example.test%2222%ada%\nOld=#98#1%switch.lab%23%%\nScreen=#128#5%mac.lab%5901%%\nSerial=#131#8%COM3%115200%\n";
        let found = mobaxterm_sessions(text);
        assert_eq!(
            names(&found.profiles),
            [
                "Files|ftp|ftp.example.test|21|ada",
                "Copy|ssh|sftp.example.test|2222|ada",
                "skipped"
            ]
        );
        assert_eq!(
            found
                .connections
                .iter()
                .map(|item| (item.kind.as_str(), item.port))
                .collect::<Vec<_>>(),
            [("telnet", 23), ("vnc", 5901)]
        );
    }

    #[test]
    fn reads_securecrt_exports_and_session_files() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<VanDyke version="3.0"><key name="Sessions">
  <key name="Default"><string name="Hostname">ignored</string></key>
  <key name="Prod">
    <key name="db &amp; cache"><string name="Hostname">db.example.test</string><string name="Protocol Name">SSH2</string><dword name="[SSH2] Port">2222</dword><string name="Username">ops</string></key>
    <key name="switch"><string name="Hostname">10.0.0.2</string><string name="Protocol Name">Telnet</string><dword name="Port">23</dword></key>
  </key>
  <key name="Prod"><string name="Comment">folder without host</string></key>
</key></VanDyke>"#;
        let found = securecrt_xml(xml);
        assert_eq!(
            names(&found.profiles),
            ["db & cache|ssh|db.example.test|2222|ops"]
        );
        assert_eq!(found.connections[0].host, "10.0.0.2");
        let ini = "S:\"Protocol Name\"=SSH2\nS:\"Hostname\"=web.example.test\nS:\"Username\"=deploy\nD:\"[SSH2] Port\"=00000016\n";
        assert_eq!(
            names(&securecrt_ini("web", ini).profiles),
            ["web|ssh|web.example.test|22|deploy"]
        );
    }

    #[test]
    fn reads_termius_style_csv() {
        let csv = "\u{feff}Label,Group,Tags,Hostname/IP,Protocol,Port,Username\n\"Web, prod\",Prod,web,web.example.test,ssh,2200,deploy\nRouter,,,192.168.1.1,telnet,,\n,,,,,,\n";
        let found = csv_sessions(csv).unwrap();
        assert_eq!(
            names(&found.profiles),
            ["Web, prod|ssh|web.example.test|2200|deploy"]
        );
        assert_eq!(found.connections[0].name, "Router");
        assert!(csv_sessions("Name,Port\nx,22\n").is_err());
    }
}
