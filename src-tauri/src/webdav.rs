use crate::{
    files::{FileEntry, FileListing},
    profiles::Profile,
};
use quick_xml::{events::Event, Reader};
use reqwest::{
    blocking::{Body, Client, Response},
    header::{CONTENT_LENGTH, CONTENT_TYPE, RANGE},
    Method, StatusCode, Url,
};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme,
};
use rustls_platform_verifier::Verifier;
use std::{
    error::Error,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::Arc,
    time::Duration,
};
use tempfile::NamedTempFile;

const PROPFIND: &str = "PROPFIND";
const MKCOL: &str = "MKCOL";
const MOVE: &str = "MOVE";
const PROPERTIES: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/><d:getlastmodified/></d:prop></d:propfind>"#;

pub fn validate_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| {
        "Enter a complete WebDAV URL, such as https://server.example.com/dav/.".to_string()
    })?;
    if url.scheme() != "https"
        && !(url.scheme() == "http"
            && matches!(
                url.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
            ))
    {
        return Err(
            "WebDAV requires HTTPS. Plain HTTP is allowed only for a local test server.".into(),
        );
    }
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "WebDAV URL must contain a server and path, without credentials, query, or fragment."
                .into(),
        );
    }
    if url
        .path_segments()
        .is_none_or(|mut segments| segments.any(|segment| segment == ".." || segment == "."))
    {
        return Err("WebDAV URL contains an unsafe path.".into());
    }
    Ok(url)
}

#[derive(Clone)]
pub struct DavEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

pub struct WebDavClient {
    client: Client,
    base: Url,
    username: String,
    password: String,
}

#[derive(Debug)]
struct PinnedVerifier {
    trusted: Arc<dyn ServerCertVerifier>,
    pins: Vec<[u8; 32]>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let verified = self.trusted.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        )?;
        if !crate::tls_pins::matches(&self.pins, end_entity.as_ref()) {
            return Err(TlsError::General(
                "WebDAV certificate fingerprint does not match this profile".into(),
            ));
        }
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.trusted.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.trusted.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.trusted.supported_verify_schemes()
    }
}

type Identity = (
    Vec<CertificateDer<'static>>,
    rustls::pki_types::PrivateKeyDer<'static>,
);

fn identities() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::path::PathBuf>>
{
    static PATHS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::path::PathBuf>>,
    > = std::sync::OnceLock::new();
    PATHS.get_or_init(Default::default)
}

const IDENTITY_SETTING: &str = "webdav.client_identity:";

/// Loads saved client certificate choices when Weave starts.
pub fn load_client_identities(db: &rusqlite::Connection) -> rusqlite::Result<()> {
    let mut statement =
        db.prepare("SELECT key, value FROM settings WHERE key LIKE 'webdav.client_identity:%'")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut paths = identities().lock().expect("identity map");
    for row in rows {
        let (key, value) = row?;
        paths.insert(
            key.trim_start_matches(IDENTITY_SETTING).to_string(),
            value.into(),
        );
    }
    Ok(())
}

/// Reads a PEM file holding the client certificate (and any chain) plus its
/// unencrypted private key.
pub(crate) fn load_identity(path: &std::path::Path) -> Result<Identity, String> {
    use rustls::pki_types::{pem::PemObject, PrivateKeyDer};
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
    let key = PrivateKeyDer::from_pem_slice(&bytes).map_err(|_| {
        "The client certificate file has no unencrypted private key. Export it with: openssl pkcs12 -in identity.p12 -out identity.pem -nodes"
    })?;
    Ok((certificates, key))
}

pub(crate) fn client_identity_path(profile_id: &str) -> Option<std::path::PathBuf> {
    identities().lock().ok()?.get(profile_id).cloned()
}

/// Forgets a deleted host's client certificate choice.
pub(crate) fn forget_client_identity(profile_id: &str) {
    if let Ok(mut paths) = identities().lock() {
        paths.remove(profile_id);
    }
}

#[tauri::command]
pub fn webdav_client_cert_get(profile_id: String) -> Option<String> {
    identities()
        .lock()
        .ok()?
        .get(&profile_id)
        .map(|path| path.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn webdav_client_cert_set(
    state: tauri::State<'_, crate::AppState>,
    profile_id: String,
    path: Option<String>,
) -> Result<(), String> {
    let db = state
        .db
        .lock()
        .map_err(|_| "Settings are busy. Try again.")?;
    let key = format!("{IDENTITY_SETTING}{profile_id}");
    let mut paths = identities()
        .lock()
        .map_err(|_| "Settings are busy. Try again.")?;
    match path.filter(|path| !path.trim().is_empty()) {
        Some(path) => {
            load_identity(std::path::Path::new(&path))?;
            db.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [&key, &path],
            )
            .map_err(|error| format!("Could not save the client certificate choice: {error}"))?;
            paths.insert(profile_id, path.into());
        }
        None => {
            db.execute("DELETE FROM settings WHERE key=?1", [&key])
                .map_err(|error| {
                    format!("Could not remove the client certificate choice: {error}")
                })?;
            paths.remove(&profile_id);
        }
    }
    Ok(())
}

pub struct WebDavUpload {
    file: NamedTempFile,
    path: String,
    position: u64,
    modified: Option<u64>,
}

impl WebDavUpload {
    pub fn set_modified(&mut self, seconds: u64) {
        self.modified = Some(seconds);
    }
}

impl WebDavClient {
    fn connection_error(action: &str, error: reqwest::Error) -> String {
        let mut details = error.to_string();
        let mut cause = error.source();
        while let Some(inner) = cause {
            details.push_str(": ");
            details.push_str(&inner.to_string());
            cause = inner.source();
        }
        let lower = details.to_ascii_lowercase();
        if lower.contains("fingerprint does not match") {
            return format!("Could not {action}: the WebDAV certificate fingerprint does not match this profile. Check the server identity and update the fingerprint only from a trusted source.");
        }
        if lower.contains("certificate") || lower.contains("unknownissuer") {
            return format!("Could not {action}: the server's TLS certificate is not trusted. Check the server certificate and this computer's trusted certificates.");
        }
        format!("Could not {action}: {details}")
    }
    pub fn connect(profile: &Profile, password: &str) -> Result<Self, String> {
        Self::connect_with_root(profile, password, None)
    }

    fn connect_with_root(
        profile: &Profile,
        password: &str,
        trusted_test_root: Option<CertificateDer<'static>>,
    ) -> Result<Self, String> {
        let mut base = validate_url(&profile.host)?;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let pins = crate::tls_pins::parse_pins(profile.tls_pin_sha256.as_deref())?;
        if !pins.is_empty() && base.scheme() != "https" {
            return Err("Certificate pinning requires an HTTPS WebDAV URL.".into());
        }
        let identity = identities()
            .lock()
            .ok()
            .and_then(|paths| paths.get(&profile.id).cloned())
            .map(|path| load_identity(&path))
            .transpose()?;
        if identity.is_some() && base.scheme() != "https" {
            return Err("Client certificates require an HTTPS WebDAV URL.".into());
        }
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none());
        if !pins.is_empty() || identity.is_some() {
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let config = ClientConfig::builder_with_provider(provider.clone())
                .with_safe_default_protocol_versions()
                .map_err(|error| format!("Could not prepare WebDAV TLS: {error}"))?;
            let trusted: Arc<dyn ServerCertVerifier> = if let Some(root) = trusted_test_root {
                let mut roots = rustls::RootCertStore::empty();
                roots
                    .add(root)
                    .map_err(|error| format!("Could not load WebDAV test certificate: {error}"))?;
                rustls::client::WebPkiServerVerifier::builder_with_provider(
                    Arc::new(roots),
                    provider,
                )
                .build()
                .map_err(|error| format!("Could not verify WebDAV test certificate: {error}"))?
            } else {
                Arc::new(Verifier::new(provider).map_err(|error| {
                    format!("Could not prepare system certificate trust for WebDAV: {error}")
                })?)
            };
            let verifier: Arc<dyn ServerCertVerifier> = if pins.is_empty() {
                trusted
            } else {
                Arc::new(PinnedVerifier { trusted, pins })
            };
            let config = config
                .dangerous()
                .with_custom_certificate_verifier(verifier);
            let config = match identity {
                Some((certificates, key)) => config
                    .with_client_auth_cert(certificates, key)
                    .map_err(|error| format!("The client certificate cannot be used: {error}"))?,
                None => config.with_no_client_auth(),
            };
            builder = builder.tls_backend_preconfigured(config);
        }
        let client = builder
            .build()
            .map_err(|error| format!("Could not prepare WebDAV connection: {error}"))?;
        Ok(Self {
            client,
            base,
            username: profile.username.clone(),
            password: password.into(),
        })
    }

    fn url(&self, path: &str) -> Result<Url, String> {
        let path = if path == "." { "/" } else { path };
        if path == "/" {
            return Ok(self.base.clone());
        }
        if !path.starts_with('/')
            || path.contains('\0')
            || path
                .split('/')
                .any(|segment| segment == "." || segment == "..")
        {
            return Err("WebDAV path must be absolute and cannot contain parent traversal.".into());
        }
        let mut url = self.base.clone();
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "Invalid WebDAV base URL.")?;
        segments.pop_if_empty();
        for segment in path.split('/').filter(|segment| !segment.is_empty()) {
            segments.push(segment);
        }
        if path.ends_with('/') && path != "/" {
            segments.push("");
        }
        drop(segments);
        Ok(url)
    }

    fn request(
        &self,
        method: &str,
        path: &str,
    ) -> Result<reqwest::blocking::RequestBuilder, String> {
        let method = Method::from_bytes(method.as_bytes()).map_err(|_| "Invalid WebDAV method.")?;
        Ok(self
            .client
            .request(method, self.url(path)?)
            .basic_auth(&self.username, Some(&self.password)))
    }

    fn checked(response: Response, action: &str, listing: bool) -> Result<Response, String> {
        if response.status() == StatusCode::MULTI_STATUS && !listing {
            return Err(format!("Could not {action}: the WebDAV server reported partial failures. Refresh the folder to inspect what changed."));
        }
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let detail = match status {
            StatusCode::UNAUTHORIZED => "Check the WebDAV username and password.",
            StatusCode::FORBIDDEN => "Your account does not have permission for this path.",
            StatusCode::NOT_FOUND => {
                "The WebDAV path does not exist. Check the server URL and folder."
            }
            StatusCode::CONFLICT => {
                "The destination parent folder is missing or conflicts with another item."
            }
            StatusCode::LOCKED => "The server has locked this item.",
            _ => "Check the server address, permissions, and available space.",
        };
        Err(format!("Could not {action}: HTTP {status}. {detail}"))
    }

    fn properties(&self, path: &str, depth: &str) -> Result<Vec<DavEntry>, String> {
        let response = self
            .request(PROPFIND, path)?
            .header("Depth", depth)
            .header(CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(PROPERTIES)
            .send()
            .map_err(|error| Self::connection_error(&format!("list WebDAV {path}"), error))?;
        let response = Self::checked(response, &format!("list WebDAV {path}"), true)?;
        let bytes = response
            .bytes()
            .map_err(|error| format!("Could not read WebDAV listing: {error}"))?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("WebDAV listing exceeds 8 MB. Open a smaller folder.".into());
        }
        parse_multistatus(&bytes, &self.base)
    }

    pub fn list(&self, path: &str) -> Result<FileListing, String> {
        let mut items = self.properties(path, "1")?;
        let self_path = normalized_path(path);
        let folder = items
            .iter()
            .find(|item| normalized_path(&item.path) == self_path)
            .ok_or_else(|| {
                format!("WebDAV server did not include {path} in its folder response.")
            })?;
        if !folder.is_dir {
            return Err(format!("{path} is a file, not a folder."));
        }
        items.retain(|item| normalized_path(&item.path) != self_path);
        let mut entries: Vec<FileEntry> = items
            .into_iter()
            .map(|item| FileEntry {
                name: item.name,
                path: item.path,
                is_dir: item.is_dir,
                is_symlink: false,
                size: item.size,
                modified: item.modified,
                readonly: false,
            })
            .collect();
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let parent = if self_path == "/" {
            None
        } else {
            Some(
                self_path
                    .rsplit_once('/')
                    .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
                    .unwrap_or("/")
                    .to_string(),
            )
        };
        Ok(FileListing {
            path: self_path,
            parent,
            entries,
        })
    }

    pub fn stat(&self, path: &str) -> Result<DavEntry, String> {
        self.properties(path, "0")?
            .into_iter()
            .find(|item| normalized_path(&item.path) == normalized_path(path))
            .ok_or_else(|| format!("WebDAV server did not describe {path}."))
    }

    pub fn mkdir(&self, path: &str) -> Result<(), String> {
        let response = self.request(MKCOL, path)?.send().map_err(|error| {
            Self::connection_error(&format!("create WebDAV folder {path}"), error)
        })?;
        Self::checked(response, &format!("create WebDAV folder {path}"), false)?;
        Ok(())
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        let destination = self.url(to)?;
        let response = self
            .request(MOVE, from)?
            .header("Destination", destination.as_str())
            .header("Overwrite", "F")
            .send()
            .map_err(|error| Self::connection_error(&format!("move WebDAV item {from}"), error))?;
        Self::checked(response, &format!("move WebDAV item {from}"), false)?;
        Ok(())
    }

    pub fn delete(&self, path: &str, recursive: bool) -> Result<(), String> {
        if path == "/" {
            return Err("The WebDAV root cannot be deleted.".into());
        }
        if !recursive && self.stat(path)?.is_dir && !self.list(path)?.entries.is_empty() {
            return Err("This WebDAV folder is not empty. Choose recursive deletion to remove its contents.".into());
        }
        let response = self
            .request("DELETE", path)?
            .send()
            .map_err(|error| Self::connection_error(&format!("delete WebDAV {path}"), error))?;
        Self::checked(response, &format!("delete WebDAV {path}"), false)?;
        Ok(())
    }

    pub fn open_read(&self, path: &str, offset: u64) -> Result<Response, String> {
        let mut request = self
            .request("GET", path)?
            .timeout(Duration::from_secs(3600));
        if offset > 0 {
            request = request.header(RANGE, format!("bytes={offset}-"));
        }
        let response = request
            .send()
            .map_err(|error| Self::connection_error(&format!("download WebDAV {path}"), error))?;
        if offset > 0 && response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(format!("WebDAV server does not support resuming downloads of {path}. Remove the partial copy and retry."));
        }
        Self::checked(response, &format!("download WebDAV {path}"), false)
    }

    pub fn open_write(
        &self,
        path: &str,
        truncate: bool,
        offset: u64,
    ) -> Result<WebDavUpload, String> {
        let mut file = NamedTempFile::new()
            .map_err(|error| format!("Could not prepare WebDAV upload: {error}"))?;
        if !truncate {
            let mut response = self.open_read(path, 0)?;
            std::io::copy(&mut response, &mut file)
                .map_err(|error| format!("Could not resume WebDAV upload: {error}"))?;
            if file
                .as_file()
                .metadata()
                .map_err(|error| format!("Could not inspect upload spool: {error}"))?
                .len()
                != offset
            {
                return Err(
                    "The remote partial file changed. Remove it and retry the upload.".into(),
                );
            }
        }
        Ok(WebDavUpload {
            file,
            path: path.into(),
            position: offset,
            modified: None,
        })
    }

    pub fn write_chunk(upload: &mut WebDavUpload, offset: u64, data: &[u8]) -> Result<(), String> {
        if upload.position != offset {
            return Err("WebDAV upload offset changed unexpectedly. Retry the transfer.".into());
        }
        upload
            .file
            .write_all(data)
            .map_err(|error| format!("Could not spool WebDAV upload: {error}"))?;
        upload.position += data.len() as u64;
        Ok(())
    }

    pub fn finish_write(&self, mut upload: WebDavUpload) -> Result<(), String> {
        upload
            .file
            .as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(|error| format!("Could not rewind WebDAV upload: {error}"))?;
        let file = upload
            .file
            .reopen()
            .map_err(|error| format!("Could not read WebDAV upload spool: {error}"))?;
        self.put_file(&upload.path, file, upload.modified)
    }

    fn put_file(&self, path: &str, file: File, modified: Option<u64>) -> Result<(), String> {
        let length = file
            .metadata()
            .map_err(|error| format!("Could not measure WebDAV upload: {error}"))?
            .len();
        let mut request = self
            .request("PUT", path)?
            .timeout(Duration::from_secs(3600))
            .header(CONTENT_LENGTH, length)
            .body(Body::new(file));
        if let Some(seconds) = modified {
            request = request.header("X-OC-MTime", seconds.to_string());
        }
        let response = request
            .send()
            .map_err(|error| Self::connection_error(&format!("upload WebDAV {path}"), error))?;
        Self::checked(response, &format!("upload WebDAV {path}"), false)?;
        Ok(())
    }

    /// Creates a public read-only link through the Nextcloud/ownCloud share
    /// API. `expires` is a date such as 2026-12-31.
    pub fn share_link(
        &self,
        path: &str,
        expires: Option<&str>,
        link_password: Option<&str>,
    ) -> Result<String, String> {
        let (root, relative) = share_api_location(&self.base, path)?;
        let mut url = root
            .join("ocs/v2.php/apps/files_sharing/api/v1/shares")
            .map_err(|_| "Invalid server address.")?;
        url.set_query(Some("format=json"));
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair("path", &relative)
            .append_pair("shareType", "3")
            .append_pair("permissions", "1");
        if let Some(date) = expires {
            form.append_pair("expireDate", date);
        }
        if let Some(secret) = link_password.filter(|secret| !secret.is_empty()) {
            form.append_pair("password", secret);
        }
        let response = self
            .client
            .post(url)
            .basic_auth(&self.username, Some(&self.password))
            .header("OCS-APIRequest", "true")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form.finish())
            .send()
            .map_err(|error| Self::connection_error("create the share link", error))?;
        let status = response.status();
        let body: serde_json::Value = response.json().unwrap_or_default();
        let meta = &body["ocs"]["meta"];
        if let Some(link) = body["ocs"]["data"]["url"]
            .as_str()
            .filter(|_| status.is_success())
        {
            return Ok(link.to_string());
        }
        let message = meta["message"].as_str().filter(|text| !text.is_empty());
        Err(match (status, message) {
            (StatusCode::UNAUTHORIZED, _) => "Could not create the share link: check the username and password.".into(),
            (_, Some(text)) => format!("The server refused the share link: {text}"),
            _ => format!("Could not create the share link: HTTP {status}. Public link sharing may be turned off on this server."),
        })
    }

    pub fn read_small(&self, path: &str, max: u64) -> Result<Vec<u8>, String> {
        let mut response = self.open_read(path, 0)?;
        let mut bytes = Vec::new();
        response
            .by_ref()
            .take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("Could not read WebDAV {path}: {error}"))?;
        if bytes.len() as u64 > max {
            return Err("This remote file is larger than the 1 MB editor limit.".into());
        }
        Ok(bytes)
    }

    pub fn write_small(&self, path: &str, data: &[u8]) -> Result<(), String> {
        let mut upload = self.open_write(path, true, 0)?;
        Self::write_chunk(&mut upload, 0, data)?;
        self.finish_write(upload)
    }
}

/// Splits a Nextcloud or ownCloud WebDAV URL into the server root and the
/// file path relative to the user's files, as the share API expects.
pub(crate) fn share_api_location(base: &Url, path: &str) -> Result<(Url, String), String> {
    let base_path = base.path();
    let index = base_path.find("/remote.php/").ok_or(
        "Share links work with Nextcloud and ownCloud servers, whose WebDAV address contains /remote.php/.",
    )?;
    let after = &base_path[index + "/remote.php/".len()..];
    let inside = if let Some(rest) = after.strip_prefix("dav/files/") {
        rest.split_once('/').map(|(_, inside)| inside).unwrap_or("")
    } else if let Some(rest) = after.strip_prefix("webdav/") {
        rest
    } else {
        after.strip_prefix("webdav").unwrap_or("")
    };
    let inside = percent_encoding::percent_decode_str(inside).decode_utf8_lossy();
    let relative = format!(
        "/{}/{}",
        inside.trim_matches('/'),
        path.trim_start_matches('/')
    )
    .replace("//", "/");
    let mut root = base.clone();
    root.set_path(&base_path[..index + 1]);
    let relative = match relative.trim_end_matches('/') {
        "" => "/".to_string(),
        trimmed => trimmed.to_string(),
    };
    Ok((root, relative))
}

fn normalized_path(path: &str) -> String {
    if path == "/" || path == "." {
        "/".into()
    } else {
        path.trim_end_matches('/').to_string()
    }
}

fn parse_multistatus(xml: &[u8], base: &Url) -> Result<Vec<DavEntry>, String> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut stack = Vec::<String>::new();
    let mut href = String::new();
    let mut is_dir = false;
    let mut size = 0;
    let mut modified = None;
    let mut entries = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = String::from_utf8_lossy(event.local_name().as_ref()).to_string();
                if name == "response" {
                    href.clear();
                    is_dir = false;
                    size = 0;
                    modified = None;
                }
                if name == "collection" {
                    is_dir = true;
                }
                stack.push(name);
            }
            Ok(Event::Empty(event)) => {
                if event.local_name().as_ref() == b"collection" {
                    is_dir = true;
                }
            }
            Ok(Event::Text(event)) => {
                let value = event
                    .decode()
                    .map_err(|error| format!("Invalid WebDAV XML text: {error}"))?;
                let value = quick_xml::escape::unescape(&value)
                    .map_err(|error| format!("Invalid WebDAV XML entity: {error}"))?;
                match stack.last().map(String::as_str) {
                    Some("href") => href.push_str(&value),
                    Some("getcontentlength") => size = value.parse().unwrap_or(0),
                    Some("getlastmodified") => {
                        modified = httpdate::parse_http_date(&value)
                            .ok()
                            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|duration| duration.as_secs())
                    }
                    _ => {}
                }
            }
            Ok(Event::End(event)) => {
                if event.local_name().as_ref() == b"response" && !href.is_empty() {
                    let uri = base
                        .join(&href)
                        .map_err(|_| "WebDAV server returned an invalid item URL.")?;
                    let root = base.path();
                    if uri.origin() != base.origin() || !uri.path().starts_with(root) {
                        return Err(
                            "WebDAV server returned an item outside the configured folder.".into(),
                        );
                    }
                    let relative = uri
                        .path()
                        .strip_prefix(root)
                        .unwrap_or("")
                        .trim_end_matches('/');
                    let mut parts = Vec::new();
                    for segment in relative.split('/').filter(|part| !part.is_empty()) {
                        let decoded = percent_encoding::percent_decode_str(segment)
                            .decode_utf8()
                            .map_err(|_| "WebDAV item has an invalid UTF-8 name.")?;
                        if decoded == "." || decoded == ".." || decoded.contains('/') {
                            return Err("WebDAV server returned an unsafe item path.".into());
                        }
                        parts.push(decoded.to_string());
                    }
                    let path = format!("/{}", parts.join("/"));
                    let name = parts.last().cloned().unwrap_or_else(|| "/".into());
                    entries.push(DavEntry {
                        path,
                        name,
                        is_dir,
                        size,
                        modified,
                    });
                }
                stack.pop();
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("WebDAV server returned invalid XML: {error}")),
            _ => {}
        }
    }
    if entries.is_empty() {
        return Err("WebDAV server returned no readable file entries.".into());
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn make_certificates(dir: &std::path::Path) {
        let script = r#"set -e
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout ca.key -out ca.pem -days 2 -subj /CN=WeaveTestCA 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout server.key -out server.csr -subj /CN=127.0.0.1 2>/dev/null
printf 'subjectAltName=IP:127.0.0.1\n' > server.ext
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out server.pem -days 2 -extfile server.ext 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout client.key -out client.csr -subj /CN=weave-client 2>/dev/null
printf 'extendedKeyUsage=clientAuth\n' > client.ext
openssl x509 -req -in client.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out client.pem -days 2 -extfile client.ext 2>/dev/null
cat client.pem client.key > identity.pem
"#;
        let status = crate::platform::command("/bin/sh")
            .arg("-c")
            .arg(script)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn presents_a_client_certificate_to_servers_that_require_one() {
        use rustls::pki_types::{pem::PemObject, PrivateKeyDer};
        use std::net::TcpListener;
        let dir = tempfile::tempdir().unwrap();
        make_certificates(dir.path());
        let read = |name: &str| std::fs::read(dir.path().join(name)).unwrap();
        let ca = CertificateDer::from_pem_slice(&read("ca.pem")).unwrap();
        let server_cert = CertificateDer::from_pem_slice(&read("server.pem")).unwrap();
        let server_key = PrivateKeyDer::from_pem_slice(&read("server.key")).unwrap();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut roots = rustls::RootCertStore::empty();
        roots.add(ca.clone()).unwrap();
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            provider.clone(),
        )
        .build()
        .unwrap();
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_client_cert_verifier(verifier)
                .with_single_cert(vec![server_cert.clone()], server_key)
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let connection = rustls::ServerConnection::new(config.clone()).unwrap();
                let mut tls = rustls::StreamOwned::new(connection, stream.unwrap());
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    match tls.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => request.extend_from_slice(&buffer[..count]),
                    }
                }
                let _ = tls.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                );
                let _ = tls.flush();
            }
        });
        let pin: String = Sha256::digest(server_cert.as_ref())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let profile = Profile {
            id: "client-cert-test".into(),
            name: "mTLS".into(),
            host: format!("https://127.0.0.1:{port}/dav/"),
            protocol: "webdav".into(),
            port: 443,
            username: "alice".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: Some(pin),
            s3_region: None,
        };
        let without = WebDavClient::connect_with_root(&profile, "pw", Some(ca.clone())).unwrap();
        assert!(without.read_small("/note.txt", 100).is_err());
        identities()
            .lock()
            .unwrap()
            .insert(profile.id.clone(), dir.path().join("identity.pem"));
        let with = WebDavClient::connect_with_root(&profile, "pw", Some(ca)).unwrap();
        assert_eq!(with.read_small("/note.txt", 100).unwrap(), b"hello");
        identities().lock().unwrap().remove(&profile.id);
        assert!(load_identity(&dir.path().join("ca.pem"))
            .err()
            .unwrap()
            .contains("private key"));
    }

    #[test]
    fn finds_the_share_api_for_nextcloud_and_owncloud_urls() {
        let nextcloud =
            Url::parse("https://cloud.example.test/nc/remote.php/dav/files/alice/Work/").unwrap();
        let (root, path) = share_api_location(&nextcloud, "/Report 1.pdf").unwrap();
        assert_eq!(root.as_str(), "https://cloud.example.test/nc/");
        assert_eq!(path, "/Work/Report 1.pdf");
        let owncloud = Url::parse("https://oc.example.test/remote.php/webdav/").unwrap();
        assert_eq!(share_api_location(&owncloud, "/a/b").unwrap().1, "/a/b");
        assert_eq!(share_api_location(&owncloud, "/").unwrap().1, "/");
        let plain = Url::parse("https://dav.example.test/files/").unwrap();
        assert!(share_api_location(&plain, "/a")
            .unwrap_err()
            .contains("Nextcloud"));
    }

    #[test]
    fn creates_a_share_link_through_the_ocs_api() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for reply in [
                r#"{"ocs":{"meta":{"status":"ok","statuscode":200,"message":"OK"},"data":{"url":"https://cloud.example.test/s/AbC123"}}}"#,
                r#"{"ocs":{"meta":{"status":"failure","statuscode":403,"message":"Public upload is not allowed"},"data":[]}}"#,
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    request.extend_from_slice(&buffer[..count]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let status = if reply.contains("failure") {
                    "403 Forbidden"
                } else {
                    "200 OK"
                };
                write!(stream, "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{reply}", reply.len()).unwrap();
                requests.push(String::from_utf8(request).unwrap());
            }
            requests
        });
        let profile = Profile {
            id: "nc".into(),
            name: "Cloud".into(),
            host: format!("http://127.0.0.1:{port}/remote.php/dav/files/alice/"),
            protocol: "webdav".into(),
            port: 443,
            username: "alice".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let client = WebDavClient::connect(&profile, "secret").unwrap();
        assert_eq!(
            client
                .share_link("/Docs/plan.pdf", Some("2026-12-31"), Some("pw"))
                .unwrap(),
            "https://cloud.example.test/s/AbC123"
        );
        assert_eq!(
            client.share_link("/Docs", None, None).unwrap_err(),
            "The server refused the share link: Public upload is not allowed"
        );
        let requests = server.join().unwrap();
        assert!(requests[0]
            .starts_with("POST /ocs/v2.php/apps/files_sharing/api/v1/shares?format=json HTTP/1.1"));
        assert!(requests[0]
            .to_ascii_lowercase()
            .contains("ocs-apirequest: true"));
        assert!(requests[0].ends_with(
            "path=%2FDocs%2Fplan.pdf&shareType=3&permissions=1&expireDate=2026-12-31&password=pw"
        ));
    }

    #[test]
    fn validates_secure_server_urls() {
        assert!(validate_url("https://example.com/dav/").is_ok());
        assert!(validate_url("http://127.0.0.1:8080/dav/").is_ok());
        assert!(validate_url("http://example.com/dav/").is_err());
        assert!(validate_url("https://user:secret@example.com/dav/").is_err());
    }

    #[test]
    fn parses_dav_namespaces_and_encoded_names() {
        let xml = br#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response><d:response><d:href>/dav/hello%20world.txt</d:href><d:propstat><d:prop><d:getcontentlength>42</d:getcontentlength></d:prop></d:propstat></d:response></d:multistatus>"#;
        let entries =
            parse_multistatus(xml, &Url::parse("https://example.com/dav/").unwrap()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].path, "/hello world.txt");
        assert_eq!(entries[1].size, 42);
    }

    #[test]
    #[ignore = "requires the local WebDAV test server"]
    fn authenticates_and_manages_folders_and_resumable_files() {
        let port = std::env::var("WEAVE_WEBDAV_TEST_PORT").expect("Set WEAVE_WEBDAV_TEST_PORT");
        let profile = Profile {
            id: "webdav-test".into(),
            name: "Local WebDAV".into(),
            host: format!("http://127.0.0.1:{port}/dav/"),
            protocol: "webdav".into(),
            port: 443,
            username: "weave".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let denied = WebDavClient::connect(&profile, "incorrect").unwrap();
        assert!(denied.list(".").err().unwrap().contains("HTTP 401"));
        let client = WebDavClient::connect(&profile, "weave-test").unwrap();
        assert_eq!(client.list(".").unwrap().path, "/");
        let folder = format!("/weave {}", uuid::Uuid::new_v4());
        client.mkdir(&folder).unwrap();
        let source = format!("{folder}/data & file.txt");
        let renamed = format!("{folder}/renamed.txt");
        client.write_small(&source, b"hello world").unwrap();
        assert_eq!(client.stat(&source).unwrap().size, 11);
        let mut timed = client.open_write(&source, true, 0).unwrap();
        WebDavClient::write_chunk(&mut timed, 0, b"hello world").unwrap();
        timed.set_modified(1_600_000_000);
        client.finish_write(timed).unwrap();
        assert_eq!(client.stat(&source).unwrap().modified, Some(1_600_000_000));
        assert_eq!(
            client.list(&folder).unwrap().entries[0].name,
            "data & file.txt"
        );
        let mut remaining = Vec::new();
        client
            .open_read(&source, 6)
            .unwrap()
            .read_to_end(&mut remaining)
            .unwrap();
        assert_eq!(remaining, b"world");
        let mut resumed = client.open_write(&source, false, 11).unwrap();
        WebDavClient::write_chunk(&mut resumed, 11, b" again").unwrap();
        client.finish_write(resumed).unwrap();
        assert_eq!(
            client.read_small(&source, 1_048_576).unwrap(),
            b"hello world again"
        );
        client.rename(&source, &renamed).unwrap();
        assert!(client
            .delete(&folder, false)
            .unwrap_err()
            .contains("not empty"));
        client.delete(&renamed, false).unwrap();
        client.delete(&folder, false).unwrap();
    }

    #[test]
    #[ignore = "requires the local WebDAV TLS test server"]
    fn webdav_rejects_an_untrusted_tls_certificate() {
        use rustls::pki_types::pem::PemObject;
        let port =
            std::env::var("WEAVE_WEBDAV_TLS_TEST_PORT").expect("Set WEAVE_WEBDAV_TLS_TEST_PORT");
        let mut profile = Profile {
            id: "webdav-tls-test".into(),
            name: "Local WebDAV TLS".into(),
            host: format!("https://127.0.0.1:{port}/dav/"),
            protocol: "webdav".into(),
            port: 443,
            username: "weave".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let error = WebDavClient::connect(&profile, "weave-test")
            .unwrap()
            .list(".")
            .err()
            .expect("Untrusted certificate was accepted");
        assert!(
            error.contains("certificate") || error.contains("issuer"),
            "Unexpected TLS error: {error}"
        );
        let certificate_path = std::env::var("WEAVE_WEBDAV_TLS_TEST_CERT")
            .expect("Set WEAVE_WEBDAV_TLS_TEST_CERT to the local test certificate");
        let certificate = CertificateDer::from_pem_file(certificate_path)
            .expect("Read the local WebDAV test certificate");
        let root_path = std::env::var("WEAVE_WEBDAV_TLS_TEST_CA")
            .expect("Set WEAVE_WEBDAV_TLS_TEST_CA to the local test CA certificate");
        let root = CertificateDer::from_pem_file(root_path)
            .expect("Read the local WebDAV test CA certificate");
        let pin = format!("{:x}", Sha256::digest(certificate.as_ref()));
        profile.tls_pin_sha256 = Some(pin);
        WebDavClient::connect_with_root(&profile, "weave-test", Some(root.clone()))
            .unwrap()
            .list(".")
            .unwrap();
        profile.tls_pin_sha256 = Some("00".repeat(32));
        let error = WebDavClient::connect_with_root(&profile, "weave-test", Some(root))
            .unwrap()
            .list(".")
            .err()
            .expect("A mismatched WebDAV certificate fingerprint was accepted");
        assert!(
            error.contains("fingerprint"),
            "Unexpected TLS error: {error}"
        );
    }
}
