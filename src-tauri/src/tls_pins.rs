//! Certificate fingerprints a profile trusts, and reading a server's
//! current fingerprint so a renewed certificate can be accepted.

use crate::{profiles, AppState};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};
use suppaftp::native_tls::TlsConnector;
use tauri::{AppHandle, Manager};

/// Up to three SHA-256 fingerprints, separated by commas or spaces, so the
/// next certificate can be trusted before the server switches to it.
pub(crate) fn parse_pins(raw: Option<&str>) -> Result<Vec<[u8; 32]>, String> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let pins = raw
        .split([',', ' ', '\n'])
        .map(|pin| pin.trim().replace(':', ""))
        .filter(|pin| !pin.is_empty())
        .map(|pin| {
            if pin.len() != 64 || !pin.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(
                    "Each certificate fingerprint must be 64 hexadecimal characters (SHA-256)."
                        .to_string(),
                );
            }
            let mut bytes = [0_u8; 32];
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&pin[index * 2..index * 2 + 2], 16).unwrap_or_default();
            }
            Ok(bytes)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if pins.len() > 3 {
        return Err("Enter at most three certificate fingerprints.".into());
    }
    Ok(pins)
}

pub(crate) fn matches(pins: &[[u8; 32]], der: &[u8]) -> bool {
    pins.is_empty() || pins.iter().any(|pin| Sha256::digest(der).as_slice() == pin)
}

/// Normalizes saved fingerprints to lowercase hex separated by ", ".
pub(crate) fn normalize(raw: &str) -> Result<Option<String>, String> {
    let pins = parse_pins(Some(raw))?;
    Ok((!pins.is_empty()).then(|| {
        pins.iter()
            .map(|pin| {
                pin.iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(", ")
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCertificate {
    sha256: String,
    trusted_by_mac: bool,
    matches_profile: bool,
}

fn read_reply(reader: &mut BufReader<TcpStream>) -> Result<u16, String> {
    loop {
        let mut line = String::new();
        if reader
            .read_line(&mut line)
            .map_err(|error| format!("The server stopped answering: {error}"))?
            == 0
        {
            return Err("The server closed the connection.".into());
        }
        if line.as_bytes().get(3) == Some(&b' ') {
            return line[..3]
                .parse()
                .map_err(|_| "The server sent an unexpected reply.".to_string());
        }
    }
}

/// Connects far enough to read the certificate. It is never used to sign in
/// or transfer anything, so an untrusted certificate is reported, not used.
pub(crate) fn fetch(
    protocol: &str,
    host: &str,
    port: u16,
    pins: &[[u8; 32]],
) -> Result<ServerCertificate, String> {
    let addresses: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("Could not find {host}: {error}"))?
        .collect();
    let open = || -> Result<TcpStream, String> {
        let stream = addresses
            .iter()
            .find_map(|address| TcpStream::connect_timeout(address, Duration::from_secs(10)).ok())
            .ok_or_else(|| format!("Could not connect to {host} on port {port}."))?;
        stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
        if protocol == "ftps" {
            let mut reader = BufReader::new(stream.try_clone().map_err(|error| error.to_string())?);
            if read_reply(&mut reader)? != 220 {
                return Err("The FTP server did not greet normally.".into());
            }
            (&stream)
                .write_all(b"AUTH TLS\r\n")
                .map_err(|error| error.to_string())?;
            if read_reply(&mut reader)? != 234 {
                return Err("The FTP server does not offer encryption (AUTH TLS).".into());
            }
        }
        Ok(stream)
    };
    let inspect = TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|error| format!("Could not prepare encryption: {error}"))?;
    let der = inspect
        .connect(host, open()?)
        .map_err(|error| format!("Could not start encryption with {host}: {error}"))?
        .peer_certificate()
        .ok()
        .flatten()
        .and_then(|certificate| certificate.to_der().ok())
        .ok_or("The server did not present a certificate.")?;
    let trusted_by_mac = TlsConnector::new()
        .is_ok_and(|tls| open().is_ok_and(|stream| tls.connect(host, stream).is_ok()));
    Ok(ServerCertificate {
        sha256: format!("{:x}", Sha256::digest(&der)),
        trusted_by_mac,
        matches_profile: !pins.is_empty() && matches(pins, &der),
    })
}

#[tauri::command]
pub async fn tls_server_certificate(
    app: AppHandle,
    protocol: String,
    host: String,
    port: u16,
    profile_id: Option<String>,
) -> Result<ServerCertificate, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let pins = match profile_id.filter(|id| !id.is_empty()) {
            Some(id) => parse_pins(
                profiles::get_profile(&app.state::<AppState>(), &id)?
                    .tls_pin_sha256
                    .as_deref(),
            )?,
            None => Vec::new(),
        };
        let (protocol, host, port) = if protocol == "webdav" {
            let url =
                reqwest::Url::parse(host.trim()).map_err(|_| "Enter the WebDAV address first.")?;
            if url.scheme() != "https" {
                return Err("Only HTTPS WebDAV addresses have a certificate.".into());
            }
            let name = url
                .host_str()
                .ok_or("Enter the WebDAV address first.")?
                .to_string();
            (
                "https".to_string(),
                name,
                url.port_or_known_default().unwrap_or(443),
            )
        } else {
            (protocol, host.trim().to_string(), port)
        };
        if !matches!(protocol.as_str(), "ftps" | "ftps-implicit" | "https") {
            return Err("Certificates are read from FTPS and HTTPS WebDAV servers.".into());
        }
        fetch(&protocol, &host, port, &pins)
    })
    .await
    .map_err(|error| format!("Reading the certificate stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_up_to_three_fingerprints() {
        let one = "ab".repeat(32);
        let two = "AB:".repeat(31) + "CD";
        assert_eq!(parse_pins(Some(&format!("{one}, {two}"))).unwrap().len(), 2);
        assert_eq!(
            normalize(&format!("{one} {two}")).unwrap().unwrap(),
            format!("{one}, {}cd", "ab".repeat(31))
        );
        assert!(parse_pins(Some("short")).is_err());
        assert!(parse_pins(Some(&[one.as_str(); 4].join(","))).is_err());
        assert!(normalize("  ").unwrap().is_none());
        assert!(matches(&[], b"anything"));
        let der = b"certificate";
        let pin: [u8; 32] = Sha256::digest(der).into();
        assert!(matches(&[[0; 32], pin], der));
        assert!(!matches(&[[0; 32]], der));
    }

    #[test]
    #[ignore = "requires the local FTP test server"]
    fn reads_the_certificate_of_an_ftps_server() {
        let port = std::env::var("WEAVE_FTPS_TEST_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let pem = std::fs::read(std::env::var("WEAVE_FTPS_TEST_CERT").unwrap()).unwrap();
        let der = suppaftp::native_tls::Certificate::from_pem(&pem)
            .unwrap()
            .to_der()
            .unwrap();
        let expected = format!("{:x}", Sha256::digest(&der));
        let unpinned = fetch("ftps", "localhost", port, &[]).unwrap();
        assert_eq!(unpinned.sha256, expected);
        assert!(!unpinned.trusted_by_mac && !unpinned.matches_profile);
        let pin: [u8; 32] = Sha256::digest(&der).into();
        assert!(
            fetch("ftps", "localhost", port, &[[0; 32], pin])
                .unwrap()
                .matches_profile
        );
    }
}
