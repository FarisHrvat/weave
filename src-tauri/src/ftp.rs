use crate::{
    files::{FileEntry, FileListing},
    profiles::Profile,
    sftp::{join_remote, validate_remote_delete},
};
use chrono::{DateTime, Utc};
use std::{
    io::{Read, Write},
    net::TcpStream,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};
use suppaftp::{
    list::{File, ListParser},
    native_tls::{TlsConnector, TlsStream as NativeStream},
    types::FileType,
    FtpError, FtpResult, ImplFtpStream, Status, TlsConnector as FtpTlsConnector, TlsStream,
    TransferStream,
};

pub trait FtpData: Read + Write + Send {
    fn finish(self: Box<Self>) -> Result<(), String>;
}

impl<T: TlsStream + Send + 'static> FtpData for TransferStream<T> {
    fn finish(self: Box<Self>) -> Result<(), String> {
        (*self)
            .finish()
            .map_err(|error| format!("FTP transfer did not finish: {error}"))
    }
}

pub struct FtpClient {
    connection: ImplFtpStream<PinnedStream>,
}

#[derive(Debug)]
struct PinnedConnector {
    tls: TlsConnector,
    pins: Vec<[u8; 32]>,
}

#[derive(Debug)]
struct PinnedStream {
    stream: NativeStream<TcpStream>,
    shutdown: bool,
}

impl FtpTlsConnector for PinnedConnector {
    type Stream = PinnedStream;

    fn connect(&self, domain: &str, socket: TcpStream) -> FtpResult<PinnedStream> {
        let stream = self
            .tls
            .connect(domain, socket)
            .map_err(|error| FtpError::SecureError(error.to_string()))?;
        if !self.pins.is_empty() {
            let certificate = stream
                .peer_certificate()
                .map_err(|error| FtpError::SecureError(error.to_string()))?
                .ok_or_else(|| {
                    FtpError::SecureError("FTPS server did not present a certificate.".into())
                })?;
            let der = certificate
                .to_der()
                .map_err(|error| FtpError::SecureError(error.to_string()))?;
            if !crate::tls_pins::matches(&self.pins, &der) {
                return Err(FtpError::SecureError(
                    "FTPS certificate fingerprint does not match this profile. Check the server identity before reconnecting.".into(),
                ));
            }
        }
        Ok(PinnedStream {
            stream,
            shutdown: true,
        })
    }
}

impl TlsStream for PinnedStream {
    type InnerStream = NativeStream<TcpStream>;

    fn tcp_stream(mut self) -> FtpResult<TcpStream> {
        self.shutdown = false;
        self.stream
            .get_ref()
            .try_clone()
            .map_err(FtpError::ConnectionError)
    }

    fn get_ref(&self) -> &TcpStream {
        self.stream.get_ref()
    }

    fn mut_ref(&mut self) -> &mut Self::InnerStream {
        &mut self.stream
    }
}

impl Drop for PinnedStream {
    fn drop(&mut self) {
        if self.shutdown {
            let _ = self.stream.shutdown();
        }
    }
}

fn check_path(path: &str) -> Result<(), String> {
    if path.contains(['\r', '\n', '\0']) {
        return Err("FTP paths cannot contain line breaks or null characters.".into());
    }
    Ok(())
}

fn set_timeouts(connection: &ImplFtpStream<PinnedStream>) -> Result<(), String> {
    connection
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| format!("Could not configure FTP timeout: {error}"))?;
    connection
        .get_ref()
        .set_write_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| format!("Could not configure FTP timeout: {error}"))
}

/// Converts a PEM client certificate file into the PKCS#8 form macOS imports.
fn client_identity(path: &std::path::Path) -> Result<suppaftp::native_tls::Identity, String> {
    use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
    let bytes = std::fs::read(path).map_err(|error| {
        format!(
            "Could not read the client certificate {}: {error}",
            path.display()
        )
    })?;
    let certificates = CertificateDer::pem_slice_iter(&bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "The client certificate file is not valid PEM.")?;
    if certificates.is_empty() {
        return Err("The client certificate file has no certificate. Use a PEM file with the certificate and its private key.".into());
    }
    let pkcs8 = match PrivateKeyDer::from_pem_slice(&bytes) {
        Ok(PrivateKeyDer::Pkcs8(key)) => key.secret_pkcs8_der().to_vec(),
        Ok(PrivateKeyDer::Pkcs1(key)) => pkcs1_to_pkcs8(key.secret_pkcs1_der()),
        _ => return Err("The client certificate needs an unencrypted RSA or PKCS#8 private key. Convert it with: openssl pkcs8 -topk8 -nocrypt -in key.pem -out key-pkcs8.pem".into()),
    };
    let pem = |label: &str, der: &[u8]| {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let encoded = STANDARD.encode(der);
        let lines: Vec<&str> = encoded
            .as_bytes()
            .chunks(64)
            .map(|line| std::str::from_utf8(line).unwrap_or_default())
            .collect();
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            lines.join("\n")
        )
    };
    let chain: String = certificates
        .iter()
        .map(|certificate| pem("CERTIFICATE", certificate))
        .collect();
    suppaftp::native_tls::Identity::from_pkcs8(
        chain.as_bytes(),
        pem("PRIVATE KEY", &pkcs8).as_bytes(),
    )
    .map_err(|error| format!("Could not use the client certificate: {error}"))
}

/// Wraps an RSA (PKCS#1) key in a PKCS#8 structure.
fn pkcs1_to_pkcs8(rsa: &[u8]) -> Vec<u8> {
    fn der(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        let length = body.len();
        if length < 128 {
            out.push(length as u8);
        } else {
            let bytes: Vec<u8> = length
                .to_be_bytes()
                .into_iter()
                .skip_while(|byte| *byte == 0)
                .collect();
            out.push(0x80 | bytes.len() as u8);
            out.extend(bytes);
        }
        out.extend_from_slice(body);
        out
    }
    let algorithm = [
        0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00,
    ];
    let mut body = vec![0x02, 0x01, 0x00];
    body.extend_from_slice(&algorithm);
    body.extend(der(0x04, rsa));
    der(0x30, &body)
}

/// The TLS settings for an FTPS profile, including its client certificate.
pub(crate) fn tls_connector_for(
    profile: &Profile,
    extra_root: Option<suppaftp::native_tls::Certificate>,
) -> Result<TlsConnector, String> {
    let mut builder = TlsConnector::builder();
    if let Some(root) = extra_root {
        builder.add_root_certificate(root);
    }
    if let Some(path) = crate::webdav::client_identity_path(&profile.id) {
        builder.identity(client_identity(&path)?);
    }
    builder
        .build()
        .map_err(|error| format!("Could not start secure FTP: {error}"))
}

impl FtpClient {
    pub fn connect(profile: &Profile, password: &str) -> Result<Self, String> {
        Self::connect_with_tls(profile, password, None)
    }

    fn connect_with_tls(
        profile: &Profile,
        password: &str,
        tls_connector: Option<TlsConnector>,
    ) -> Result<Self, String> {
        let address = format!("{}:{}", profile.host, profile.port);
        let pins = crate::tls_pins::parse_pins(profile.tls_pin_sha256.as_deref())?;
        let mut connection = match profile.protocol.as_str() {
            "ftp" | "ftps" => {
                let mut connection = ImplFtpStream::<PinnedStream>::connect(&address)
                    .map_err(|error| format!("Could not connect to {}: {error}", profile.name))?;
                set_timeouts(&connection)?;
                if profile.protocol == "ftps" {
                    let tls = match tls_connector {
                        Some(tls) => tls,
                        None => tls_connector_for(profile, None)?,
                    };
                    connection = connection
                        .into_secure(
                            PinnedConnector {
                                tls,
                                pins: pins.clone(),
                            },
                            &profile.host,
                        )
                        .map_err(|error| {
                            format!(
                                "Could not verify the FTPS connection to {}: {error}",
                                profile.host
                            )
                        })?;
                }
                connection
            }
            "ftps-implicit" => {
                let tls = match tls_connector {
                    Some(tls) => tls,
                    None => tls_connector_for(profile, None)?,
                };
                let mut connection = ImplFtpStream::<PinnedStream>::connect_secure_implicit(
                    &address,
                    PinnedConnector {
                        tls,
                        pins: pins.clone(),
                    },
                    &profile.host,
                )
                .map_err(|error| {
                    format!(
                        "Could not verify the implicit FTPS connection to {}: {error}",
                        profile.host
                    )
                })?;
                connection
                    .custom_command("PBSZ 0", &[Status::CommandOk])
                    .map_err(|error| format!("Could not prepare secure FTP data: {error}"))?;
                connection
                    .custom_command("PROT P", &[Status::CommandOk])
                    .map_err(|error| format!("Could not encrypt FTP data: {error}"))?;
                connection
            }
            _ => return Err("Choose FTP or FTPS for this file connection.".into()),
        };
        set_timeouts(&connection)?;
        connection
            .login(profile.username.as_str(), password)
            .map_err(|error| format!("Could not sign in to {}: {error}", profile.name))?;
        connection
            .transfer_type(FileType::Binary)
            .map_err(|error| format!("Could not enable binary transfers: {error}"))?;
        Ok(Self { connection })
    }

    pub fn list(&mut self, path: &str) -> Result<FileListing, String> {
        check_path(path)?;
        self.connection
            .cwd(path)
            .map_err(|error| format!("Cannot open FTP folder {path}: {error}"))?;
        let folder = self
            .connection
            .pwd()
            .map_err(|error| format!("Cannot read FTP folder path: {error}"))?;
        let (lines, machine) = match self.connection.mlsd(None) {
            Ok(lines) => (lines, true),
            Err(_) => (
                self.connection
                    .list(None)
                    .map_err(|error| format!("Cannot list FTP folder {folder}: {error}"))?,
                false,
            ),
        };
        let mut entries = Vec::with_capacity(lines.len());
        for line in lines {
            let parsed = if machine {
                ListParser::parse_mlsd(&line)
            } else {
                line.parse::<File>()
            };
            let file = parsed.map_err(|error| {
                format!("Cannot understand an entry in FTP folder {folder}: {error}")
            })?;
            if matches!(file.name(), "." | "..") {
                continue;
            }
            entries.push(FileEntry {
                name: file.name().into(),
                path: join_remote(&folder, file.name()),
                is_dir: file.is_directory(),
                is_symlink: file.is_symlink(),
                size: file.size() as u64,
                modified: file
                    .modified()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs()),
                readonly: false,
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let parent = if folder == "/" {
            None
        } else {
            Path::new(&folder)
                .parent()
                .map(|path| path.to_string_lossy().to_string())
        };
        Ok(FileListing {
            path: folder,
            parent,
            entries,
        })
    }

    pub fn stat(&mut self, path: &str) -> Result<FileEntry, String> {
        check_path(path)?;
        let (parent, name) = path
            .trim_end_matches('/')
            .rsplit_once('/')
            .ok_or("Choose a file or folder inside the FTP server.")?;
        let parent = if parent.is_empty() { "/" } else { parent };
        self.list(parent)?
            .entries
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("FTP item no longer exists: {path}"))
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), String> {
        check_path(path)?;
        self.connection
            .mkdir(path)
            .map_err(|error| format!("Could not create FTP folder {path}: {error}"))
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        check_path(from)?;
        check_path(to)?;
        self.connection
            .rename(from, to)
            .map_err(|error| format!("Could not rename FTP item {from}: {error}"))
    }

    pub fn remove(&mut self, path: &str, directory: bool) -> Result<(), String> {
        check_path(path)?;
        let result = if directory {
            self.connection.rmdir(path)
        } else {
            self.connection.rm(path)
        };
        result.map_err(|error| format!("Could not delete FTP item {path}: {error}"))
    }

    pub fn delete(&mut self, path: &str, recursive: bool) -> Result<(), String> {
        validate_remote_delete(path)?;
        let item = self.stat(path)?;
        if item.is_dir && recursive {
            for child in self.list(path)?.entries {
                self.delete(&child.path, true)?;
            }
        }
        self.remove(path, item.is_dir)
    }

    pub fn chmod(&mut self, path: &str, mode: u32) -> Result<(), String> {
        check_path(path)?;
        self.connection
            .site(format!("CHMOD {mode:o} {path}"))
            .map(|_| ())
            .map_err(|error| {
                format!("This FTP server could not change permissions for {path}: {error}")
            })
    }

    pub fn set_mtime(&mut self, path: &str, seconds: u64) -> Result<(), String> {
        check_path(path)?;
        let timestamp = i64::try_from(seconds)
            .ok()
            .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0))
            .ok_or("This file's modification time is outside the supported FTP range.")?
            .format("%Y%m%d%H%M%S");
        self.connection
            .custom_command(format!("MFMT {timestamp} {path}"), &[Status::File])
            .map(|_| ())
            .map_err(|error| {
                format!(
                    "This FTP server could not preserve the modification time of {path}: {error}"
                )
            })
    }

    pub fn open_read(&mut self, path: &str, offset: u64) -> Result<Box<dyn FtpData>, String> {
        check_path(path)?;
        if offset > 0 {
            self.connection
                .resume_transfer(offset as usize)
                .map_err(|error| format!("Could not resume FTP download at {offset}: {error}"))?;
        }
        self.connection
            .retr_as_stream(path)
            .map(|stream| Box::new(stream) as Box<dyn FtpData>)
            .map_err(|error| format!("Could not read FTP file {path}: {error}"))
    }

    pub fn open_write(&mut self, path: &str, append: bool) -> Result<Box<dyn FtpData>, String> {
        check_path(path)?;
        let result = if append {
            self.connection.append_with_stream(path)
        } else {
            self.connection.put_with_stream(path)
        };
        result
            .map(|stream| Box::new(stream) as Box<dyn FtpData>)
            .map_err(|error| format!("Could not write FTP file {path}: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn accepts_only_sha256_certificate_fingerprints() {
        assert!(crate::tls_pins::parse_pins(None).unwrap().is_empty());
        assert_eq!(
            crate::tls_pins::parse_pins(Some(&"ab".repeat(32))).unwrap(),
            vec![[0xab; 32]]
        );
        assert!(crate::tls_pins::parse_pins(Some("short")).is_err());
        assert!(crate::tls_pins::parse_pins(Some(&"zz".repeat(32))).is_err());
    }

    fn secure_roundtrip(protocol: &str, port: &str) {
        let cert_path = std::env::var("WEAVE_FTPS_TEST_CERT")
            .expect("Set WEAVE_FTPS_TEST_CERT to the local test certificate path");
        let mut profile = Profile {
            id: "secure-test".into(),
            name: "Local FTPS test".into(),
            host: "localhost".into(),
            protocol: protocol.into(),
            port: port.parse().unwrap(),
            username: "weave".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        assert!(FtpClient::connect(&profile, "weave-test").is_err());
        let certificate =
            suppaftp::native_tls::Certificate::from_pem(&std::fs::read(cert_path).unwrap())
                .unwrap();
        profile.tls_pin_sha256 = Some(format!(
            "{:x}",
            Sha256::digest(certificate.to_der().unwrap())
        ));
        let trusted_tls = || {
            TlsConnector::builder()
                .add_root_certificate(certificate.clone())
                .build()
                .unwrap()
        };
        let mut wrong_pin = profile.clone();
        wrong_pin.tls_pin_sha256 = Some("00".repeat(32));
        assert!(
            FtpClient::connect_with_tls(&wrong_pin, "weave-test", Some(trusted_tls()))
                .err()
                .unwrap()
                .contains("fingerprint")
        );
        let mut client =
            FtpClient::connect_with_tls(&profile, "weave-test", Some(trusted_tls())).unwrap();
        let path = format!("/secure-{}.txt", uuid::Uuid::new_v4());
        let mut upload = client.open_write(&path, false).unwrap();
        upload.write_all(b"encrypted transfer").unwrap();
        upload.finish().unwrap();
        let mut download = client.open_read(&path, 0).unwrap();
        let mut contents = Vec::new();
        download.read_to_end(&mut contents).unwrap();
        download.finish().unwrap();
        assert_eq!(contents, b"encrypted transfer");
        client.remove(&path, false).unwrap();
    }

    #[test]
    #[ignore = "requires the local FTPS test server"]
    fn ftps_validates_certificate_and_transfers_data() {
        let port = std::env::var("WEAVE_FTPS_TEST_PORT")
            .expect("Set WEAVE_FTPS_TEST_PORT to the local FTPS test server port");
        secure_roundtrip("ftps", &port);
    }

    #[test]
    #[ignore = "requires the local implicit FTPS test server"]
    fn implicit_ftps_validates_certificate_and_transfers_data() {
        let port = std::env::var("WEAVE_IMPLICIT_FTPS_TEST_PORT")
            .expect("Set WEAVE_IMPLICIT_FTPS_TEST_PORT to the local implicit FTPS server port");
        secure_roundtrip("ftps-implicit", &port);
    }

    #[test]
    #[ignore = "requires the local FTP test server"]
    fn anonymous_ftp_browses_without_a_password() {
        let port = std::env::var("WEAVE_FTP_TEST_PORT")
            .expect("Set WEAVE_FTP_TEST_PORT to the local FTP test server port");
        let profile = Profile {
            id: "anonymous-test".into(),
            name: "Anonymous FTP test".into(),
            host: "127.0.0.1".into(),
            protocol: "ftp".into(),
            port: port.parse().unwrap(),
            username: "anonymous".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let mut client = FtpClient::connect(&profile, "").unwrap();
        assert!(!client.list("/").unwrap().entries.is_empty());
        assert!(client.mkdir("/anonymous-read-only").is_err());
    }

    #[test]
    #[ignore = "requires the local FTP test server"]
    fn browses_edits_and_deletes_on_a_real_ftp_server() {
        let port = std::env::var("WEAVE_FTP_TEST_PORT")
            .expect("Set WEAVE_FTP_TEST_PORT to the local FTP test server port");
        let profile = Profile {
            id: "test".into(),
            name: "Local FTP test".into(),
            host: "127.0.0.1".into(),
            protocol: "ftp".into(),
            port: port.parse().unwrap(),
            username: "weave".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let mut client = FtpClient::connect(&profile, "weave-test").unwrap();
        let folder = format!("/weave-test-{}", uuid::Uuid::new_v4());
        client.mkdir(&folder).unwrap();
        let file = join_remote(&folder, "notes.txt");
        let mut upload = client.open_write(&file, false).unwrap();
        upload.write_all(b"hello world").unwrap();
        upload.finish().unwrap();
        client.set_mtime(&file, 1_600_000_000).unwrap();

        let listing = client.list(&folder).unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, "notes.txt");
        assert_eq!(client.stat(&file).unwrap().size, 11);
        assert_eq!(client.stat(&file).unwrap().modified, Some(1_600_000_000));
        let mut download = client.open_read(&file, 6).unwrap();
        let mut contents = Vec::new();
        download.read_to_end(&mut contents).unwrap();
        download.finish().unwrap();
        assert_eq!(contents, b"world");

        let renamed = join_remote(&folder, "renamed.txt");
        client.rename(&file, &renamed).unwrap();
        client.delete(&folder, true).unwrap();
        assert!(client.stat(&renamed).is_err());
    }

    #[test]
    #[ignore = "requires the local FTP test server"]
    fn ftps_presents_a_client_certificate_when_the_server_requires_one() {
        let port = std::env::var("WEAVE_FTPS_CLIENT_CERT_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let identity = std::env::var("WEAVE_FTPS_CLIENT_IDENTITY").unwrap();
        let root = suppaftp::native_tls::Certificate::from_pem(
            &std::fs::read(std::env::var("WEAVE_FTPS_TEST_CERT").unwrap()).unwrap(),
        )
        .unwrap();
        let profile = Profile {
            id: "client-cert".into(),
            name: "Client certificate".into(),
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
        let without = TlsConnector::builder()
            .add_root_certificate(root.clone())
            .build()
            .unwrap();
        assert!(FtpClient::connect_with_tls(&profile, "weave-test", Some(without)).is_err());
        let with = TlsConnector::builder()
            .add_root_certificate(root)
            .identity(client_identity(Path::new(&identity)).unwrap())
            .build()
            .unwrap();
        let mut client = FtpClient::connect_with_tls(&profile, "weave-test", Some(with)).unwrap();
        assert!(client.list("/").is_ok());
    }

    #[test]
    fn wraps_rsa_keys_for_macos() {
        let wrapped = pkcs1_to_pkcs8(&[0x30, 0x03, 0x02, 0x01, 0x00]);
        assert_eq!(&wrapped[..2], &[0x30, 0x19]);
        assert_eq!(
            &wrapped[wrapped.len() - 7..],
            &[0x04, 0x05, 0x30, 0x03, 0x02, 0x01, 0x00]
        );
    }
}
