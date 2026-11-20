use crate::{
    files::{FileEntry, FileListing},
    profiles::{self, Profile},
    vault, AppState,
};
use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use md5::Md5;
use percent_encoding::percent_decode_str;
use quick_xml::{events::Event, Reader};
use reqwest::{
    blocking::{Body, Client, RequestBuilder, Response},
    header::{CONTENT_LENGTH, RANGE},
    Method, StatusCode, Url,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};
use tauri::{AppHandle, Manager};
use tempfile::NamedTempFile;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;
const MAX_SINGLE_UPLOAD: u64 = 5 * 1024 * 1024 * 1024;
const MULTIPART_THRESHOLD: u64 = 64 * 1024 * 1024;
const MULTIPART_PART_SIZE: u64 = 16 * 1024 * 1024;
const COPY_PART_SIZE: u64 = 512 * 1024 * 1024;

#[derive(Clone)]
pub struct S3Entry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

pub struct S3Upload {
    file: NamedTempFile,
    path: String,
    position: u64,
}

pub struct S3Client {
    client: Client,
    endpoint: Url,
    bucket: String,
    virtual_hosted: bool,
    region: String,
    access_key: String,
    secret: Zeroizing<String>,
    session_token: Option<Zeroizing<String>>,
    /// S3 Express One Zone buckets sign with short-lived session keys.
    express: Option<Mutex<Option<ExpressSession>>>,
}

/// Access key, secret and session token.
type SessionKeys = (String, Zeroizing<String>, Zeroizing<String>);

struct ExpressSession {
    access_key: String,
    secret: Zeroizing<String>,
    token: Zeroizing<String>,
    renew_after: Instant,
}

/// Directory bucket names end in --x-s3, such as photos--use1-az4--x-s3.
fn directory_bucket(bucket: &str) -> bool {
    bucket.ends_with("--x-s3")
}

fn encode(value: &str, keep_slash: bool) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (keep_slash && byte == b'/')
        {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hmac(key: &[u8], message: &[u8]) -> Result<[u8; 32], String> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|_| "Could not prepare S3 request signing.")?;
    mac.update(message);
    Ok(mac.finalize().into_bytes().into())
}

/// SHA-256 for signing and MD5 to match the part's ETag when resuming.
fn hash_file_range(
    path: &std::path::Path,
    offset: u64,
    length: u64,
) -> Result<(String, String), String> {
    let mut file =
        File::open(path).map_err(|error| format!("Could not read S3 upload: {error}"))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("Could not seek in S3 upload: {error}"))?;
    let mut remaining = length;
    let mut digest = Sha256::new();
    let mut md5 = Md5::new();
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let chunk_size = buffer.len().min(remaining as usize);
        let count = file
            .read(&mut buffer[..chunk_size])
            .map_err(|error| format!("Could not hash S3 upload part: {error}"))?;
        if count == 0 {
            return Err("S3 upload file changed while a part was being prepared.".into());
        }
        digest.update(&buffer[..count]);
        md5.update(&buffer[..count]);
        remaining -= count as u64;
    }
    Ok((hex(&digest.finalize()), hex(&md5.finalize())))
}

/// Text for `&quot;`, `&#34;` and the other references quick-xml reports separately.
fn entity(reference: &quick_xml::events::BytesRef) -> String {
    if let Ok(Some(character)) = reference.resolve_char_ref() {
        return character.to_string();
    }
    match &**reference {
        b"quot" => "\"",
        b"amp" => "&",
        b"lt" => "<",
        b"gt" => ">",
        b"apos" => "'",
        _ => "",
    }
    .to_string()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_root(bytes: &[u8], expected: &str) -> Result<(), String> {
    let mut reader = Reader::from_reader(bytes);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                if event.local_name().as_ref() == expected.as_bytes() {
                    return Ok(());
                }
                if event.local_name().as_ref() == b"Error" {
                    let code =
                        xml_field(bytes, "Error", "Code").unwrap_or_else(|_| "UnknownError".into());
                    let message = xml_field(bytes, "Error", "Message").ok();
                    return Err(format!(
                        "S3 server rejected {expected}: {code}{}",
                        message
                            .map(|value| format!(": {value}"))
                            .unwrap_or_default()
                    ));
                }
                return Err(format!(
                    "S3 server returned an unexpected {expected} response."
                ));
            }
            Ok(Event::Eof) | Err(_) => {
                return Err(format!(
                    "S3 server returned an invalid {expected} response."
                ));
            }
            _ => {}
        }
    }
}

fn xml_field(bytes: &[u8], root: &str, field: &str) -> Result<String, String> {
    xml_root(bytes, root)?;
    let mut reader = Reader::from_reader(bytes);
    let mut in_field = false;
    let mut value = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if event.local_name().as_ref() == field.as_bytes() => {
                in_field = true;
            }
            Ok(Event::Text(event)) if in_field => {
                let decoded = event
                    .decode()
                    .map_err(|error| format!("Invalid S3 {field}: {error}"))?;
                value.push_str(
                    &quick_xml::escape::unescape(&decoded)
                        .map_err(|error| format!("Invalid S3 {field}: {error}"))?,
                );
            }
            Ok(Event::GeneralRef(reference)) if in_field => value.push_str(&entity(&reference)),
            Ok(Event::End(event)) if event.local_name().as_ref() == field.as_bytes() => break,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    if value.is_empty() {
        return Err(format!("S3 server did not return a {field}."));
    }
    Ok(value)
}

/// Repeated elements such as <Part> or <Upload>, each as a field map.
pub(crate) fn xml_records(bytes: &[u8], record: &str) -> Vec<HashMap<String, String>> {
    let mut reader = Reader::from_reader(bytes);
    let mut records = Vec::new();
    let mut current: Option<HashMap<String, String>> = None;
    let mut field: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = String::from_utf8_lossy(event.local_name().as_ref()).to_string();
                if name == record {
                    current = Some(HashMap::new());
                } else if current.is_some() {
                    field = Some(name);
                }
            }
            Ok(Event::Text(event)) => {
                if let (Some(record), Some(name)) = (current.as_mut(), field.as_ref()) {
                    if let Ok(text) = event.decode() {
                        let text = quick_xml::escape::unescape(&text)
                            .map(|text| text.into_owned())
                            .unwrap_or_default();
                        record.entry(name.clone()).or_default().push_str(&text);
                    }
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let (Some(record), Some(name)) = (current.as_mut(), field.as_ref()) {
                    record
                        .entry(name.clone())
                        .or_default()
                        .push_str(&entity(&reference));
                }
            }
            Ok(Event::End(event)) => {
                if event.local_name().as_ref() == record.as_bytes() {
                    records.extend(current.take());
                }
                field = None;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    records
}

/// The bucket of a virtual-hosted URL such as https://my-bucket.s3.amazonaws.com.
fn virtual_bucket(endpoint: &Url) -> Option<&str> {
    let host = endpoint.host_str()?;
    let (bucket, rest) = host.split_once('.')?;
    (rest.contains('.') || rest == "localhost").then_some(bucket)
}

pub fn validate_profile(profile: &Profile) -> Result<(), String> {
    let endpoint = Url::parse(&profile.host).map_err(|_| {
        "Enter an S3 bucket URL, such as https://s3.eu-central-1.amazonaws.com/my-bucket."
    })?;
    let local = endpoint.host_str().is_some_and(|host| {
        matches!(host, "localhost" | "127.0.0.1" | "::1") || host.ends_with(".localhost")
    });
    if endpoint.scheme() != "https" && !(endpoint.scheme() == "http" && local) {
        return Err(
            "S3 requires HTTPS. Plain HTTP is allowed only for a local test server.".into(),
        );
    }
    if endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err("S3 bucket URL must not contain credentials, a query, or a fragment.".into());
    }
    let segments = endpoint
        .path_segments()
        .ok_or("Enter a bucket in the S3 URL path.")?
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let bucket = match segments.as_slice() {
        [bucket] => *bucket,
        [] => virtual_bucket(&endpoint).ok_or(
            "Put the bucket in the URL path (https://s3.example.com/my-bucket) or the host name (https://my-bucket.s3.example.com).",
        )?,
        _ => {
            return Err(
                "S3 bucket URL must end with exactly one bucket name, for example /my-bucket."
                    .into(),
            )
        }
    };
    if bucket == "."
        || bucket == ".."
        || !bucket
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_'))
    {
        return Err(
            "The S3 bucket name may use letters, numbers, dots, dashes and underscores.".into(),
        );
    }
    let region = profile.s3_region.as_deref().unwrap_or("");
    if region.is_empty()
        || region.len() > 64
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("Enter an S3 signing region, such as eu-central-1 or auto.".into());
    }
    if profile.username.is_empty() || profile.username.contains(char::is_whitespace) {
        return Err("Enter the S3 access key ID without spaces.".into());
    }
    Ok(())
}

impl S3Client {
    fn signing_key(&self, secret: &str, service: &str, date: &str) -> Result<[u8; 32], String> {
        let dated = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes())?;
        let regional = hmac(&dated, self.region.as_bytes())?;
        let service = hmac(&regional, service.as_bytes())?;
        hmac(&service, b"aws4_request")
    }

    fn signature(&self, date: &str, timestamp: &str, canonical: &str) -> Result<String, String> {
        self.signature_with(&self.secret, "s3", date, timestamp, canonical)
    }

    fn signature_with(
        &self,
        secret: &str,
        service: &str,
        date: &str,
        timestamp: &str,
        canonical: &str,
    ) -> Result<String, String> {
        let scope = format!("{date}/{}/{service}/aws4_request", self.region);
        let signed = format!(
            "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
            hex(&Sha256::digest(canonical.as_bytes()))
        );
        Ok(hex(&hmac(
            &self.signing_key(secret, service, date)?,
            signed.as_bytes(),
        )?))
    }

    fn authority(url: &Url) -> String {
        match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().unwrap_or("")),
            None => url.host_str().unwrap_or("").to_string(),
        }
    }

    #[cfg(test)]
    pub fn connect(profile: &Profile, secret: &str) -> Result<Self, String> {
        Self::connect_with_token(profile, secret, None)
    }

    pub fn connect_with_token(
        profile: &Profile,
        secret: &str,
        session_token: Option<&str>,
    ) -> Result<Self, String> {
        validate_profile(profile)?;
        if secret.is_empty() {
            return Err(
                "Enter the S3 secret access key in this connection's password field.".into(),
            );
        }
        if session_token.is_some_and(|token| {
            token.is_empty() || token.len() > 8192 || token.chars().any(char::is_whitespace)
        }) {
            return Err(
                "S3 session token must be nonempty and contain no spaces, up to 8192 characters."
                    .into(),
            );
        }
        let mut endpoint =
            Url::parse(&profile.host).map_err(|error| format!("Invalid S3 bucket URL: {error}"))?;
        let path_bucket = endpoint.path().trim_matches('/').to_string();
        let virtual_hosted = path_bucket.is_empty();
        let bucket = if virtual_hosted {
            virtual_bucket(&endpoint).unwrap_or_default().to_string()
        } else {
            path_bucket
        };
        endpoint.set_path("/");
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| format!("Could not prepare S3 connection: {error}"))?;
        Ok(Self {
            client,
            endpoint,
            virtual_hosted,
            region: profile.s3_region.clone().unwrap_or_default(),
            access_key: profile.username.clone(),
            secret: Zeroizing::new(secret.into()),
            session_token: session_token.map(|token| Zeroizing::new(token.into())),
            express: directory_bucket(&bucket).then(|| Mutex::new(None)),
            bucket,
        })
    }

    fn key(path: &str) -> Result<&str, String> {
        if path == "/" {
            return Ok("");
        }
        if !path.starts_with('/')
            || path.contains('\0')
            || path.split('/').any(|part| part == "." || part == "..")
        {
            return Err("S3 path must be absolute and cannot contain parent traversal.".into());
        }
        Ok(path.trim_start_matches('/'))
    }

    fn url(&self, key: &str, query: &[(String, String)]) -> Result<Url, String> {
        let mut url = self.endpoint.clone();
        if self.virtual_hosted {
            url.set_path(&format!("/{}", encode(key, true)));
        } else {
            url.set_path(&format!("/{}/{}", self.bucket, encode(key, true)));
        }
        let mut pairs = query.to_vec();
        pairs.sort();
        if !pairs.is_empty() {
            url.set_query(Some(
                &pairs
                    .iter()
                    .map(|(key, value)| format!("{}={}", encode(key, false), encode(value, false)))
                    .collect::<Vec<_>>()
                    .join("&"),
            ));
        }
        Ok(url)
    }

    /// Signs a request. Directory buckets use a session from CreateSession.
    fn request(
        &self,
        method: Method,
        key: &str,
        query: &[(String, String)],
        body_hash: &str,
        extra: &[(&str, String)],
    ) -> Result<RequestBuilder, String> {
        match self.express_credentials()? {
            Some((access_key, secret, token)) => {
                let mut extra = extra.to_vec();
                extra.push(("x-amz-s3session-token", token.to_string()));
                self.signed(
                    method,
                    key,
                    query,
                    body_hash,
                    &extra,
                    &access_key,
                    &secret,
                    "s3express",
                    false,
                )
            }
            None => self.signed(
                method,
                key,
                query,
                body_hash,
                extra,
                &self.access_key,
                &self.secret,
                "s3",
                true,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn signed(
        &self,
        method: Method,
        key: &str,
        query: &[(String, String)],
        body_hash: &str,
        extra: &[(&str, String)],
        access_key: &str,
        secret: &str,
        service: &str,
        with_session_token: bool,
    ) -> Result<RequestBuilder, String> {
        let url = self.url(key, query)?;
        let now: DateTime<Utc> = SystemTime::now().into();
        let date = now.format("%Y%m%d").to_string();
        let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let mut headers = vec![
            ("host", Self::authority(&url)),
            ("x-amz-content-sha256", body_hash.to_string()),
            ("x-amz-date", timestamp.clone()),
        ];
        headers.extend(extra.iter().cloned());
        if let Some(token) = self.session_token.as_ref().filter(|_| with_session_token) {
            headers.push(("x-amz-security-token", token.to_string()));
        }
        headers.sort_by_key(|(name, _)| *name);
        let signed_headers = headers
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(";");
        let canonical_headers = headers
            .iter()
            .map(|(name, value)| format!("{name}:{}\n", value.trim()))
            .collect::<String>();
        let canonical = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method.as_str(),
            url.path(),
            url.query().unwrap_or(""),
            canonical_headers,
            signed_headers,
            body_hash
        );
        let scope = format!("{date}/{}/{service}/aws4_request", self.region);
        let signature = self.signature_with(secret, service, &date, &timestamp, &canonical)?;
        let authorization =
            format!("AWS4-HMAC-SHA256 Credential={access_key}/{scope},SignedHeaders={signed_headers},Signature={signature}");
        let mut request = self
            .client
            .request(method, url)
            .header("Authorization", authorization);
        for (name, value) in headers.into_iter().filter(|(name, _)| *name != "host") {
            request = request.header(name, value);
        }
        Ok(request)
    }

    /// Session keys for a directory bucket, renewed a minute before they expire.
    fn express_credentials(&self) -> Result<Option<SessionKeys>, String> {
        let Some(express) = &self.express else {
            return Ok(None);
        };
        let mut session = express.lock().map_err(|_| "The S3 session is busy.")?;
        if session
            .as_ref()
            .is_none_or(|current| Instant::now() >= current.renew_after)
        {
            let request = self.signed(
                Method::GET,
                "",
                &[("session".into(), String::new())],
                &hex(&Sha256::digest(b"")),
                &[],
                &self.access_key,
                &self.secret,
                "s3express",
                true,
            )?;
            let bytes = self
                .send(request, "start an S3 directory bucket session")?
                .bytes()
                .map_err(|error| format!("Could not read the S3 session: {error}"))?;
            let field = |name| xml_field(&bytes, "CreateSessionResult", name);
            *session = Some(ExpressSession {
                access_key: field("AccessKeyId")?,
                secret: Zeroizing::new(field("SecretAccessKey")?),
                token: Zeroizing::new(field("SessionToken")?),
                renew_after: Instant::now() + Duration::from_secs(240),
            });
        }
        let current = session
            .as_ref()
            .ok_or("The S3 session could not be started.")?;
        Ok(Some((
            current.access_key.clone(),
            current.secret.clone(),
            current.token.clone(),
        )))
    }

    pub fn presign_get(&self, path: &str, expires_seconds: u32) -> Result<String, String> {
        if self.express.is_some() {
            return Err("S3 directory buckets do not support share links from Weave. Use a general purpose bucket to share files.".into());
        }
        if !(60..=604_800).contains(&expires_seconds) {
            return Err("Choose a share link lifetime between one minute and seven days.".into());
        }
        let item = self.stat(path)?;
        if item.is_dir {
            return Err("Choose a single S3 file to share, not a folder.".into());
        }
        let key = Self::key(&item.path)?;
        let now: DateTime<Utc> = SystemTime::now().into();
        let date = now.format("%Y%m%d").to_string();
        let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let mut query = vec![
            ("X-Amz-Algorithm".into(), "AWS4-HMAC-SHA256".into()),
            (
                "X-Amz-Credential".into(),
                format!("{}/{scope}", self.access_key),
            ),
            ("X-Amz-Date".into(), timestamp.clone()),
            ("X-Amz-Expires".into(), expires_seconds.to_string()),
            ("X-Amz-SignedHeaders".into(), "host".into()),
        ];
        if let Some(token) = &self.session_token {
            query.push(("X-Amz-Security-Token".into(), token.to_string()));
        }
        let mut url = self.url(key, &query)?;
        let canonical = format!(
            "GET\n{}\n{}\nhost:{}\n\nhost\nUNSIGNED-PAYLOAD",
            url.path(),
            url.query().unwrap_or(""),
            Self::authority(&url)
        );
        let signature = self.signature(&date, &timestamp, &canonical)?;
        let signed_query = format!("{}&X-Amz-Signature={signature}", url.query().unwrap_or(""));
        url.set_query(Some(&signed_query));
        Ok(url.into())
    }

    fn checked(response: Response, action: &str) -> Result<Response, String> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let hint = match status {
            StatusCode::FORBIDDEN => {
                "Check the access key, secret key, session token expiry, signing region, and bucket permissions."
            }
            StatusCode::NOT_FOUND => "The bucket or object does not exist.",
            StatusCode::MOVED_PERMANENTLY | StatusCode::TEMPORARY_REDIRECT => {
                "Use the bucket's regional endpoint URL."
            }
            StatusCode::REQUEST_TIMEOUT => "The S3 server timed out. Retry the operation.",
            _ => "Check the bucket URL, permissions, and server response.",
        };
        Err(format!("Could not {action}: HTTP {status}. {hint}"))
    }

    fn send(&self, request: RequestBuilder, action: &str) -> Result<Response, String> {
        let response = request
            .send()
            .map_err(|error| format!("Could not {action}: {error}"))?;
        Self::checked(response, action)
    }

    fn list_page(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
        token: Option<&str>,
    ) -> Result<S3Page, String> {
        let mut query = vec![
            ("list-type".into(), "2".into()),
            ("prefix".into(), prefix.into()),
            ("max-keys".into(), "1000".into()),
            ("encoding-type".into(), "url".into()),
        ];
        if let Some(delimiter) = delimiter {
            query.push(("delimiter".into(), delimiter.into()));
        }
        if let Some(token) = token {
            query.push(("continuation-token".into(), token.into()));
        }
        let request = self.request(Method::GET, "", &query, &hex(&Sha256::digest(b"")), &[])?;
        let response = self.send(request, "list S3 objects")?;
        let bytes = response
            .bytes()
            .map_err(|error| format!("Could not read S3 listing: {error}"))?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("S3 listing exceeds 16 MB. Open a smaller prefix.".into());
        }
        parse_list(&bytes)
    }

    fn objects(&self, prefix: &str, delimiter: Option<&str>) -> Result<Vec<S3Object>, String> {
        let mut results = Vec::new();
        let mut token = None;
        let mut seen = HashSet::new();
        loop {
            let page = self.list_page(prefix, delimiter, token.as_deref())?;
            results.extend(page.objects);
            if results.len() > 20_000 {
                return Err(
                    "S3 folder has more than 20,000 objects. Choose a smaller folder.".into(),
                );
            }
            match page.next_token {
                Some(next) if seen.insert(next.clone()) => token = Some(next),
                Some(_) => return Err("S3 server repeated a listing continuation token.".into()),
                None => break,
            }
        }
        Ok(results)
    }

    pub fn list(&self, path: &str) -> Result<FileListing, String> {
        let key = Self::key(path)?;
        let prefix = if key.is_empty() {
            String::new()
        } else {
            format!("{}/", key.trim_end_matches('/'))
        };
        let mut entries = Vec::new();
        let objects = self.objects(&prefix, Some("/"))?;
        if !key.is_empty() && objects.is_empty() {
            return Err(format!(
                "S3 folder /{} does not exist.",
                key.trim_end_matches('/')
            ));
        }
        for object in objects {
            if object.key == prefix {
                continue;
            }
            let relative = object
                .key
                .strip_prefix(&prefix)
                .ok_or("S3 listing returned an object outside this folder.")?;
            let name = relative.trim_end_matches('/');
            if name.is_empty() || name.contains('/') {
                continue;
            }
            entries.push(FileEntry {
                name: name.into(),
                path: format!("/{prefix}{name}{}", if object.is_dir { "/" } else { "" }),
                is_dir: object.is_dir,
                is_symlink: false,
                size: object.size,
                modified: object.modified,
                readonly: false,
            });
        }
        entries.sort_by(|left, right| {
            right
                .is_dir
                .cmp(&left.is_dir)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        let normalized = if prefix.is_empty() {
            "/".into()
        } else {
            format!("/{}", prefix.trim_end_matches('/'))
        };
        let parent = if normalized == "/" {
            None
        } else {
            Some(
                normalized
                    .rsplit_once('/')
                    .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
                    .unwrap_or("/")
                    .into(),
            )
        };
        Ok(FileListing {
            path: normalized,
            parent,
            entries,
        })
    }

    pub fn stat(&self, path: &str) -> Result<S3Entry, String> {
        let key = Self::key(path)?;
        if key.is_empty() {
            return Ok(S3Entry {
                path: "/".into(),
                name: "/".into(),
                is_dir: true,
                size: 0,
                modified: None,
            });
        }
        let normalized = key.trim_end_matches('/');
        let (parent, name) = normalized.rsplit_once('/').unwrap_or(("", normalized));
        let listing = self.list(&format!("/{parent}"))?;
        let matches = listing
            .entries
            .into_iter()
            .filter(|item| item.name == name)
            .collect::<Vec<_>>();
        let item = matches
            .iter()
            .find(|item| item.path == path)
            .or_else(|| matches.first())
            .ok_or_else(|| format!("S3 item /{normalized} does not exist."))?;
        Ok(S3Entry {
            path: item.path.clone(),
            name: item.name.clone(),
            is_dir: item.is_dir,
            size: item.size,
            modified: item.modified,
        })
    }

    pub fn mkdir(&self, path: &str) -> Result<(), String> {
        let key = Self::key(path)?;
        if key.is_empty() {
            return Err("Choose a folder below the S3 bucket root.".into());
        }
        let key = format!("{}/", key.trim_end_matches('/'));
        let request = self
            .request(Method::PUT, &key, &[], &hex(&Sha256::digest(b"")), &[])?
            .body(Vec::new());
        self.send(request, "create S3 folder")?;
        Ok(())
    }

    fn delete_key(&self, key: &str) -> Result<(), String> {
        let request = self.request(Method::DELETE, key, &[], &hex(&Sha256::digest(b"")), &[])?;
        self.send(request, "delete S3 object")?;
        Ok(())
    }

    pub fn delete(&self, path: &str, recursive: bool) -> Result<(), String> {
        let item = self.stat(path)?;
        if item.path == "/" {
            return Err("The S3 bucket root cannot be deleted.".into());
        }
        if !item.is_dir {
            return self.delete_key(Self::key(&item.path)?);
        }
        let prefix = format!("{}/", Self::key(&item.path)?.trim_end_matches('/'));
        let objects = self.objects(&prefix, None)?;
        if !recursive && objects.iter().any(|object| object.key != prefix) {
            return Err(
                "This S3 folder is not empty. Choose recursive deletion to remove its objects."
                    .into(),
            );
        }
        for object in objects {
            self.delete_key(&object.key)?;
        }
        Ok(())
    }

    fn copy_key(&self, source: &str, destination: &str, size: u64) -> Result<(), String> {
        self.copy_key_in_parts(source, destination, size, MAX_SINGLE_UPLOAD, COPY_PART_SIZE)
    }

    /// Objects above 5 GB are copied on the server in parts (UploadPartCopy).
    fn copy_key_in_parts(
        &self,
        source: &str,
        destination: &str,
        size: u64,
        threshold: u64,
        part_size: u64,
    ) -> Result<(), String> {
        if size <= threshold {
            return self.copy_whole(source, destination);
        }
        let part_size = part_size.max(size.div_ceil(10_000));
        let header = format!("/{}/{}", self.bucket, encode(source, true));
        let upload_id = self.start_multipart(destination)?;
        let empty_hash = hex(&Sha256::digest(b""));
        let result = (|| {
            let mut completed = Vec::new();
            for part_number in 1..=size.div_ceil(part_size) {
                let first = (part_number - 1) * part_size;
                let last = (first + part_size).min(size) - 1;
                let query = [
                    ("partNumber".into(), part_number.to_string()),
                    ("uploadId".into(), upload_id.clone()),
                ];
                let extra = [
                    ("x-amz-copy-source", header.clone()),
                    ("x-amz-copy-source-range", format!("bytes={first}-{last}")),
                ];
                let bytes = self
                    .send(
                        self.request(Method::PUT, destination, &query, &empty_hash, &extra)?
                            .timeout(Duration::from_secs(3600)),
                        "copy part of the S3 object",
                    )?
                    .bytes()
                    .map_err(|error| format!("Could not read the S3 copy response: {error}"))?;
                completed.push((part_number, xml_field(&bytes, "CopyPartResult", "ETag")?));
            }
            self.complete_multipart(destination, &upload_id, &completed)
        })();
        if let Err(error) = result {
            self.abort_multipart(destination, &upload_id);
            return Err(format!("{error} The source was kept."));
        }
        Ok(())
    }

    fn copy_whole(&self, source: &str, destination: &str) -> Result<(), String> {
        let header = format!("/{}/{}", self.bucket, encode(source, true));
        let request = self.request(
            Method::PUT,
            destination,
            &[],
            &hex(&Sha256::digest(b"")),
            &[("x-amz-copy-source", header)],
        )?;
        let response = self.send(request, "copy S3 object")?;
        let bytes = response
            .bytes()
            .map_err(|error| format!("Could not finish S3 copy: {error}"))?;
        if bytes.len() > 1024 * 1024 {
            return Err("S3 copy response is unexpectedly large. The source was kept.".into());
        }
        let mut reader = Reader::from_reader(bytes.as_ref());
        let root = loop {
            match reader.read_event() {
                Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                    break String::from_utf8_lossy(event.local_name().as_ref()).to_string()
                }
                Ok(Event::Eof) => {
                    return Err("S3 copy returned an empty response. The source was kept.".into())
                }
                Err(error) => return Err(format!("Could not read S3 copy response: {error}")),
                _ => {}
            }
        };
        if root != "CopyObjectResult" {
            return Err(format!(
                "S3 copy returned {root} instead of a completed copy. The source was kept."
            ));
        }
        Ok(())
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        let source = self.stat(from)?;
        let from_key = Self::key(&source.path)?;
        let to_key = Self::key(to)?;
        if to_key.is_empty() {
            return Err("Choose a destination below the S3 bucket root.".into());
        }
        if source.is_dir {
            let old_prefix = format!("{}/", from_key.trim_end_matches('/'));
            let new_prefix = format!("{}/", to_key.trim_end_matches('/'));
            if new_prefix.starts_with(&old_prefix) {
                return Err("Cannot move an S3 folder into itself.".into());
            }
            let objects = self.objects(&old_prefix, None)?;
            for object in &objects {
                let suffix = object
                    .key
                    .strip_prefix(&old_prefix)
                    .ok_or("S3 listing changed during move.")?;
                self.copy_key(&object.key, &format!("{new_prefix}{suffix}"), object.size)?;
            }
            for object in objects {
                self.delete_key(&object.key)?;
            }
        } else {
            self.copy_key(from_key, to_key, source.size)?;
            self.delete_key(from_key)?;
        }
        Ok(())
    }

    pub fn open_read(&self, path: &str, offset: u64) -> Result<Response, String> {
        let key = Self::key(path)?;
        let mut request = self
            .request(Method::GET, key, &[], &hex(&Sha256::digest(b"")), &[])?
            .timeout(Duration::from_secs(3600));
        if offset > 0 {
            request = request.header(RANGE, format!("bytes={offset}-"));
        }
        let response = self.send(request, "download S3 object")?;
        if offset > 0 && response.status() != StatusCode::PARTIAL_CONTENT {
            return Err("The S3 server did not accept the download range. Remove the partial copy and retry.".into());
        }
        Ok(response)
    }

    pub fn open_write(&self, path: &str, truncate: bool, offset: u64) -> Result<S3Upload, String> {
        let key = Self::key(path)?;
        if key.is_empty() {
            return Err("Choose an S3 object path below the bucket root.".into());
        }
        let mut file = NamedTempFile::new()
            .map_err(|error| format!("Could not prepare S3 upload: {error}"))?;
        if !truncate {
            let mut response = self.open_read(path, 0)?;
            std::io::copy(&mut response, &mut file)
                .map_err(|error| format!("Could not resume S3 upload: {error}"))?;
            if file
                .as_file()
                .metadata()
                .map_err(|error| format!("Could not inspect S3 upload: {error}"))?
                .len()
                != offset
            {
                return Err("The remote partial S3 object changed. Remove it and retry.".into());
            }
        }
        Ok(S3Upload {
            file,
            path: path.into(),
            position: offset,
        })
    }

    pub fn write_chunk(upload: &mut S3Upload, offset: u64, data: &[u8]) -> Result<(), String> {
        if upload.position != offset {
            return Err("S3 upload offset changed unexpectedly. Retry the transfer.".into());
        }
        upload
            .file
            .write_all(data)
            .map_err(|error| format!("Could not spool S3 upload: {error}"))?;
        upload.position += data.len() as u64;
        Ok(())
    }

    pub fn finish_write(&self, upload: S3Upload) -> Result<(), String> {
        self.finish_write_with_limits(upload, MULTIPART_THRESHOLD, MULTIPART_PART_SIZE)
    }

    fn finish_write_with_limits(
        &self,
        mut upload: S3Upload,
        multipart_threshold: u64,
        part_size: u64,
    ) -> Result<(), String> {
        let size = upload
            .file
            .as_file()
            .metadata()
            .map_err(|error| format!("Could not inspect S3 upload: {error}"))?
            .len();
        if size > multipart_threshold || size > MAX_SINGLE_UPLOAD {
            return self.multipart_write(&upload, size, part_size);
        }
        upload
            .file
            .as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(|error| format!("Could not read S3 upload: {error}"))?;
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = upload
                .file
                .read(&mut buffer)
                .map_err(|error| format!("Could not hash S3 upload: {error}"))?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        let key = Self::key(&upload.path)?;
        let request = self.request(Method::PUT, key, &[], &hex(&digest.finalize()), &[])?;
        let file = File::open(upload.file.path())
            .map_err(|error| format!("Could not reopen S3 upload: {error}"))?;
        self.send(
            request
                .header(CONTENT_LENGTH, size)
                .body(Body::new(file))
                .timeout(Duration::from_secs(3600)),
            "upload S3 object",
        )?;
        Ok(())
    }

    fn start_multipart(&self, key: &str) -> Result<String, String> {
        let started = self.send(
            self.request(
                Method::POST,
                key,
                &[("uploads".into(), "".into())],
                &hex(&Sha256::digest(b"")),
                &[],
            )?,
            "start S3 multipart upload",
        )?;
        xml_field(
            &started
                .bytes()
                .map_err(|error| format!("Could not read S3 upload ID: {error}"))?,
            "InitiateMultipartUploadResult",
            "UploadId",
        )
    }

    fn complete_multipart(
        &self,
        key: &str,
        upload_id: &str,
        parts: &[(u64, String)],
    ) -> Result<(), String> {
        let body = format!(
            "<CompleteMultipartUpload>{}</CompleteMultipartUpload>",
            parts
                .iter()
                .map(|(number, etag)| format!(
                    "<Part><PartNumber>{number}</PartNumber><ETag>{}</ETag></Part>",
                    xml_escape(etag)
                ))
                .collect::<String>()
        );
        let response = self.send(
            self.request(
                Method::POST,
                key,
                &[("uploadId".into(), upload_id.to_string())],
                &hex(&Sha256::digest(body.as_bytes())),
                &[],
            )?
            .body(body)
            .timeout(Duration::from_secs(3600)),
            "complete S3 multipart upload",
        )?;
        xml_root(
            &response
                .bytes()
                .map_err(|error| format!("Could not read S3 completion response: {error}"))?,
            "CompleteMultipartUploadResult",
        )
    }

    fn abort_multipart(&self, key: &str, upload_id: &str) -> Option<String> {
        self.request(
            Method::DELETE,
            key,
            &[("uploadId".into(), upload_id.to_string())],
            &hex(&Sha256::digest(b"")),
            &[],
        )
        .and_then(|request| self.send(request, "abort S3 multipart upload"))
        .err()
    }

    /// The newest unfinished upload for this exact key, left by a dropped connection.
    fn unfinished_upload(&self, key: &str) -> Result<Option<String>, String> {
        let query = [
            ("uploads".into(), String::new()),
            ("prefix".into(), key.to_string()),
        ];
        let bytes = self
            .send(
                self.request(Method::GET, "", &query, &hex(&Sha256::digest(b"")), &[])?,
                "list unfinished S3 uploads",
            )?
            .bytes()
            .map_err(|error| format!("Could not read unfinished S3 uploads: {error}"))?;
        Ok(xml_records(&bytes, "Upload")
            .into_iter()
            .filter(|upload| upload.get("Key").is_some_and(|found| found == key))
            .filter_map(|mut upload| upload.remove("UploadId"))
            .next_back())
    }

    /// Parts already on the server: number -> (ETag, size).
    fn uploaded_parts(
        &self,
        key: &str,
        upload_id: &str,
    ) -> Result<HashMap<u64, (String, u64)>, String> {
        let mut parts = HashMap::new();
        let mut marker = String::from("0");
        loop {
            let query = [
                ("uploadId".into(), upload_id.to_string()),
                ("part-number-marker".into(), marker.clone()),
            ];
            let bytes = self
                .send(
                    self.request(Method::GET, key, &query, &hex(&Sha256::digest(b"")), &[])?,
                    "list uploaded S3 parts",
                )?
                .bytes()
                .map_err(|error| format!("Could not read uploaded S3 parts: {error}"))?;
            for part in xml_records(&bytes, "Part") {
                let number = part.get("PartNumber").and_then(|value| value.parse().ok());
                let size = part.get("Size").and_then(|value| value.parse().ok());
                if let (Some(number), Some(size), Some(etag)) = (number, size, part.get("ETag")) {
                    parts.insert(number, (etag.clone(), size));
                }
            }
            let truncated = xml_field(&bytes, "ListPartsResult", "IsTruncated")
                .is_ok_and(|value| value == "true");
            match xml_field(&bytes, "ListPartsResult", "NextPartNumberMarker") {
                Ok(next) if truncated && next != marker => marker = next,
                _ => return Ok(parts),
            }
        }
    }

    /// Uploads in parts. After a dropped connection the finished parts stay
    /// on the server, and the next attempt sends only what is missing.
    fn multipart_write(&self, upload: &S3Upload, size: u64, part_size: u64) -> Result<(), String> {
        let minimum_part_size = size.div_ceil(10_000).div_ceil(1024 * 1024) * 1024 * 1024;
        let part_size = part_size.max(minimum_part_size);
        if !(5 * 1024 * 1024..=MAX_SINGLE_UPLOAD).contains(&part_size) {
            return Err("S3 multipart parts must be between 5 MB and 5 GB.".into());
        }
        let key = Self::key(&upload.path)?;
        let (upload_id, mut uploaded) = match self.unfinished_upload(key)? {
            Some(id) => {
                let parts = self.uploaded_parts(key, &id)?;
                (id, parts)
            }
            None => (self.start_multipart(key)?, HashMap::new()),
        };
        let result = (|| {
            let mut completed = Vec::new();
            for part_number in 1..=size.div_ceil(part_size) {
                let offset = (part_number - 1) * part_size;
                let length = part_size.min(size - offset);
                let (body_hash, md5) = hash_file_range(upload.file.path(), offset, length)?;
                if let Some((etag, _)) = uploaded
                    .remove(&part_number)
                    .filter(|(etag, listed)| *listed == length && etag.trim_matches('"') == md5)
                {
                    completed.push((part_number, etag));
                    continue;
                }
                let mut file = File::open(upload.file.path())
                    .map_err(|error| format!("Could not reopen S3 upload: {error}"))?;
                file.seek(SeekFrom::Start(offset))
                    .map_err(|error| format!("Could not read S3 upload part: {error}"))?;
                let query = [
                    ("partNumber".into(), part_number.to_string()),
                    ("uploadId".into(), upload_id.clone()),
                ];
                let response = self.send(
                    self.request(Method::PUT, key, &query, &body_hash, &[])?
                        .header(CONTENT_LENGTH, length)
                        .body(Body::new(file.take(length)))
                        .timeout(Duration::from_secs(3600)),
                    "send S3 upload part",
                )?;
                let etag = response
                    .headers()
                    .get("ETag")
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| !value.is_empty())
                    .ok_or("S3 server did not return an ETag for an upload part.")?;
                completed.push((part_number, etag.to_string()));
            }
            self.complete_multipart(key, &upload_id, &completed)
        })();
        match result {
            Ok(()) => Ok(()),
            Err(error) if crate::transfer::retryable(&error) => Err(format!(
                "{error} The finished parts were kept, so retrying continues the upload."
            )),
            Err(error) => match self.abort_multipart(key, &upload_id) {
                None => Err(error),
                Some(abort_error) => Err(format!(
                    "{error} Also could not abort the incomplete upload: {abort_error}"
                )),
            },
        }
    }

    /// Content type, caching, storage class, custom metadata and tags of one object.
    pub fn properties(&self, path: &str) -> Result<S3Properties, String> {
        let key = Self::key(path)?;
        if key.is_empty() || key.ends_with('/') {
            return Err("Choose a file to see its S3 properties.".into());
        }
        let empty = hex(&Sha256::digest(b""));
        let response = self.send(
            self.request(Method::HEAD, key, &[], &empty, &[])?,
            "read the S3 object's properties",
        )?;
        let headers = response.headers();
        let text = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string()
        };
        let mut metadata: Vec<(String, String)> = headers
            .iter()
            .filter_map(|(name, value)| {
                let name = name.as_str().strip_prefix("x-amz-meta-")?;
                Some((name.to_string(), value.to_str().ok()?.to_string()))
            })
            .collect();
        metadata.sort();
        let mut properties = S3Properties {
            content_type: text("content-type"),
            cache_control: text("cache-control"),
            content_disposition: text("content-disposition"),
            content_encoding: text("content-encoding"),
            storage_class: Some(text("x-amz-storage-class"))
                .filter(|class| !class.is_empty())
                .unwrap_or_else(|| "STANDARD".into()),
            encryption: text("x-amz-server-side-encryption"),
            etag: text("etag").trim_matches('"').to_string(),
            last_modified: text("last-modified"),
            size: headers
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok()?.parse().ok())
                .unwrap_or(0),
            metadata,
            tags: None,
        };
        // Some S3-compatible services have no tagging; the rest still shows.
        if let Ok(response) = self.send(
            self.request(
                Method::GET,
                key,
                &[("tagging".into(), String::new())],
                &empty,
                &[],
            )?,
            "read the S3 object's tags",
        ) {
            if let Ok(bytes) = response.bytes() {
                properties.tags = Some(
                    xml_records(&bytes, "Tag")
                        .into_iter()
                        .filter_map(|tag| {
                            Some((
                                tag.get("Key")?.clone(),
                                tag.get("Value").cloned().unwrap_or_default(),
                            ))
                        })
                        .collect(),
                );
            }
        }
        Ok(properties)
    }

    /// Rewrites the object's headers and metadata by copying it onto itself.
    pub fn set_properties(&self, path: &str, changes: &S3PropertyChanges) -> Result<(), String> {
        let key = Self::key(path)?;
        if key.is_empty() || key.ends_with('/') {
            return Err("Choose a file to change its S3 properties.".into());
        }
        let current = self.properties(path)?;
        if current.size > MAX_SINGLE_UPLOAD {
            return Err("S3 can only change the properties of objects up to 5 GB in place.".into());
        }
        let mut extra: Vec<(String, String)> = vec![
            (
                "x-amz-copy-source".into(),
                format!("/{}/{}", self.bucket, encode(key, true)),
            ),
            ("x-amz-metadata-directive".into(), "REPLACE".into()),
        ];
        for (name, value) in [
            ("content-type", &changes.content_type),
            ("cache-control", &changes.cache_control),
            ("content-disposition", &changes.content_disposition),
            ("content-encoding", &changes.content_encoding),
        ] {
            let value = value.trim();
            if value.contains(['\r', '\n']) || !value.is_ascii() {
                return Err(format!("The {name} value must be plain text on one line."));
            }
            if !value.is_empty() {
                extra.push((name.into(), value.into()));
            }
        }
        let class = changes.storage_class.trim();
        if !class.is_empty() {
            if !class.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                return Err("Choose a storage class such as STANDARD or GLACIER_IR.".into());
            }
            extra.push(("x-amz-storage-class".into(), class.into()));
        }
        // Keep the object encrypted the way it was.
        if current.encryption == "AES256" || current.encryption == "aws:kms" {
            extra.push((
                "x-amz-server-side-encryption".into(),
                current.encryption.clone(),
            ));
        }
        let mut seen = HashSet::new();
        for (name, value) in &changes.metadata {
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            {
                return Err(format!("“{name}” is not a valid metadata name. Use letters, numbers, dashes, dots or underscores."));
            }
            if !value.is_ascii() || value.contains(['\r', '\n']) {
                return Err(format!(
                    "The value of “{name}” must be plain ASCII text on one line."
                ));
            }
            if !seen.insert(name.clone()) {
                return Err(format!("“{name}” is listed twice."));
            }
            extra.push((format!("x-amz-meta-{name}"), value.trim().to_string()));
        }
        let extra: Vec<(&str, String)> = extra
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        let request = self.request(Method::PUT, key, &[], &hex(&Sha256::digest(b"")), &extra)?;
        self.send(request, "change the S3 object's properties")?;
        Ok(())
    }

    pub fn set_tags(&self, path: &str, tags: &[(String, String)]) -> Result<(), String> {
        let key = Self::key(path)?;
        if key.is_empty() || key.ends_with('/') {
            return Err("Choose a file to tag.".into());
        }
        if tags.len() > 10 {
            return Err("S3 allows up to 10 tags on an object.".into());
        }
        let mut seen = HashSet::new();
        let mut body = String::from("<Tagging><TagSet>");
        for (name, value) in tags {
            let name = name.trim();
            if name.is_empty() || name.chars().count() > 128 || value.chars().count() > 256 {
                return Err("Tag names need 1–128 characters and values up to 256.".into());
            }
            if !seen.insert(name) {
                return Err(format!("The tag “{name}” is listed twice."));
            }
            body.push_str(&format!(
                "<Tag><Key>{}</Key><Value>{}</Value></Tag>",
                quick_xml::escape::escape(name),
                quick_xml::escape::escape(value.trim())
            ));
        }
        body.push_str("</TagSet></Tagging>");
        let md5 = base64::engine::general_purpose::STANDARD.encode(Md5::digest(body.as_bytes()));
        let request = self
            .request(
                Method::PUT,
                key,
                &[("tagging".into(), String::new())],
                &hex(&Sha256::digest(body.as_bytes())),
                &[("content-md5", md5)],
            )?
            .body(body);
        self.send(request, "save the S3 object's tags")?;
        Ok(())
    }

    pub fn write_small(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let mut upload = self.open_write(path, true, 0)?;
        Self::write_chunk(&mut upload, 0, bytes)?;
        self.finish_write(upload)
    }
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct S3Properties {
    content_type: String,
    cache_control: String,
    content_disposition: String,
    content_encoding: String,
    storage_class: String,
    encryption: String,
    etag: String,
    last_modified: String,
    size: u64,
    metadata: Vec<(String, String)>,
    /// None when the service does not support tags.
    tags: Option<Vec<(String, String)>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct S3PropertyChanges {
    content_type: String,
    cache_control: String,
    content_disposition: String,
    content_encoding: String,
    storage_class: String,
    metadata: Vec<(String, String)>,
}

fn connected_client(app: &AppHandle, profile_id: &str) -> Result<S3Client, String> {
    let state = app.state::<AppState>();
    let profile = profiles::get_profile(&state, profile_id)?;
    if profile.protocol != "s3" {
        return Err("Choose an S3 connection.".into());
    }
    let secret = vault::password_for(&state, &profile)?;
    let token = vault::s3_token_for(&state, profile_id)?;
    S3Client::connect_with_token(
        &profile,
        &secret,
        token.as_ref().map(|value| value.as_str()),
    )
}

#[tauri::command]
pub async fn s3_object_properties(
    app: AppHandle,
    profile_id: String,
    path: String,
) -> Result<S3Properties, String> {
    tauri::async_runtime::spawn_blocking(move || {
        connected_client(&app, &profile_id)?.properties(&path)
    })
    .await
    .map_err(|error| format!("Reading S3 properties stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn s3_object_properties_set(
    app: AppHandle,
    profile_id: String,
    path: String,
    changes: S3PropertyChanges,
    tags: Option<Vec<(String, String)>>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let client = connected_client(&app, &profile_id)?;
        client.set_properties(&path, &changes)?;
        if let Some(tags) = tags {
            client.set_tags(&path, &tags)?;
        }
        Ok(())
    })
    .await
    .map_err(|error| format!("Saving S3 properties stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn s3_share_url(
    app: AppHandle,
    profile_id: String,
    path: String,
    expires_seconds: u32,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let profile = profiles::get_profile(&state, &profile_id)?;
        if profile.protocol != "s3" {
            return Err("Choose an S3 connection to create a share link.".into());
        }
        let secret = vault::password_for(&state, &profile)?;
        let token = vault::s3_token_for(&state, &profile_id)?;
        S3Client::connect_with_token(
            &profile,
            &secret,
            token.as_ref().map(|value| value.as_str()),
        )?
        .presign_get(&path, expires_seconds)
    })
    .await
    .map_err(|error| format!("Could not create S3 share link: {error}"))?
}

#[tauri::command]
pub async fn webdav_share_url(
    app: AppHandle,
    profile_id: String,
    path: String,
    expire_days: Option<u32>,
    link_password: Option<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let profile = profiles::get_profile(&state, &profile_id)?;
        if profile.protocol != "webdav" {
            return Err("Choose a WebDAV connection to create a share link.".into());
        }
        let expires = expire_days
            .filter(|days| (1..=365).contains(days))
            .map(|days| {
                (chrono::Utc::now() + chrono::Duration::days(days.into()))
                    .format("%Y-%m-%d")
                    .to_string()
            });
        let password = vault::password_for(&state, &profile)?;
        crate::webdav::WebDavClient::connect(&profile, &password)?.share_link(
            &path,
            expires.as_deref(),
            link_password.as_deref(),
        )
    })
    .await
    .map_err(|error| format!("Could not create the share link: {error}"))?
}

#[tauri::command]
pub fn s3_share_copy_link(url: String) -> Result<(), String> {
    use std::{io::Write, process::Stdio};

    let parsed =
        url::Url::parse(&url).map_err(|_| "The share link is not a valid URL.".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || url.len() > 16_384 {
        return Err("The share link must be a valid HTTP or HTTPS URL under 16 KB.".into());
    }
    let mut child = crate::platform::clipboard_command()?
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not open the clipboard: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "The clipboard did not accept input.".to_string())?
        .write_all(url.as_bytes())
        .map_err(|error| format!("Could not write the share link to the clipboard: {error}"))?;
    let status = child
        .wait()
        .map_err(|error| format!("Could not finish copying the share link: {error}"))?;
    if !status.success() {
        return Err("The clipboard could not copy the share link.".into());
    }
    Ok(())
}

struct S3Object {
    key: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>,
}
struct S3Page {
    objects: Vec<S3Object>,
    next_token: Option<String>,
}

fn parse_list(xml: &[u8]) -> Result<S3Page, String> {
    let mut reader = Reader::from_reader(xml);
    let mut stack = Vec::<String>::new();
    let mut objects = Vec::new();
    let mut key = String::new();
    let mut size = 0;
    let mut modified = None;
    let mut next_token = None;
    let mut is_truncated = false;
    let mut root_seen = false;
    let mut url_encoded = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = String::from_utf8_lossy(event.local_name().as_ref()).to_string();
                if stack.is_empty() {
                    if name != "ListBucketResult" {
                        return Err("S3 server returned an unexpected listing document.".into());
                    }
                    root_seen = true;
                }
                if name == "Contents" || name == "CommonPrefixes" {
                    key.clear();
                    size = 0;
                    modified = None;
                }
                stack.push(name);
            }
            Ok(Event::Text(event)) => {
                let value = event
                    .decode()
                    .map_err(|error| format!("Invalid S3 listing text: {error}"))?;
                let value = quick_xml::escape::unescape(&value)
                    .map_err(|error| format!("Invalid S3 listing entity: {error}"))?;
                match stack.last().map(String::as_str) {
                    Some("Key") | Some("Prefix") => key.push_str(&value),
                    Some("Size") => {
                        size = value
                            .trim()
                            .parse()
                            .map_err(|_| "S3 server returned an invalid object size.")?
                    }
                    Some("LastModified") => {
                        modified = DateTime::parse_from_rfc3339(value.trim())
                            .ok()
                            .and_then(|date| u64::try_from(date.timestamp()).ok())
                    }
                    Some("NextContinuationToken") => {
                        next_token.get_or_insert_with(String::new).push_str(&value)
                    }
                    Some("IsTruncated") => is_truncated = value.trim() == "true",
                    Some("EncodingType") => url_encoded = value.trim() == "url",
                    _ => {}
                }
            }
            Ok(Event::GeneralRef(event)) => {
                let value = if let Some(character) = event
                    .resolve_char_ref()
                    .map_err(|error| format!("Invalid S3 listing character reference: {error}"))?
                {
                    character.to_string()
                } else {
                    let name = event
                        .decode()
                        .map_err(|error| format!("Invalid S3 listing entity: {error}"))?;
                    quick_xml::escape::resolve_xml_entity(&name)
                        .ok_or_else(|| format!("Unknown S3 listing entity: {name}"))?
                        .to_string()
                };
                match stack.last().map(String::as_str) {
                    Some("Key") | Some("Prefix") => key.push_str(&value),
                    Some("NextContinuationToken") => {
                        next_token.get_or_insert_with(String::new).push_str(&value)
                    }
                    _ => return Err("S3 listing contains an unexpected entity reference.".into()),
                }
            }
            Ok(Event::End(event)) => {
                let name = event.local_name();
                if name.as_ref() == b"Contents" && !key.is_empty() {
                    objects.push(S3Object {
                        key: key.clone(),
                        is_dir: key.ends_with('/'),
                        size,
                        modified,
                    });
                }
                if name.as_ref() == b"CommonPrefixes" && !key.is_empty() {
                    objects.push(S3Object {
                        key: key.clone(),
                        is_dir: true,
                        size: 0,
                        modified: None,
                    });
                }
                stack.pop();
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("Could not parse S3 listing: {error}")),
            _ => {}
        }
    }
    if !root_seen {
        return Err("S3 server returned an empty listing document.".into());
    }
    if is_truncated && next_token.as_deref().is_none_or(str::is_empty) {
        return Err("S3 listing was truncated without a continuation token.".into());
    }
    if url_encoded {
        for object in &mut objects {
            object.key = percent_decode_str(&object.key)
                .decode_utf8()
                .map_err(|_| "S3 listing contains an invalid encoded object name.")?
                .into_owned();
        }
    }
    Ok(S3Page {
        objects,
        next_token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(host: &str) -> Profile {
        Profile {
            id: "test".into(),
            name: "S3 test".into(),
            host: host.into(),
            protocol: "s3".into(),
            port: 443,
            username: "AKIDEXAMPLE".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: Some("us-east-1".into()),
        }
    }

    /// Answers each request on one keep-alive connection and returns the request heads.
    fn serve_s3(replies: Vec<&'static str>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut heads = Vec::new();
            let (mut stream, _) = listener.accept().unwrap();
            let mut pending = Vec::new();
            for reply in replies {
                let head = loop {
                    if let Some(end) = pending.windows(4).position(|window| window == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&pending[..end]).to_string();
                        pending.drain(..end + 4);
                        break head;
                    }
                    let mut buffer = [0_u8; 4096];
                    let count = stream.read(&mut buffer).unwrap();
                    pending.extend_from_slice(&buffer[..count]);
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{reply}",
                    reply.len()
                )
                .unwrap();
                heads.push(head);
            }
            heads
        });
        (format!("http://{address}"), handle)
    }

    #[test]
    fn signs_directory_bucket_requests_with_a_session() {
        let (base, handle) = serve_s3(vec![
            "<CreateSessionResult><Credentials><SessionToken>zonal-token</SessionToken><SecretAccessKey>zonal-secret</SecretAccessKey><AccessKeyId>ASIAZONAL</AccessKeyId><Expiration>2030-01-01T00:00:00Z</Expiration></Credentials></CreateSessionResult>",
            "<ListBucketResult><Contents><Key>photo.jpg</Key><Size>3</Size></Contents></ListBucketResult>",
        ]);
        let client = S3Client::connect(
            &profile(&format!("{base}/photos--use1-az4--x-s3")),
            "secret",
        )
        .unwrap();
        let listing = client.list("/").unwrap();
        assert_eq!(listing.entries[0].name, "photo.jpg");
        let heads = handle.join().unwrap();
        let session = heads[0].to_ascii_lowercase();
        assert!(session.starts_with("get /photos--use1-az4--x-s3/?session="));
        assert!(
            session.contains("credential=akidexample/")
                && session.contains("/s3express/aws4_request")
        );
        let listed = heads[1].to_ascii_lowercase();
        assert!(
            listed.contains("credential=asiazonal/") && listed.contains("/s3express/aws4_request")
        );
        assert!(listed.contains("x-amz-s3session-token: zonal-token"));
        assert!(listed
            .contains("signedheaders=host;x-amz-content-sha256;x-amz-date;x-amz-s3session-token,"));
        assert!(client
            .presign_get("/photo.jpg", 600)
            .unwrap_err()
            .contains("directory buckets"));
    }

    #[test]
    fn encodes_bucket_paths_and_sorted_query_without_double_encoding() {
        let client =
            S3Client::connect(&profile("http://127.0.0.1:12345/test-bucket"), "secret").unwrap();
        let url = client
            .url(
                "folder/a b+ü.txt",
                &[
                    ("prefix".into(), "a b".into()),
                    ("list-type".into(), "2".into()),
                ],
            )
            .unwrap();
        assert_eq!(url.path(), "/test-bucket/folder/a%20b%2B%C3%BC.txt");
        let hosted = S3Client::connect(
            &profile("https://my-bucket.s3.eu-central-1.amazonaws.com"),
            "secret",
        )
        .unwrap();
        assert_eq!(
            hosted.url("a b.txt", &[]).unwrap().as_str(),
            "https://my-bucket.s3.eu-central-1.amazonaws.com/a%20b.txt"
        );
        assert!(validate_profile(&profile("https://amazonaws.com")).is_err());
        assert_eq!(url.query(), Some("list-type=2&prefix=a%20b"));
    }

    #[test]
    fn requires_a_secure_bucket_url_and_explicit_signing_region() {
        let mut target = profile("https://s3.eu-central-1.amazonaws.com/my-bucket");
        assert!(validate_profile(&target).is_ok());
        target.s3_region = None;
        assert!(validate_profile(&target).unwrap_err().contains("region"));
        target.s3_region = Some("eu-central-1".into());
        target.host = "http://s3.eu-central-1.amazonaws.com/my-bucket".into();
        assert!(validate_profile(&target).unwrap_err().contains("HTTPS"));
        target.host = "https://s3.eu-central-1.amazonaws.com/a/b".into();
        assert!(validate_profile(&target).unwrap_err().contains("bucket"));
    }

    #[test]
    fn reads_s3_objects_and_prefixes() {
        let xml = br#"<ListBucketResult><Contents><Key>notes/a&amp;b.txt</Key><Size>4</Size><LastModified>2024-01-01T00:00:00.000Z</LastModified></Contents><CommonPrefixes><Prefix>notes/photos/</Prefix></CommonPrefixes><NextContinuationToken>next</NextContinuationToken></ListBucketResult>"#;
        let page = parse_list(xml).unwrap();
        assert_eq!(page.objects.len(), 2);
        assert_eq!(page.objects[0].key, "notes/a&b.txt");
        assert_eq!(page.objects[0].size, 4);
        assert!(page.objects[1].is_dir);
        assert_eq!(page.next_token.as_deref(), Some("next"));
    }

    #[test]
    #[ignore = "requires the local S3 test server"]
    fn s3_integration_virtual_hosted_bucket() {
        let port = std::env::var("WEAVE_S3_TEST_PORT").unwrap();
        let profile = profile(&format!("http://weave-test.localhost:{port}"));
        validate_profile(&profile).unwrap();
        let client = S3Client::connect(&profile, "weave-test-secret").unwrap();
        client
            .write_small("/virtual/hello.txt", b"virtual host")
            .unwrap();
        assert_eq!(
            client
                .open_read("/virtual/hello.txt", 0)
                .unwrap()
                .bytes()
                .unwrap()
                .as_ref(),
            b"virtual host"
        );
        assert!(client
            .list("/virtual")
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.name == "hello.txt"));
        client.delete("/virtual/hello.txt", false).unwrap();
    }

    #[test]
    #[ignore = "requires the local S3 test server"]
    fn s3_integration_resumes_multipart_uploads_and_copies_in_parts() {
        let port = std::env::var("WEAVE_S3_TEST_PORT").unwrap();
        let profile = profile(&format!("http://127.0.0.1:{port}/weave-test"));
        let client = S3Client::connect(&profile, "weave-test-secret").unwrap();
        let payload: Vec<u8> = (0..11 * 1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect();
        let spool = |client: &S3Client| {
            let mut upload = client.open_write("/flaky-part.bin", true, 0).unwrap();
            S3Client::write_chunk(&mut upload, 0, &payload).unwrap();
            upload
        };
        let error = client
            .finish_write_with_limits(spool(&client), 5 * 1024 * 1024, 5 * 1024 * 1024)
            .unwrap_err();
        assert!(error.contains("retrying continues"), "{error}");
        let upload_id = client.unfinished_upload("flaky-part.bin").unwrap().unwrap();
        assert_eq!(
            client
                .uploaded_parts("flaky-part.bin", &upload_id)
                .unwrap()
                .len(),
            1
        );
        // The server refuses a second copy of part 1, so this proves it is skipped.
        client
            .finish_write_with_limits(spool(&client), 5 * 1024 * 1024, 5 * 1024 * 1024)
            .unwrap();
        assert_eq!(
            client
                .open_read("/flaky-part.bin", 0)
                .unwrap()
                .bytes()
                .unwrap()
                .as_ref(),
            payload
        );
        assert!(client
            .unfinished_upload("flaky-part.bin")
            .unwrap()
            .is_none());

        client
            .copy_key_in_parts(
                "flaky-part.bin",
                "copied/flaky-part.bin",
                payload.len() as u64,
                6 * 1024 * 1024,
                5 * 1024 * 1024,
            )
            .unwrap();
        assert_eq!(
            client
                .open_read("/copied/flaky-part.bin", 0)
                .unwrap()
                .bytes()
                .unwrap()
                .as_ref(),
            payload
        );
    }

    #[test]
    #[ignore = "requires the local S3 test server"]
    fn s3_integration_multipart_uploads_and_aborts_failed_parts() {
        let port = std::env::var("WEAVE_S3_TEST_PORT").unwrap();
        let profile = profile(&format!("http://127.0.0.1:{port}/weave-test"));
        let client = S3Client::connect(&profile, "weave-test-secret").unwrap();
        let payload = vec![b'z'; 10 * 1024 * 1024 + 17];
        let mut upload = client.open_write("/large.bin", true, 0).unwrap();
        S3Client::write_chunk(&mut upload, 0, &payload).unwrap();
        client
            .finish_write_with_limits(upload, 5 * 1024 * 1024, 5 * 1024 * 1024)
            .unwrap();
        assert_eq!(
            client
                .open_read("/large.bin", 0)
                .unwrap()
                .bytes()
                .unwrap()
                .as_ref(),
            payload
        );

        client
            .write_small("/fail-part.bin", b"old contents")
            .unwrap();
        let mut failed = client.open_write("/fail-part.bin", true, 0).unwrap();
        S3Client::write_chunk(&mut failed, 0, &payload).unwrap();
        assert!(client
            .finish_write_with_limits(failed, 5 * 1024 * 1024, 5 * 1024 * 1024)
            .unwrap_err()
            .contains("HTTP 500"));
        assert_eq!(
            client
                .open_read("/fail-part.bin", 0)
                .unwrap()
                .bytes()
                .unwrap()
                .as_ref(),
            b"old contents"
        );
        let mut incomplete = client.open_write("/fail-complete.bin", true, 0).unwrap();
        S3Client::write_chunk(&mut incomplete, 0, &payload).unwrap();
        assert!(client
            .finish_write_with_limits(incomplete, 5 * 1024 * 1024, 5 * 1024 * 1024)
            .unwrap_err()
            .contains("InternalError"));
        assert!(client.stat("/fail-complete.bin").is_err());
        let outstanding = client
            .send(
                client
                    .request(
                        Method::GET,
                        "",
                        &[("uploads".into(), "".into())],
                        &hex(&Sha256::digest(b"")),
                        &[],
                    )
                    .unwrap(),
                "list test uploads",
            )
            .unwrap();
        assert_eq!(
            xml_field(
                &outstanding.bytes().unwrap(),
                "ListMultipartUploadsResult",
                "Count"
            )
            .unwrap(),
            "0"
        );
    }

    #[test]
    #[ignore = "requires the local S3 test server"]
    fn s3_integration_session_token_signs_requests_and_share_links() {
        let port = std::env::var("WEAVE_S3_TEST_PORT").unwrap();
        let mut profile = profile(&format!("http://127.0.0.1:{port}/weave-test"));
        profile.username = "AKIDTEMP".into();
        assert!(S3Client::connect(&profile, "weave-test-secret")
            .unwrap()
            .list("/")
            .err()
            .unwrap()
            .contains("HTTP 403"));
        let client = S3Client::connect_with_token(
            &profile,
            "weave-test-secret",
            Some("weave+token/2026=temporary"),
        )
        .unwrap();
        client
            .write_small("/token-file.txt", b"private file")
            .unwrap();
        assert_eq!(
            client
                .open_read("/token-file.txt", 0)
                .unwrap()
                .bytes()
                .unwrap()
                .as_ref(),
            b"private file"
        );
        let share = client.presign_get("/token-file.txt", 900).unwrap();
        assert!(share.contains("X-Amz-Security-Token=weave%2Btoken%2F2026%3Dtemporary"));
        let anonymous = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        assert_eq!(
            anonymous.get(&share).send().unwrap().status(),
            StatusCode::OK
        );
        let tampered = share.replace("weave%2Btoken%2F2026%3Dtemporary", "wrong");
        assert_eq!(
            anonymous.get(&tampered).send().unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    #[ignore = "requires the local S3 test server"]
    fn s3_integration_reads_and_changes_properties_and_tags() {
        let port = std::env::var("WEAVE_S3_TEST_PORT").unwrap();
        let profile = profile(&format!("http://127.0.0.1:{port}/weave-test"));
        let client = S3Client::connect(&profile, "weave-test-secret").unwrap();
        client.write_small("/props/report.csv", b"a,b\n").unwrap();
        let before = client.properties("/props/report.csv").unwrap();
        assert_eq!(
            (before.size, before.storage_class.as_str()),
            (4, "STANDARD")
        );
        assert_eq!(before.tags, Some(Vec::new()));
        let changes = S3PropertyChanges {
            content_type: "text/csv".into(),
            cache_control: "max-age=60".into(),
            content_disposition: "attachment; filename=\"report.csv\"".into(),
            content_encoding: String::new(),
            storage_class: "STANDARD_IA".into(),
            metadata: vec![("Owner".into(), "finance team".into())],
        };
        client
            .set_properties("/props/report.csv", &changes)
            .unwrap();
        client
            .set_tags(
                "/props/report.csv",
                &[
                    ("project".into(), "Q3 & Q4".into()),
                    ("tier".into(), "<cold>".into()),
                ],
            )
            .unwrap();
        let after = client.properties("/props/report.csv").unwrap();
        assert_eq!(after.content_type, "text/csv");
        assert_eq!(after.cache_control, "max-age=60");
        assert_eq!(after.storage_class, "STANDARD_IA");
        assert_eq!(
            after.metadata,
            vec![("owner".to_string(), "finance team".to_string())]
        );
        assert_eq!(
            after.tags.unwrap(),
            vec![
                ("project".to_string(), "Q3 & Q4".to_string()),
                ("tier".to_string(), "<cold>".to_string())
            ]
        );
        assert_eq!(
            client
                .open_read("/props/report.csv", 0)
                .unwrap()
                .bytes()
                .unwrap()
                .as_ref(),
            b"a,b\n"
        );
        let duplicate = S3PropertyChanges {
            metadata: vec![("a".into(), "1".into()), ("A".into(), "2".into())],
            ..changes
        };
        assert!(client
            .set_properties("/props/report.csv", &duplicate)
            .unwrap_err()
            .contains("twice"));
        let too_many: Vec<_> = (0..11).map(|n| (format!("t{n}"), String::new())).collect();
        assert!(client.set_tags("/props/report.csv", &too_many).is_err());
        assert!(client.properties("/props/").is_err());
        client.delete("/props/", true).unwrap();
    }

    #[test]
    #[ignore = "requires the local S3 test server"]
    fn s3_integration_browses_copies_and_transfers() {
        let port = std::env::var("WEAVE_S3_TEST_PORT").unwrap();
        let profile = profile(&format!("http://127.0.0.1:{port}/weave-test"));
        let client = S3Client::connect(&profile, "weave-test-secret").unwrap();
        assert!(client
            .list("/")
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.name == "paged"));
        assert_eq!(client.list("/paged").unwrap().entries.len(), 1005);
        assert!(S3Client::connect(&profile, "incorrect")
            .unwrap()
            .list("/")
            .err()
            .unwrap()
            .contains("HTTP 403"));
        assert!(client.list("/missing-folder").is_err());
        client.write_small("/collision", b"file").unwrap();
        client.mkdir("/collision").unwrap();
        assert!(!client.stat("/collision").unwrap().is_dir);
        assert!(client.stat("/collision/").unwrap().is_dir);
        client.delete("/collision", false).unwrap();
        client.delete("/collision/", true).unwrap();
        client.mkdir("/new").unwrap();
        client
            .write_small("/new/a &b.txt", b"hello from S3")
            .unwrap();
        let listing = client.list("/new").unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, "a &b.txt");
        let share = client.presign_get("/new/a &b.txt", 3600).unwrap();
        let anonymous = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let shared = anonymous.get(&share).send().unwrap();
        assert_eq!(shared.status(), StatusCode::OK);
        assert_eq!(shared.bytes().unwrap().as_ref(), b"hello from S3");
        let tampered = share.replace("X-Amz-Expires=3600", "X-Amz-Expires=7200");
        assert_eq!(
            anonymous.get(&tampered).send().unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert!(client.presign_get("/new", 3600).is_err());
        assert!(client.presign_get("/new/a &b.txt", 604_801).is_err());
        let mut response = client.open_read("/new/a &b.txt", 6).unwrap();
        let mut rest = String::new();
        response.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "from S3");
        client.rename("/new/a &b.txt", "/new/renamed.txt").unwrap();
        assert_eq!(client.stat("/new/renamed.txt").unwrap().size, 13);
        client.mkdir("/new/sub").unwrap();
        client
            .write_small("/new/sub/nested.txt", b"nested")
            .unwrap();
        client.rename("/new", "/moved").unwrap();
        assert_eq!(client.stat("/moved/sub/nested.txt").unwrap().size, 6);
        client.delete("/moved", true).unwrap();
        assert!(client.stat("/moved").is_err());
    }
}
