use crate::{
    profiles::{self, Profile},
    vault, AppState,
};
use serde::Serialize;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    path::Path,
    time::Duration,
};
use suppaftp::native_tls::{TlsConnector, TlsStream};
use tauri::State;
use zeroize::Zeroizing;

/// The control connection, encrypted after AUTH TLS for FTPS servers.
enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(data),
            Self::Tls(stream) => stream.write(data),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

fn secure(profile: &Profile) -> bool {
    matches!(profile.protocol.as_str(), "ftps" | "ftps-implicit")
}

struct Reply {
    code: u16,
    text: String,
}

struct Control {
    reader: BufReader<Stream>,
    peer: SocketAddr,
}

fn upgrade(stream: TcpStream, profile: &Profile, tls: &TlsConnector) -> Result<Stream, String> {
    let host = profile.host.trim_matches(|c| c == '[' || c == ']');
    let secured = tls.connect(host, stream).map_err(|error| {
        format!("Could not set up encryption with {}: {error}. Check that its certificate is trusted on this computer.", profile.name)
    })?;
    let pins = crate::tls_pins::parse_pins(profile.tls_pin_sha256.as_deref())?;
    if !pins.is_empty() {
        let der = secured
            .peer_certificate()
            .ok()
            .flatten()
            .and_then(|certificate| certificate.to_der().ok())
            .ok_or("The FTPS server did not present a certificate.")?;
        if !crate::tls_pins::matches(&pins, &der) {
            return Err(format!(
                "{}'s certificate fingerprint does not match the profile.",
                profile.name
            ));
        }
    }
    Ok(Stream::Tls(Box::new(secured)))
}

impl Control {
    fn connect_with(profile: &Profile, password: &str, tls: &TlsConnector) -> Result<Self, String> {
        if !matches!(profile.protocol.as_str(), "ftp" | "ftps" | "ftps-implicit") {
            return Err("FXP copies between FTP or FTPS servers.".into());
        }
        let own_tls;
        let tls = if crate::webdav::client_identity_path(&profile.id).is_some() {
            own_tls = crate::ftp::tls_connector_for(profile, None)?;
            &own_tls
        } else {
            tls
        };
        let address = format!("{}:{}", profile.host, profile.port);
        let socket = address
            .to_socket_addrs()
            .map_err(|error| format!("Could not resolve {}: {error}", profile.host))?
            .find(|address| address.is_ipv4())
            .ok_or("FXP currently needs IPv4 FTP servers.")?;
        let stream = TcpStream::connect_timeout(&socket, Duration::from_secs(10))
            .map_err(|error| format!("Could not connect to {}: {error}", profile.name))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(60 * 30)))
            .map_err(|error| format!("Could not set FTP timeout: {error}"))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(30)))
            .map_err(|error| format!("Could not set FTP timeout: {error}"))?;
        let peer = stream
            .peer_addr()
            .map_err(|error| format!("Could not inspect FTP server address: {error}"))?;
        let stream = if profile.protocol == "ftps-implicit" {
            upgrade(stream, profile, tls)?
        } else {
            Stream::Plain(stream)
        };
        let mut control = Self {
            reader: BufReader::new(stream),
            peer,
        };
        control.expect("server greeting", &[220])?;
        if profile.protocol == "ftps" {
            control.send_line("AUTH TLS")?;
            control.expect("encryption", &[234])?;
            let Stream::Plain(plain) = control.reader.into_inner() else {
                return Err("The FTP connection changed unexpectedly.".into());
            };
            control = Self {
                reader: BufReader::new(upgrade(plain, profile, tls)?),
                peer,
            };
        }
        let first = control.command(&format!("USER {}", profile.username))?;
        match first.code {
            230 => {}
            331 => {
                control.send_line(&format!("PASS {password}"))?;
                control.expect("sign in", &[230])?;
            }
            _ => {
                return Err(format!(
                    "{} rejected the FTP username: {}",
                    profile.name, first.text
                ))
            }
        }
        if secure(profile) {
            control.send_line("PBSZ 0")?;
            control.expect("encryption setup", &[200])?;
        }
        control.send_line("TYPE I")?;
        control.expect("binary mode", &[200])?;
        Ok(control)
    }

    fn send_line(&mut self, line: &str) -> Result<(), String> {
        if line.contains(['\r', '\n']) {
            return Err("FTP path or account text contains a line break.".into());
        }
        let writer = self.reader.get_mut();
        writer
            .write_all(line.as_bytes())
            .and_then(|_| writer.write_all(b"\r\n"))
            .and_then(|_| writer.flush())
            .map_err(|error| format!("FTP control connection closed: {error}"))
    }

    fn reply(&mut self) -> Result<Reply, String> {
        let mut text = String::new();
        let mut first_code = None;
        for _ in 0..100 {
            let mut line = String::new();
            let bytes = self
                .reader
                .read_line(&mut line)
                .map_err(|error| format!("Could not read FTP server reply: {error}"))?;
            if bytes == 0 {
                return Err("FTP server closed the control connection.".into());
            }
            if text.len() + line.len() > 64 * 1024 {
                return Err("FTP server reply is too long.".into());
            }
            let code = line.get(..3).and_then(|prefix| prefix.parse::<u16>().ok());
            let separator = line.as_bytes().get(3).copied();
            if first_code.is_none() {
                first_code = code;
            }
            text.push_str(&line);
            if code == first_code && separator == Some(b' ') {
                return Ok(Reply {
                    code: code.unwrap_or_default(),
                    text: text.trim().into(),
                });
            }
        }
        Err("FTP server sent too many reply lines.".into())
    }

    fn expect(&mut self, action: &str, accepted: &[u16]) -> Result<Reply, String> {
        let reply = self.reply()?;
        if accepted.contains(&reply.code) {
            Ok(reply)
        } else {
            Err(format!("FTP {action} failed: {}", reply.text))
        }
    }

    fn command(&mut self, command: &str) -> Result<Reply, String> {
        self.send_line(command)?;
        self.reply()
    }

    /// A server-side checksum through the common XSHA256/XSHA1/XMD5/XCRC
    /// extensions, or None when the server offers none of them.
    fn checksum(&mut self, algorithm: &str, path: &str) -> Result<Option<String>, String> {
        let reply = self.command(&format!("{algorithm} {path}"))?;
        if !(200..300).contains(&reply.code) {
            return Ok(None);
        }
        Ok(reply
            .text
            .split_whitespace()
            .skip(1)
            .find(|word| word.len() >= 8 && word.chars().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase))
    }

    fn size(&mut self, path: &str) -> Result<Option<u64>, String> {
        let reply = self.command(&format!("SIZE {path}"))?;
        match reply.code {
            213 => reply
                .text
                .split_whitespace()
                .nth(1)
                .and_then(|size| size.parse().ok())
                .map(Some)
                .ok_or("FTP server returned an invalid file size.".into()),
            550 => Ok(None),
            _ => Err(format!("FTP server could not check {path}: {}", reply.text)),
        }
    }
}

use std::net::ToSocketAddrs;

fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 4096
        && !path.contains(['\r', '\n'])
        && !path.split('/').any(|part| part == "." || part == "..")
}

fn passive_address(reply: &Reply, peer: SocketAddr) -> Result<SocketAddr, String> {
    let start = reply
        .text
        .find('(')
        .ok_or("FTP destination did not return a PASV address.")?;
    let end = reply.text[start + 1..]
        .find(')')
        .ok_or("FTP destination returned an invalid PASV address.")?
        + start
        + 1;
    let parts: Vec<u8> = reply.text[start + 1..end]
        .split(',')
        .map(str::trim)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| "FTP destination returned an invalid PASV address.")?;
    if parts.len() != 6 {
        return Err("FTP destination returned an invalid PASV address.".into());
    }
    let address = IpAddr::V4(Ipv4Addr::new(parts[0], parts[1], parts[2], parts[3]));
    if address != peer.ip() {
        return Err("FTP destination advertised a data address different from its control server. FXP stopped to prevent routing data to another host.".into());
    }
    let port = u16::from(parts[4]) * 256 + u16::from(parts[5]);
    if port == 0 {
        return Err("FTP destination returned an invalid data port.".into());
    }
    Ok(SocketAddr::new(address, port))
}

pub(crate) const CLEAR_DATA_NEEDED: &str = "These servers cannot encrypt the copy between them";

/// Chooses how the data between the two servers is protected. Encrypted
/// server-to-server data needs SSCN on the server that opens the connection.
fn protect_data(
    source: &mut Control,
    destination: &mut Control,
    any_secure: bool,
    allow_clear_data: bool,
) -> Result<(), String> {
    if !any_secure {
        return Ok(());
    }
    let both_private = [source.command("PROT P")?, destination.command("PROT P")?]
        .iter()
        .all(|reply| reply.code == 200);
    if both_private && source.command("SSCN ON")?.code == 200 {
        return Ok(());
    }
    if !allow_clear_data {
        return Err(format!(
            "{CLEAR_DATA_NEEDED} (neither supports SSCN). Allow an unencrypted server-to-server copy to continue; sign-in and commands stay encrypted."
        ));
    }
    for control in [source, destination] {
        let reply = control.command("PROT C")?;
        if reply.code != 200 {
            return Err(format!(
                "A server requires encrypted data and refused an unencrypted copy: {}",
                reply.text
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_fxp(
    source_profile: &Profile,
    source_password: &str,
    source_path: &str,
    destination_profile: &Profile,
    destination_password: &str,
    destination_dir: &str,
    allow_clear_data: bool,
    tls: &TlsConnector,
) -> Result<FxpResult, String> {
    if source_profile.id == destination_profile.id {
        return Err(
            "FXP needs two different FTP servers. Use the normal copy action for one server."
                .into(),
        );
    }
    if !valid_path(source_path) || !valid_path(destination_dir) {
        return Err("Choose absolute FTP paths without parent segments or line breaks.".into());
    }
    let name = Path::new(source_path)
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or("Choose a file inside the source FTP server.")?;
    let destination_path = format!("{}/{}", destination_dir.trim_end_matches('/'), name);
    if !valid_path(&destination_path) {
        return Err("The destination FTP path is invalid.".into());
    }
    let temporary = format!("{destination_path}.weave-{}.part", uuid::Uuid::new_v4());
    let mut source = Control::connect_with(source_profile, source_password, tls)?;
    let mut destination = Control::connect_with(destination_profile, destination_password, tls)?;
    protect_data(
        &mut source,
        &mut destination,
        secure(source_profile) || secure(destination_profile),
        allow_clear_data,
    )?;
    let source_size = source
        .size(source_path)?
        .ok_or("The source FTP file does not exist or its size cannot be read.")?;
    if destination.size(&destination_path)?.is_some() {
        return Err("A file with that name already exists on the destination FTP server. FXP will not overwrite it.".into());
    }
    let passive = destination.command("PASV")?;
    if passive.code != 227 {
        return Err(format!(
            "The destination FTP server refused passive mode: {}",
            passive.text
        ));
    }
    let address = passive_address(&passive, destination.peer)?;
    let IpAddr::V4(ip) = address.ip() else {
        unreachable!()
    };
    let octets = ip.octets();
    let port = address.port();
    let port_command = format!(
        "PORT {},{},{},{},{},{}",
        octets[0],
        octets[1],
        octets[2],
        octets[3],
        port / 256,
        port % 256
    );
    let port_reply = source.command(&port_command)?;
    if port_reply.code != 200 {
        return Err(format!("The source FTP server refused FXP routing: {}. Its PORT policy may block server-to-server transfers.", port_reply.text));
    }
    destination.send_line(&format!("STOR {temporary}"))?;
    destination.expect("destination upload", &[125, 150])?;
    let transferred = (|| {
        source.send_line(&format!("RETR {source_path}"))?;
        source.expect("source download", &[125, 150])?;
        source.expect("source completion", &[226, 250])?;
        destination.expect("destination completion", &[226, 250])?;
        let copied_size = destination
            .size(&temporary)?
            .ok_or("The destination FTP server did not retain the transferred file.")?;
        if copied_size != source_size {
            return Err(format!("FXP copied {copied_size} of {source_size} bytes."));
        }
        let mut verified_with = None;
        for algorithm in ["XSHA256", "XSHA1", "XMD5", "XCRC"] {
            let Some(original) = source.checksum(algorithm, source_path)? else {
                continue;
            };
            let Some(copy) = destination.checksum(algorithm, &temporary)? else {
                continue;
            };
            if original != copy {
                return Err("The copy's checksum does not match the source file.".into());
            }
            verified_with = Some(algorithm.trim_start_matches('X').to_string());
            break;
        }
        if destination.size(&destination_path)?.is_some() {
            return Err("Another file appeared at the destination during FXP.".into());
        }
        destination.send_line(&format!("RNFR {temporary}"))?;
        destination.expect("rename preparation", &[350])?;
        destination.send_line(&format!("RNTO {destination_path}"))?;
        destination.expect("destination rename", &[250])?;
        Ok(FxpResult {
            destination_path,
            bytes_copied: source_size,
            verified_with,
        })
    })();
    if transferred.is_ok() {
        return transferred;
    }
    drop(source);
    drop(destination);
    let cleaned = Control::connect_with(destination_profile, destination_password, tls)
        .and_then(|mut control| control.command(&format!("DELE {temporary}")))
        .is_ok_and(|reply| matches!(reply.code, 250 | 550));
    let reason = transferred.unwrap_err();
    if cleaned {
        Err(format!(
            "{reason} The temporary destination file was removed."
        ))
    } else {
        Err(format!(
            "{reason} A temporary file may remain at {temporary}; remove it from Files."
        ))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FxpResult {
    destination_path: String,
    bytes_copied: u64,
    /// The checksum both servers agreed on, or None when only the size could be compared.
    verified_with: Option<String>,
}

#[tauri::command]
pub async fn fxp_transfer(
    state: State<'_, AppState>,
    source_profile: String,
    source_path: String,
    destination_profile: String,
    destination_dir: String,
    allow_clear_data: Option<bool>,
) -> Result<FxpResult, String> {
    let source = profiles::get_profile(&state, &source_profile)?;
    let destination = profiles::get_profile(&state, &destination_profile)?;
    let source_password: Zeroizing<String> = vault::password_for(&state, &source)?;
    let destination_password: Zeroizing<String> = vault::password_for(&state, &destination)?;
    tauri::async_runtime::spawn_blocking(move || {
        let tls = TlsConnector::new()
            .map_err(|error| format!("Could not prepare encryption: {error}"))?;
        run_fxp(
            &source,
            &source_password,
            &source_path,
            &destination,
            &destination_password,
            &destination_dir,
            allow_clear_data.unwrap_or(false),
            &tls,
        )
    })
    .await
    .map_err(|error| format!("FXP transfer stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_pasv_redirection_and_unsafe_paths() {
        let reply = Reply {
            code: 227,
            text: "227 Entering Passive Mode (10,0,0,2,8,2)".into(),
        };
        let peer: SocketAddr = "10.0.0.1:21".parse().unwrap();
        assert!(passive_address(&reply, peer).is_err());
        assert!(!valid_path("/folder/../secret"));
        assert!(!valid_path("/file\r\nDELE /other"));
    }

    #[test]
    #[ignore = "requires two local FTP test servers"]
    fn copies_between_real_ftp_servers_without_relaying_bytes() {
        let source_port = std::env::var("WEAVE_FTP_TEST_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let destination_port = std::env::var("WEAVE_FXP_TEST_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let source_root = std::env::var("WEAVE_FTP_TEST_ROOT").unwrap();
        let destination_root = std::env::var("WEAVE_FXP_TEST_ROOT").unwrap();
        std::fs::write(
            Path::new(&source_root).join("fxp-source.txt"),
            "direct ftp copy",
        )
        .unwrap();
        let profile = |id: &str, port| Profile {
            id: id.into(),
            name: id.into(),
            host: "127.0.0.1".into(),
            protocol: "ftp".into(),
            port,
            username: "weave".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let result = run_fxp(
            &profile("source", source_port),
            "weave-test",
            "/fxp-source.txt",
            &profile("destination", destination_port),
            "weave-test",
            "/",
            false,
            &TlsConnector::new().unwrap(),
        )
        .unwrap();
        assert_eq!(result.bytes_copied, 15);
        assert_eq!(result.verified_with.as_deref(), Some("MD5"));
        assert_eq!(
            std::fs::read_to_string(Path::new(&destination_root).join("fxp-source.txt")).unwrap(),
            "direct ftp copy"
        );
        assert!(run_fxp(
            &profile("source", source_port),
            "weave-test",
            "/fxp-source.txt",
            &profile("destination", destination_port),
            "weave-test",
            "/",
            false,
            &TlsConnector::new().unwrap(),
        )
        .unwrap_err()
        .contains("already exists"));
    }

    #[test]
    #[ignore = "requires two local FTPS test servers"]
    fn copies_between_ftps_servers_only_with_consent_for_clear_data() {
        let port = |name: &str| std::env::var(name).unwrap().parse::<u16>().unwrap();
        let source_root = std::env::var("WEAVE_FTP_TEST_ROOT").unwrap();
        let destination_root = std::env::var("WEAVE_FXPS_DEST_ROOT").unwrap();
        let certificate = suppaftp::native_tls::Certificate::from_pem(
            &std::fs::read(std::env::var("WEAVE_FTPS_TEST_CERT").unwrap()).unwrap(),
        )
        .unwrap();
        let tls = TlsConnector::builder()
            .add_root_certificate(certificate)
            .build()
            .unwrap();
        std::fs::write(
            Path::new(&source_root).join("fxps-source.txt"),
            "secure control copy",
        )
        .unwrap();
        let profile = |id: &str, port| Profile {
            id: id.into(),
            name: id.into(),
            host: "localhost".into(),
            protocol: "ftps".into(),
            port,
            username: "weave".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let source = profile("source", port("WEAVE_FXPS_SOURCE_PORT"));
        let destination = profile("destination", port("WEAVE_FXPS_DEST_PORT"));
        let refused = run_fxp(
            &source,
            "weave-test",
            "/fxps-source.txt",
            &destination,
            "weave-test",
            "/",
            false,
            &tls,
        )
        .unwrap_err();
        assert!(refused.contains(CLEAR_DATA_NEEDED));
        let result = run_fxp(
            &source,
            "weave-test",
            "/fxps-source.txt",
            &destination,
            "weave-test",
            "/",
            true,
            &tls,
        )
        .unwrap();
        assert_eq!(result.bytes_copied, 19);
        assert_eq!(result.verified_with.as_deref(), Some("MD5"));
        assert_eq!(
            std::fs::read_to_string(Path::new(&destination_root).join("fxps-source.txt")).unwrap(),
            "secure control copy"
        );
    }
}
