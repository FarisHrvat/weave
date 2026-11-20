//! Azure Blob Storage containers, signed with the account key (Shared Key)
//! or used through a shared access signature. Folders are name prefixes, as
//! in Azure Storage Explorer; an empty folder is kept as a blob ending in "/".

use crate::{
    cloud::{listing, normalize, split},
    files::{FileEntry, FileListing},
    profiles::Profile,
    s3::xml_records,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reqwest::{
    blocking::{Body, Client, RequestBuilder, Response},
    Method, StatusCode, Url,
};
use sha2::Sha256;
use std::{
    io::{Read, Seek, SeekFrom, Write},
    time::{Duration, Instant, SystemTime},
};
use tempfile::NamedTempFile;

const VERSION: &str = "2021-12-02";
/// Larger uploads are sent in blocks.
const SINGLE_UPLOAD: u64 = 64 * 1024 * 1024;
const BLOCK_SIZE: u64 = 16 * 1024 * 1024;

enum Credential {
    SharedKey(Vec<u8>),
    /// The query string of a shared access signature, without "?".
    Sas(String),
}

pub struct AzureUpload {
    file: NamedTempFile,
    path: String,
    position: u64,
}

pub struct AzureClient {
    http: Client,
    /// The container URL, for example https://account.blob.core.windows.net/photos
    container: Url,
    account: String,
    credential: Credential,
}

fn rfc1123(time: SystemTime) -> String {
    DateTime::<Utc>::from(time)
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

fn modified(value: Option<&String>) -> Option<u64> {
    DateTime::parse_from_rfc2822(value?)
        .ok()
        .and_then(|time| u64::try_from(time.timestamp()).ok())
}

/// The container URL without query, and the account name its host implies.
pub(crate) fn container_url(host: &str) -> Result<(Url, String), String> {
    let mut container = Url::parse(host.trim()).map_err(|_| {
        "Enter the container URL, like https://account.blob.core.windows.net/container.".to_string()
    })?;
    let local = matches!(container.host_str(), Some("127.0.0.1" | "localhost"));
    if container.scheme() != "https" && !local {
        return Err("Azure container URLs must use https.".into());
    }
    container.set_query(None);
    container.set_fragment(None);
    let path = container.path().trim_end_matches('/').to_string();
    if path.is_empty() {
        return Err("Add the container name to the URL, after the account address.".into());
    }
    container.set_path(&path);
    let account = container
        .host_str()
        .filter(|_| !local)
        .and_then(|host| host.split('.').next())
        .unwrap_or_default()
        .to_string();
    Ok((container, account))
}

impl AzureClient {
    pub fn connect(profile: &Profile, secret: &str) -> Result<Self, String> {
        let (container, implied) = container_url(&profile.host)?;
        let account = if profile.username.trim().is_empty() {
            implied
        } else {
            profile.username.trim().to_string()
        };
        let secret = secret.trim();
        let credential = if secret.contains("sig=") {
            Credential::Sas(secret.trim_start_matches('?').to_string())
        } else {
            if account.is_empty() {
                return Err("Enter the storage account name as the user name.".into());
            }
            Credential::SharedKey(STANDARD.decode(secret).map_err(|_| {
                "Enter the storage account key, or a shared access signature (SAS) token, as the password."
                    .to_string()
            })?)
        };
        let http = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|error| format!("Could not prepare the Azure connection: {error}"))?;
        Ok(Self {
            http,
            container,
            account,
            credential,
        })
    }

    fn key(path: &str) -> String {
        normalize(path).trim_start_matches('/').to_string()
    }

    fn url(&self, key: &str, query: &[(&str, String)]) -> Url {
        let mut url = self.container.clone();
        if !key.is_empty() {
            let mut segments = url.path_segments_mut().expect("container URLs have a path");
            for part in key.split('/') {
                segments.push(part);
            }
        }
        {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in query {
                pairs.append_pair(name, value);
            }
        }
        if let Credential::Sas(sas) = &self.credential {
            let joined = match url.query() {
                Some(existing) if !existing.is_empty() => format!("{existing}&{sas}"),
                _ => sas.clone(),
            };
            url.set_query(Some(&joined));
        }
        if url.query() == Some("") {
            url.set_query(None);
        }
        url
    }

    /// Signs a request with the account key (Shared Key authorization).
    fn request(
        &self,
        method: Method,
        key: &str,
        query: &[(&str, String)],
        headers: &[(&str, String)],
        length: u64,
    ) -> Result<RequestBuilder, String> {
        let url = self.url(key, query);
        let mut all: Vec<(String, String)> = headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
            .collect();
        all.push(("x-ms-date".into(), rfc1123(SystemTime::now())));
        all.push(("x-ms-version".into(), VERSION.into()));
        let mut request = self.http.request(method.clone(), url.clone());
        if let Credential::SharedKey(secret) = &self.credential {
            let header = |name: &str| {
                all.iter()
                    .find(|(header, _)| header == name)
                    .map(|(_, value)| value.as_str())
                    .unwrap_or("")
            };
            let mut canonical_headers: Vec<_> = all
                .iter()
                .filter(|(name, _)| name.starts_with("x-ms-"))
                .map(|(name, value)| format!("{name}:{}\n", value.trim()))
                .collect();
            canonical_headers.sort();
            let mut resource = format!("/{}{}", self.account, url.path());
            let mut params: Vec<(String, String)> = url
                .query_pairs()
                .map(|(name, value)| (name.to_ascii_lowercase(), value.into_owned()))
                .collect();
            params.sort();
            for (name, value) in params {
                resource.push_str(&format!("\n{name}:{value}"));
            }
            let length = if length == 0 {
                String::new()
            } else {
                length.to_string()
            };
            let string_to_sign = format!(
                "{}\n{}\n{}\n{}\n{}\n{}\n\n\n\n\n\n{}\n{}{}",
                method.as_str(),
                header("content-encoding"),
                header("content-language"),
                length,
                header("content-md5"),
                header("content-type"),
                header("range"),
                canonical_headers.concat(),
                resource
            );
            let mut mac = Hmac::<Sha256>::new_from_slice(secret)
                .map_err(|_| "The storage account key is not valid.")?;
            mac.update(string_to_sign.as_bytes());
            let signature = STANDARD.encode(mac.finalize().into_bytes());
            request = request.header(
                "Authorization",
                format!("SharedKey {}:{signature}", self.account),
            );
        }
        for (name, value) in all {
            request = request.header(name, value);
        }
        Ok(request.header("Content-Length", length.to_string()))
    }

    fn send(&self, request: RequestBuilder, action: &str) -> Result<Response, String> {
        let response = request
            .send()
            .map_err(|error| format!("Could not {action}: {error}"))?;
        Self::checked(response, action)
    }

    fn checked(response: Response, action: &str) -> Result<Response, String> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let code = response
            .headers()
            .get("x-ms-error-code")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let hint = match (status, code.as_str()) {
            (StatusCode::FORBIDDEN, "AuthenticationFailed") => {
                "Check the account name and key, or that the SAS token has not expired."
            }
            (StatusCode::FORBIDDEN, _) => {
                "The key or SAS token does not allow this. Check its permissions."
            }
            (StatusCode::NOT_FOUND, "ContainerNotFound") => "The container does not exist.",
            (StatusCode::NOT_FOUND, _) => "It does not exist.",
            (StatusCode::CONFLICT, _) => "Something else is using it. Try again.",
            _ => "Check the container URL and the server's response.",
        };
        Err(format!("Could not {action}: HTTP {status}. {hint}"))
    }

    fn get(&self, key: &str, query: &[(&str, String)], action: &str) -> Result<Response, String> {
        self.send(self.request(Method::GET, key, query, &[], 0)?, action)
    }

    /// Blobs and folders directly inside `prefix`, or everything below it when `delimiter` is None.
    fn blobs(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
        limit: Option<usize>,
    ) -> Result<(Vec<FileEntry>, Vec<String>), String> {
        let mut files = Vec::new();
        let mut folders = Vec::new();
        let mut marker = String::new();
        loop {
            let mut query = vec![
                ("restype", "container".to_string()),
                ("comp", "list".to_string()),
                ("prefix", prefix.to_string()),
            ];
            if let Some(delimiter) = delimiter {
                query.push(("delimiter", delimiter.to_string()));
            }
            if let Some(limit) = limit {
                query.push(("maxresults", limit.to_string()));
            }
            if !marker.is_empty() {
                query.push(("marker", marker.clone()));
            }
            let bytes = self
                .get("", &query, "list the Azure container")?
                .bytes()
                .map_err(|error| format!("Could not read the Azure listing: {error}"))?;
            for blob in xml_records(&bytes, "Blob") {
                let Some(name) = blob.get("Name") else {
                    continue;
                };
                files.push(FileEntry {
                    name: name.rsplit('/').next().unwrap_or(name).to_string(),
                    path: format!("/{name}"),
                    is_dir: false,
                    is_symlink: false,
                    size: blob
                        .get("Content-Length")
                        .and_then(|size| size.parse().ok())
                        .unwrap_or(0),
                    modified: modified(blob.get("Last-Modified")),
                    readonly: false,
                });
            }
            folders.extend(
                xml_records(&bytes, "BlobPrefix")
                    .into_iter()
                    .filter_map(|record| record.get("Name").cloned()),
            );
            marker = xml_records(&bytes, "EnumerationResults")
                .first()
                .and_then(|record| record.get("NextMarker").cloned())
                .unwrap_or_default();
            if marker.is_empty() || limit.is_some() {
                return Ok((files, folders));
            }
        }
    }

    pub fn list(&self, path: &str) -> Result<FileListing, String> {
        let key = Self::key(path);
        let prefix = if key.is_empty() {
            String::new()
        } else {
            format!("{key}/")
        };
        let (files, folders) = self.blobs(&prefix, Some("/"), None)?;
        if files.is_empty() && folders.is_empty() && !key.is_empty() {
            return Err(format!("The folder {} does not exist.", normalize(path)));
        }
        let mut entries: Vec<FileEntry> = files
            .into_iter()
            // The placeholder that keeps an empty folder.
            .filter(|file| file.path.trim_start_matches('/') != prefix)
            .collect();
        entries.extend(folders.into_iter().map(|folder| {
            let trimmed = folder.trim_end_matches('/');
            FileEntry {
                name: trimmed.rsplit('/').next().unwrap_or(trimmed).to_string(),
                path: format!("/{trimmed}"),
                is_dir: true,
                is_symlink: false,
                size: 0,
                modified: None,
                readonly: false,
            }
        }));
        Ok(listing(path, entries))
    }

    pub fn stat(&self, path: &str) -> Result<FileEntry, String> {
        let key = Self::key(path);
        let (_, name) = split(path);
        if key.is_empty() {
            return Ok(FileEntry {
                name: "/".into(),
                path: "/".into(),
                is_dir: true,
                is_symlink: false,
                size: 0,
                modified: None,
                readonly: false,
            });
        }
        let head = self
            .request(Method::HEAD, &key, &[], &[], 0)?
            .send()
            .map_err(|error| format!("Could not reach Azure: {error}"))?;
        if head.status().is_success() {
            let header = |name: &str| {
                head.headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .map(String::from)
            };
            return Ok(FileEntry {
                name,
                path: normalize(path),
                is_dir: false,
                is_symlink: false,
                size: header("content-length")
                    .and_then(|size| size.parse().ok())
                    .unwrap_or(0),
                modified: modified(header("last-modified").as_ref()),
                readonly: false,
            });
        }
        if head.status() != StatusCode::NOT_FOUND {
            return Err(Self::checked(head, "inspect the blob")
                .err()
                .unwrap_or_else(|| "Could not inspect the blob.".into()));
        }
        let (files, folders) = self.blobs(&format!("{key}/"), Some("/"), Some(1))?;
        if files.is_empty() && folders.is_empty() {
            return Err(format!("{} does not exist.", normalize(path)));
        }
        Ok(FileEntry {
            name,
            path: normalize(path),
            is_dir: true,
            is_symlink: false,
            size: 0,
            modified: None,
            readonly: false,
        })
    }

    fn put_blob(&self, key: &str, body: Body, length: u64, action: &str) -> Result<(), String> {
        let request = self
            .request(
                Method::PUT,
                key,
                &[],
                &[("x-ms-blob-type", "BlockBlob".into())],
                length,
            )?
            .body(body);
        self.send(request, action).map(|_| ())
    }

    pub fn mkdir(&self, path: &str) -> Result<(), String> {
        let key = Self::key(path);
        if key.is_empty() {
            return Err("Choose a folder inside the container.".into());
        }
        self.put_blob(
            &format!("{key}/"),
            Body::from(Vec::new()),
            0,
            "create the folder",
        )
    }

    fn delete_blob(&self, key: &str) -> Result<(), String> {
        let request = self.request(Method::DELETE, key, &[], &[], 0)?;
        self.send(request, "delete the blob").map(|_| ())
    }

    pub fn delete(&self, path: &str, recursive: bool) -> Result<(), String> {
        let item = self.stat(path)?;
        let key = Self::key(path);
        if !item.is_dir {
            return self.delete_blob(&key);
        }
        let (files, _) = self.blobs(&format!("{key}/"), None, None)?;
        if !recursive && files.iter().any(|file| file.path != format!("/{key}/")) {
            return Err("The folder is not empty.".into());
        }
        for file in files {
            self.delete_blob(file.path.trim_start_matches('/'))?;
        }
        Ok(())
    }

    /// Copies one blob within the container and waits for the copy to finish.
    fn copy_blob(&self, from: &str, to: &str) -> Result<(), String> {
        let source = self.url(from, &[]).to_string();
        let request = self.request(Method::PUT, to, &[], &[("x-ms-copy-source", source)], 0)?;
        let response = self.send(request, "copy the blob")?;
        let mut status = response
            .headers()
            .get("x-ms-copy-status")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("success")
            .to_string();
        let started = Instant::now();
        while status == "pending" {
            if started.elapsed() > Duration::from_secs(600) {
                return Err("Azure is still copying the blob. Try again later.".into());
            }
            std::thread::sleep(Duration::from_millis(500));
            let head = self.send(
                self.request(Method::HEAD, to, &[], &[], 0)?,
                "check the copy",
            )?;
            status = head
                .headers()
                .get("x-ms-copy-status")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("success")
                .to_string();
        }
        if status != "success" {
            return Err(format!("Azure could not copy the blob ({status})."));
        }
        Ok(())
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        let item = self.stat(from)?;
        let (source, target) = (Self::key(from), Self::key(to));
        if target.is_empty() || target == source {
            return Err("Choose a new name.".into());
        }
        if self.stat(to).is_ok() {
            return Err(format!("{} already exists.", normalize(to)));
        }
        if !item.is_dir {
            self.copy_blob(&source, &target)?;
            return self.delete_blob(&source);
        }
        let (files, _) = self.blobs(&format!("{source}/"), None, None)?;
        for file in &files {
            let name = file.path.trim_start_matches('/');
            self.copy_blob(name, &format!("{target}/{}", &name[source.len() + 1..]))?;
        }
        for file in files {
            self.delete_blob(file.path.trim_start_matches('/'))?;
        }
        Ok(())
    }

    pub fn open_read(&self, path: &str, offset: u64) -> Result<Response, String> {
        let key = Self::key(path);
        let headers = if offset > 0 {
            vec![("x-ms-range", format!("bytes={offset}-"))]
        } else {
            Vec::new()
        };
        self.send(
            self.request(Method::GET, &key, &[], &headers, 0)?,
            "download the blob",
        )
    }

    pub fn open_write(
        &self,
        path: &str,
        truncate: bool,
        offset: u64,
    ) -> Result<AzureUpload, String> {
        if Self::key(path).is_empty() {
            return Err("Choose a file name inside the container.".into());
        }
        let mut file = NamedTempFile::new()
            .map_err(|error| format!("Could not prepare the upload: {error}"))?;
        if !truncate && offset > 0 {
            let mut response = self.open_read(path, 0)?;
            std::io::copy(&mut response, &mut file)
                .map_err(|error| format!("Could not resume the upload: {error}"))?;
            let length = file
                .as_file()
                .metadata()
                .map_err(|error| format!("Could not inspect the upload: {error}"))?
                .len();
            if length != offset {
                return Err("The partial blob changed. Remove it and retry.".into());
            }
        }
        Ok(AzureUpload {
            file,
            path: path.into(),
            position: offset,
        })
    }

    pub fn write_chunk(upload: &mut AzureUpload, offset: u64, data: &[u8]) -> Result<(), String> {
        if upload.position != offset {
            return Err("The upload position changed unexpectedly. Retry the transfer.".into());
        }
        upload
            .file
            .write_all(data)
            .map_err(|error| format!("Could not spool the upload: {error}"))?;
        upload.position += data.len() as u64;
        Ok(())
    }

    pub fn finish_write(&self, upload: AzureUpload) -> Result<(), String> {
        self.finish_write_with(upload, SINGLE_UPLOAD, BLOCK_SIZE)
    }

    fn finish_write_with(
        &self,
        mut upload: AzureUpload,
        single: u64,
        block_size: u64,
    ) -> Result<(), String> {
        let key = Self::key(&upload.path);
        let size = upload
            .file
            .as_file()
            .metadata()
            .map_err(|error| format!("Could not inspect the upload: {error}"))?
            .len();
        let file = upload.file.as_file_mut();
        file.seek(SeekFrom::Start(0))
            .map_err(|error| format!("Could not read the upload: {error}"))?;
        if size <= single {
            let copy = file
                .try_clone()
                .map_err(|error| format!("Could not read the upload: {error}"))?;
            return self.put_blob(&key, Body::sized(copy, size), size, "upload the blob");
        }
        let mut ids = Vec::new();
        let mut buffer = vec![0_u8; block_size as usize];
        let mut index = 0_u32;
        loop {
            let mut filled = 0;
            while filled < buffer.len() {
                let count = file
                    .read(&mut buffer[filled..])
                    .map_err(|error| format!("Could not read the upload: {error}"))?;
                if count == 0 {
                    break;
                }
                filled += count;
            }
            if filled == 0 {
                break;
            }
            let id = STANDARD.encode(format!("weave-{index:08}"));
            let request = self
                .request(
                    Method::PUT,
                    &key,
                    &[("comp", "block".into()), ("blockid", id.clone())],
                    &[],
                    filled as u64,
                )?
                .body(buffer[..filled].to_vec());
            self.send(request, "upload part of the blob")?;
            ids.push(id);
            index += 1;
        }
        let list = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><BlockList>{}</BlockList>",
            ids.iter()
                .map(|id| format!("<Latest>{id}</Latest>"))
                .collect::<String>()
        );
        let request = self
            .request(
                Method::PUT,
                &key,
                &[("comp", "blocklist".into())],
                &[("content-type", "application/xml".into())],
                list.len() as u64,
            )?
            .body(list);
        self.send(request, "finish the blob upload").map(|_| ())
    }

    pub fn write_small(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let key = Self::key(path);
        self.put_blob(
            &key,
            Body::from(bytes.to_vec()),
            bytes.len() as u64,
            "upload the blob",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Azurite's well-known development key, published by Microsoft for the
    /// local emulator. It opens nothing outside a test.
    const KEY: &str =
        "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

    fn profile(host: &str, user: &str) -> Profile {
        Profile {
            id: "azure-test".into(),
            name: "Azure".into(),
            host: host.into(),
            protocol: "azure".into(),
            port: 443,
            username: user.into(),
            key_path: None,
            jump_host: None,
            color: String::new(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        }
    }

    #[test]
    fn reads_the_account_and_credential_from_the_profile() {
        let client = AzureClient::connect(
            &profile("https://myaccount.blob.core.windows.net/photos/", ""),
            KEY,
        )
        .unwrap();
        assert_eq!(client.account, "myaccount");
        assert_eq!(
            client.url("a b/c.txt", &[]).as_str(),
            "https://myaccount.blob.core.windows.net/photos/a%20b/c.txt"
        );
        let sas = AzureClient::connect(
            &profile("https://myaccount.blob.core.windows.net/photos", ""),
            "?sv=2022-11-02&sp=rl&sig=abc%3D",
        )
        .unwrap();
        assert_eq!(
            sas.url("", &[("comp", "list".into())]).as_str(),
            "https://myaccount.blob.core.windows.net/photos?comp=list&sv=2022-11-02&sp=rl&sig=abc%3D"
        );
        assert!(AzureClient::connect(&profile("http://example.com/c", "a"), KEY).is_err());
        assert!(
            AzureClient::connect(&profile("https://a.blob.core.windows.net", "a"), KEY).is_err()
        );
        assert!(AzureClient::connect(
            &profile("https://a.blob.core.windows.net/c", "a"),
            "not base64!"
        )
        .err()
        .unwrap()
        .contains("account key"));
    }

    #[test]
    #[ignore = "requires Azurite, the Azure Storage emulator"]
    fn azure_integration_browses_transfers_and_renames() {
        let port = std::env::var("WEAVE_AZURE_TEST_PORT").unwrap();
        let account = "devstoreaccount1";
        let base = format!("http://127.0.0.1:{port}/{account}/weave-test");
        let client = AzureClient::connect(&profile(&base, account), KEY).unwrap();
        // Create the container.
        let _ = client.send(
            client
                .request(Method::PUT, "", &[("restype", "container".into())], &[], 0)
                .unwrap(),
            "create the container",
        );
        let wrong =
            AzureClient::connect(&profile(&base, account), &STANDARD.encode(b"wrong key")).unwrap();
        assert!(wrong.list("/").err().unwrap().contains("HTTP 403"));

        client.mkdir("/docs").unwrap();
        assert!(client.stat("/docs").unwrap().is_dir);
        assert!(client.list("/docs").unwrap().entries.is_empty());
        client
            .write_small("/docs/a &b ü.txt", b"hello azure")
            .unwrap();
        let listing = client.list("/docs").unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, "a &b ü.txt");
        assert_eq!(listing.entries[0].size, 11);
        assert!(listing.entries[0].modified.is_some());
        assert!(client
            .list("/")
            .unwrap()
            .entries
            .iter()
            .any(|e| e.is_dir && e.name == "docs"));

        let mut ranged = client.open_read("/docs/a &b ü.txt", 6).unwrap();
        let mut text = String::new();
        ranged.read_to_string(&mut text).unwrap();
        assert_eq!(text, "azure");

        let mut upload = client.open_write("/big/blocks.bin", true, 0).unwrap();
        let data: Vec<u8> = (0..2_500_000).map(|n| (n % 251) as u8).collect();
        AzureClient::write_chunk(&mut upload, 0, &data).unwrap();
        client
            .finish_write_with(upload, 1_000_000, 1_000_000)
            .unwrap();
        let mut downloaded = Vec::new();
        client
            .open_read("/big/blocks.bin", 0)
            .unwrap()
            .read_to_end(&mut downloaded)
            .unwrap();
        assert_eq!(downloaded, data);

        client.rename("/docs", "/renamed").unwrap();
        assert!(client.stat("/docs").is_err());
        assert_eq!(
            client.list("/renamed").unwrap().entries[0].name,
            "a &b ü.txt"
        );
        assert!(client
            .delete("/renamed", false)
            .unwrap_err()
            .contains("not empty"));
        client.delete("/renamed", true).unwrap();
        client.delete("/big/blocks.bin", false).unwrap();
        assert!(client.list("/").unwrap().entries.is_empty());
    }
}
