use crate::{
    files::{FileEntry, FileListing},
    ftp::FtpClient,
    profiles::{self, Profile},
    vault, AppState,
};
use std::{
    io::{Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};
use tauri::State;

const INIT: u8 = 1;
const VERSION: u8 = 2;
const OPEN: u8 = 3;
const CLOSE: u8 = 4;
const READ: u8 = 5;
const WRITE: u8 = 6;
const LSTAT: u8 = 7;
const SETSTAT: u8 = 9;
const EXTENDED: u8 = 200;
const OPENDIR: u8 = 11;
const READDIR: u8 = 12;
const REMOVE: u8 = 13;
const MKDIR: u8 = 14;
const RMDIR: u8 = 15;
const REALPATH: u8 = 16;
const RENAME: u8 = 18;
const STATUS: u8 = 101;
const HANDLE: u8 = 102;
const DATA: u8 = 103;
const NAME: u8 = 104;
const ATTRS: u8 = 105;

const READ_FLAG: u32 = 1;
const WRITE_FLAG: u32 = 2;
const APPEND_FLAG: u32 = 4;
const CREATE_FLAG: u32 = 8;
const TRUNCATE_FLAG: u32 = 16;

pub struct SftpClient {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    next_id: u32,
    copy_data: bool,
}

#[derive(Default, Clone)]
pub struct FileAttrs {
    pub size: u64,
    pub modified: Option<u64>,
    pub permissions: Option<u32>,
}

struct Packet<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Packet<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .position
            .checked_add(count)
            .ok_or("Malformed SFTP response.")?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or("Truncated SFTP response.")?;
        self.position = end;
        Ok(bytes)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<&'a [u8], String> {
        let len = self.u32()? as usize;
        self.take(len)
    }
    fn text(&mut self) -> Result<String, String> {
        Ok(String::from_utf8_lossy(self.string()?).into_owned())
    }
    fn attrs(&mut self) -> Result<FileAttrs, String> {
        let flags = self.u32()?;
        let mut result = FileAttrs::default();
        if flags & 1 != 0 {
            result.size = self.u64()?;
        }
        if flags & 2 != 0 {
            self.take(8)?;
        }
        if flags & 4 != 0 {
            result.permissions = Some(self.u32()?);
        }
        if flags & 8 != 0 {
            self.take(4)?;
            result.modified = Some(self.u32()? as u64);
        }
        if flags & 0x8000_0000 != 0 {
            let count = self.u32()?;
            for _ in 0..count {
                self.string()?;
                self.string()?;
            }
        }
        Ok(result)
    }
}

fn u32_bytes(value: u32, out: &mut Vec<u8>) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn u64_bytes(value: u64, out: &mut Vec<u8>) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn string_bytes(value: &[u8], out: &mut Vec<u8>) {
    u32_bytes(value.len() as u32, out);
    out.extend_from_slice(value);
}

impl SftpClient {
    pub fn connect(profile: &Profile) -> Result<Self, String> {
        if profile.protocol != "ssh" {
            return Err("This connection uses FTP or FTPS. Open it in Files instead.".into());
        }
        let socket = crate::ssh::control_path(profile)?;
        let mut command = crate::platform::command("ssh");
        // Passwords, verification codes and new host keys are asked in Weave's
        // window; the sign-in then stays open for five minutes.
        command.args(["-o", "ConnectTimeout=10"]);
        command.args(crate::ssh::sharing_args("auto", "300", &socket));
        crate::ssh_prompt::apply(&mut command, profile);
        if let Ok(known_hosts) = std::env::var("WEAVE_SSH_TEST_KNOWN_HOSTS") {
            command
                .arg("-o")
                .arg(format!("UserKnownHostsFile={known_hosts}"));
            command.arg("-o").arg("StrictHostKeyChecking=yes");
        }
        command.arg("-p").arg(profile.port.to_string());
        command.args(crate::host_options::prepare(profile));
        if let Some(key) = profile.key_path.as_ref() {
            command.arg("-i").arg(expand_home(key));
        }
        if let Some(jump) = profile.jump_host.as_ref() {
            command.arg("-J").arg(jump);
        }
        command
            .arg("-l")
            .arg(&profile.username)
            .arg("-s")
            .arg(&profile.host)
            .arg("sftp");
        Self::from_command(command).map_err(|error| sign_in_error(profile, &error))
    }

    pub(crate) fn from_command(mut command: Command) -> Result<Self, String> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Could not start SFTP: {error}"))?;
        let input = child.stdin.take().ok_or("SFTP input is unavailable.")?;
        let output = child.stdout.take().ok_or("SFTP output is unavailable.")?;
        let mut client = Self {
            child,
            input,
            output,
            next_id: 1,
            copy_data: false,
        };
        let mut init = vec![INIT];
        u32_bytes(3, &mut init);
        client.send(&init)?;
        let version = client.receive()?;
        let mut packet = Packet::new(&version);
        if packet.take(1)?[0] != VERSION || packet.u32()? != 3 {
            return Err("This SFTP server does not support protocol version 3.".into());
        }
        while packet.position < version.len() {
            let name = packet.text()?;
            let revision = packet.text()?;
            if name == "copy-data" && revision == "1" {
                client.copy_data = true;
            }
        }
        Ok(client)
    }

    fn send(&mut self, payload: &[u8]) -> Result<(), String> {
        self.input
            .write_all(&(payload.len() as u32).to_be_bytes())
            .and_then(|_| self.input.write_all(payload))
            .map_err(|error| format!("SFTP connection closed while sending data: {error}"))
    }

    fn receive(&mut self) -> Result<Vec<u8>, String> {
        let mut len_bytes = [0; 4];
        if let Err(error) = self.output.read_exact(&mut len_bytes) {
            let mut detail = String::new();
            if let Some(mut stderr) = self.child.stderr.take() {
                let _ = stderr.read_to_string(&mut detail);
            }
            let detail = detail.trim();
            return Err(if detail.is_empty() {
                format!("SFTP connection closed: {error}")
            } else {
                detail.to_string()
            });
        }
        let len = u32::from_be_bytes(len_bytes) as usize;
        if len > 16 * 1024 * 1024 {
            return Err("SFTP server sent an oversized response.".into());
        }
        let mut response = vec![0; len];
        self.output
            .read_exact(&mut response)
            .map_err(|error| format!("SFTP response was incomplete: {error}"))?;
        Ok(response)
    }

    fn request(&mut self, kind: u8, payload: &[u8]) -> Result<(u8, Vec<u8>), String> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let mut message = vec![kind];
        u32_bytes(id, &mut message);
        message.extend_from_slice(payload);
        self.send(&message)?;
        let response = self.receive()?;
        let mut packet = Packet::new(&response);
        let response_kind = packet.take(1)?[0];
        if packet.u32()? != id {
            return Err("SFTP server returned an unexpected response ID.".into());
        }
        Ok((response_kind, response[5..].to_vec()))
    }

    fn expect(&mut self, kind: u8, payload: &[u8], expected: u8) -> Result<Vec<u8>, String> {
        let (response_kind, response) = self.request(kind, payload)?;
        if response_kind == STATUS {
            let mut packet = Packet::new(&response);
            let code = packet.u32()?;
            let message = packet.text().unwrap_or_default();
            if code == 0 && expected == STATUS {
                return Ok(response);
            }
            return Err(if message.is_empty() {
                format!("SFTP operation failed (code {code}).")
            } else {
                message
            });
        }
        if response_kind == expected {
            return Ok(response);
        }
        Err(format!(
            "SFTP server returned unexpected packet type {response_kind}."
        ))
    }

    pub fn realpath(&mut self, path: &str) -> Result<String, String> {
        let mut payload = Vec::new();
        string_bytes(path.as_bytes(), &mut payload);
        let response = self.expect(REALPATH, &payload, NAME)?;
        let mut packet = Packet::new(&response);
        if packet.u32()? == 0 {
            return Err("SFTP server did not return a path.".into());
        }
        packet.text()
    }

    pub fn stat(&mut self, path: &str) -> Result<FileAttrs, String> {
        let mut payload = Vec::new();
        string_bytes(path.as_bytes(), &mut payload);
        let response = self.expect(LSTAT, &payload, ATTRS)?;
        Packet::new(&response).attrs()
    }

    pub fn list(&mut self, path: &str) -> Result<FileListing, String> {
        let canonical = self.realpath(path)?;
        let mut payload = Vec::new();
        string_bytes(canonical.as_bytes(), &mut payload);
        let handle = self.expect(OPENDIR, &payload, HANDLE)?;
        let mut handle_packet = Packet::new(&handle);
        let handle = handle_packet.string()?.to_vec();
        let mut entries = Vec::new();
        loop {
            let mut request = Vec::new();
            string_bytes(&handle, &mut request);
            let (kind, response) = self.request(READDIR, &request)?;
            if kind == STATUS {
                let mut packet = Packet::new(&response);
                if packet.u32()? == 1 {
                    break;
                }
                return Err(packet
                    .text()
                    .unwrap_or_else(|_| "Could not read remote folder.".into()));
            }
            if kind != NAME {
                return Err("SFTP server returned an invalid folder listing.".into());
            }
            let mut packet = Packet::new(&response);
            let count = packet.u32()?;
            for _ in 0..count {
                let name = packet.text()?;
                packet.string()?;
                let attrs = packet.attrs()?;
                if name == "." || name == ".." {
                    continue;
                }
                let mode = attrs.permissions.unwrap_or(0);
                entries.push(FileEntry {
                    path: join_remote(&canonical, &name),
                    name,
                    is_dir: mode & 0o170000 == 0o040000,
                    is_symlink: mode & 0o170000 == 0o120000,
                    size: attrs.size,
                    modified: attrs.modified,
                    readonly: mode & 0o222 == 0,
                });
            }
        }
        self.close(&handle)?;
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let parent = if canonical == "/" {
            None
        } else {
            Some(
                canonical
                    .rsplit_once('/')
                    .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
                    .unwrap_or("/")
                    .to_string(),
            )
        };
        Ok(FileListing {
            path: canonical,
            parent,
            entries,
        })
    }

    pub fn open_read(&mut self, path: &str) -> Result<Vec<u8>, String> {
        self.open(path, READ_FLAG)
    }
    pub fn open_write(&mut self, path: &str, truncate: bool) -> Result<Vec<u8>, String> {
        self.open(
            path,
            WRITE_FLAG | CREATE_FLAG | if truncate { TRUNCATE_FLAG } else { APPEND_FLAG },
        )
    }

    fn open(&mut self, path: &str, flags: u32) -> Result<Vec<u8>, String> {
        let mut payload = Vec::new();
        string_bytes(path.as_bytes(), &mut payload);
        u32_bytes(flags, &mut payload);
        u32_bytes(0, &mut payload);
        let response = self.expect(OPEN, &payload, HANDLE)?;
        Ok(Packet::new(&response).string()?.to_vec())
    }

    pub fn read(
        &mut self,
        handle: &[u8],
        offset: u64,
        length: u32,
    ) -> Result<Option<Vec<u8>>, String> {
        let mut payload = Vec::new();
        string_bytes(handle, &mut payload);
        u64_bytes(offset, &mut payload);
        u32_bytes(length, &mut payload);
        let (kind, response) = self.request(READ, &payload)?;
        if kind == STATUS {
            let mut packet = Packet::new(&response);
            return if packet.u32()? == 1 {
                Ok(None)
            } else {
                Err(packet
                    .text()
                    .unwrap_or_else(|_| "Could not read remote file.".into()))
            };
        }
        if kind != DATA {
            return Err("SFTP server returned invalid file data.".into());
        }
        Ok(Some(Packet::new(&response).string()?.to_vec()))
    }

    pub fn write(&mut self, handle: &[u8], offset: u64, data: &[u8]) -> Result<(), String> {
        let mut payload = Vec::new();
        string_bytes(handle, &mut payload);
        u64_bytes(offset, &mut payload);
        string_bytes(data, &mut payload);
        self.expect(WRITE, &payload, STATUS).map(|_| ())
    }

    pub fn close(&mut self, handle: &[u8]) -> Result<(), String> {
        let mut payload = Vec::new();
        string_bytes(handle, &mut payload);
        self.expect(CLOSE, &payload, STATUS).map(|_| ())
    }

    pub fn supports_copy_data(&self) -> bool {
        self.copy_data
    }

    pub fn copy_data(
        &mut self,
        source: &[u8],
        destination: &[u8],
        offset: u64,
        length: u64,
    ) -> Result<(), String> {
        if !self.copy_data {
            return Err("This SFTP server does not support server-side copying.".into());
        }
        let mut payload = Vec::new();
        string_bytes(b"copy-data", &mut payload);
        string_bytes(source, &mut payload);
        u64_bytes(offset, &mut payload);
        u64_bytes(length, &mut payload);
        string_bytes(destination, &mut payload);
        u64_bytes(offset, &mut payload);
        self.expect(EXTENDED, &payload, STATUS)
            .map(|_| ())
            .map_err(|error| format!("Server-side SFTP copy failed: {error}"))
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), String> {
        let mut payload = Vec::new();
        string_bytes(path.as_bytes(), &mut payload);
        u32_bytes(0, &mut payload);
        self.expect(MKDIR, &payload, STATUS).map(|_| ())
    }

    pub fn remove(&mut self, path: &str, is_dir: bool) -> Result<(), String> {
        let mut payload = Vec::new();
        string_bytes(path.as_bytes(), &mut payload);
        self.expect(if is_dir { RMDIR } else { REMOVE }, &payload, STATUS)
            .map(|_| ())
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        let mut payload = Vec::new();
        string_bytes(from.as_bytes(), &mut payload);
        string_bytes(to.as_bytes(), &mut payload);
        self.expect(RENAME, &payload, STATUS).map(|_| ())
    }

    pub fn chmod(&mut self, path: &str, mode: u32) -> Result<(), String> {
        let mut payload = Vec::new();
        string_bytes(path.as_bytes(), &mut payload);
        u32_bytes(4, &mut payload);
        u32_bytes(mode, &mut payload);
        self.expect(SETSTAT, &payload, STATUS).map(|_| ())
    }

    pub fn set_mtime(&mut self, path: &str, seconds: u64) -> Result<(), String> {
        let seconds = u32::try_from(seconds).map_err(|_| {
            "This modification time is outside the SFTP server's supported range.".to_owned()
        })?;
        let mut payload = Vec::new();
        string_bytes(path.as_bytes(), &mut payload);
        u32_bytes(8, &mut payload);
        u32_bytes(seconds, &mut payload);
        u32_bytes(seconds, &mut payload);
        self.expect(SETSTAT, &payload, STATUS).map(|_| ())
    }
}

impl Drop for SftpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Turns ssh's last words into what to check.
pub(crate) fn sign_in_error(profile: &Profile, detail: &str) -> String {
    let detail = detail.trim();
    let Some(start) = detail.find("Permission denied") else {
        return format!("Could not connect to {}: {detail}", profile.name);
    };
    let methods = detail[start..]
        .split_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(methods, _)| methods)
        .unwrap_or("");
    let passwords = methods.contains("password") || methods.contains("keyboard-interactive");
    let advice = if !passwords && methods.contains("publickey") {
        "This server only accepts SSH keys. Choose the private key it expects in the host editor, or add your key to the SSH agent."
    } else if passwords {
        "The server did not accept the sign-in. Check that the user name matches exactly (case matters) and that the password or code is right."
    } else {
        "Check the user name, password or key this server expects."
    };
    format!(
        "{} refused the sign-in as “{}”. {advice} (ssh said: {})",
        profile.name,
        profile.username,
        detail[start..]
            .lines()
            .next()
            .unwrap_or("Permission denied")
    )
}

pub fn join_remote(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", parent.trim_end_matches('/'))
    }
}

pub fn validate_remote_delete(path: &str) -> Result<(), String> {
    if path.trim_matches('/').is_empty() || path.split('/').any(|part| part == "." || part == "..")
    {
        return Err(
            "Choose a specific remote item. The root and parent paths cannot be deleted.".into(),
        );
    }
    Ok(())
}

fn expand_home(path: &str) -> String {
    path.strip_prefix("~/")
        .and_then(|rest| dirs::home_dir().map(|home| home.join(rest).to_string_lossy().to_string()))
        .unwrap_or_else(|| path.to_string())
}

#[tauri::command]
pub async fn sftp_list(
    state: State<'_, AppState>,
    profile_id: String,
    path: String,
) -> Result<FileListing, String> {
    let profile = profiles::get_profile(&state, &profile_id)?;
    if crate::cloud::is_cloud(&profile.protocol) {
        let client = crate::cloud::CloudClient::connect(&profile)?;
        return tauri::async_runtime::spawn_blocking(move || client.list(&path))
            .await
            .map_err(|error| format!("The cloud drive request stopped unexpectedly: {error}"))?;
    }
    let password = (profile.protocol != "ssh")
        .then(|| vault::password_for(&state, &profile))
        .transpose()?;
    let s3_token = (profile.protocol == "s3")
        .then(|| vault::s3_token_for(&state, &profile_id))
        .transpose()?
        .flatten();
    tauri::async_runtime::spawn_blocking(move || {
        if profile.protocol == "webdav" {
            crate::webdav::WebDavClient::connect(
                &profile,
                password.as_ref().map_or("", |value| value.as_str()),
            )?
            .list(&path)
        } else if profile.protocol == "s3" {
            crate::s3::S3Client::connect_with_token(
                &profile,
                password.as_ref().map_or("", |value| value.as_str()),
                s3_token.as_ref().map(|value| value.as_str()),
            )?
            .list(&path)
        } else if profile.protocol == "azure" {
            crate::azure::AzureClient::connect(
                &profile,
                password.as_ref().map_or("", |value| value.as_str()),
            )?
            .list(&path)
        } else if let Some(password) = password {
            FtpClient::connect(&profile, &password)?.list(&path)
        } else {
            SftpClient::connect(&profile)?.list(&path)
        }
    })
    .await
    .map_err(|error| format!("Remote listing stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn sftp_mkdir(
    state: State<'_, AppState>,
    profile_id: String,
    parent: String,
    name: String,
) -> Result<(), String> {
    validate_name(&name)?;
    let profile = profiles::get_profile(&state, &profile_id)?;
    if crate::cloud::is_cloud(&profile.protocol) {
        let client = crate::cloud::CloudClient::connect(&profile)?;
        return tauri::async_runtime::spawn_blocking(move || {
            client.mkdir(&join_remote(&parent, &name))
        })
        .await
        .map_err(|error| format!("The cloud drive request stopped unexpectedly: {error}"))?;
    }
    let password = (profile.protocol != "ssh")
        .then(|| vault::password_for(&state, &profile))
        .transpose()?;
    let s3_token = (profile.protocol == "s3")
        .then(|| vault::s3_token_for(&state, &profile_id))
        .transpose()?
        .flatten();
    tauri::async_runtime::spawn_blocking(move || {
        let path = join_remote(&parent, &name);
        if profile.protocol == "webdav" {
            crate::webdav::WebDavClient::connect(
                &profile,
                password.as_ref().map_or("", |value| value.as_str()),
            )?
            .mkdir(&path)
        } else if profile.protocol == "s3" {
            crate::s3::S3Client::connect_with_token(
                &profile,
                password.as_ref().map_or("", |value| value.as_str()),
                s3_token.as_ref().map(|value| value.as_str()),
            )?
            .mkdir(&path)
        } else if profile.protocol == "azure" {
            crate::azure::AzureClient::connect(
                &profile,
                password.as_ref().map_or("", |value| value.as_str()),
            )?
            .mkdir(&path)
        } else if let Some(password) = password {
            FtpClient::connect(&profile, &password)?.mkdir(&path)
        } else {
            SftpClient::connect(&profile)?.mkdir(&path)
        }
    })
    .await
    .map_err(|error| format!("Remote folder creation stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn sftp_rename(
    state: State<'_, AppState>,
    profile_id: String,
    path: String,
    name: String,
) -> Result<(), String> {
    validate_name(&name)?;
    let parent = path
        .rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .ok_or("Cannot rename the remote root folder.")?;
    let target = join_remote(parent, &name);
    let parent = parent.to_string();
    let profile = profiles::get_profile(&state, &profile_id)?;
    if crate::cloud::is_cloud(&profile.protocol) {
        let client = crate::cloud::CloudClient::connect(&profile)?;
        return tauri::async_runtime::spawn_blocking(move || {
            if client
                .list(&parent)?
                .entries
                .iter()
                .any(|entry| entry.name == name)
            {
                return Err(format!("{target} already exists. Choose a different name."));
            }
            client.rename(&path, &target)
        })
        .await
        .map_err(|error| format!("The cloud drive request stopped unexpectedly: {error}"))?;
    }
    let password = (profile.protocol != "ssh")
        .then(|| vault::password_for(&state, &profile))
        .transpose()?;
    let s3_token = (profile.protocol == "s3")
        .then(|| vault::s3_token_for(&state, &profile_id))
        .transpose()?
        .flatten();
    tauri::async_runtime::spawn_blocking(move || {
        if profile.protocol == "webdav" {
            let client = crate::webdav::WebDavClient::connect(
                &profile,
                password.as_ref().map_or("", |value| value.as_str()),
            )?;
            if client
                .list(&parent)?
                .entries
                .iter()
                .any(|entry| entry.name == name)
            {
                return Err(format!("{target} already exists. Choose a different name."));
            }
            client.rename(&path, &target)
        } else if profile.protocol == "s3" {
            let client = crate::s3::S3Client::connect_with_token(
                &profile,
                password.as_ref().map_or("", |value| value.as_str()),
                s3_token.as_ref().map(|value| value.as_str()),
            )?;
            if client
                .list(&parent)?
                .entries
                .iter()
                .any(|entry| entry.name == name)
            {
                return Err(format!("{target} already exists. Choose a different name."));
            }
            client.rename(&path, &target)
        } else if profile.protocol == "azure" {
            let client = crate::azure::AzureClient::connect(
                &profile,
                password.as_ref().map_or("", |value| value.as_str()),
            )?;
            if client
                .list(&parent)?
                .entries
                .iter()
                .any(|entry| entry.name == name)
            {
                return Err(format!("{target} already exists. Choose a different name."));
            }
            client.rename(&path, &target)
        } else if let Some(password) = password {
            let mut client = FtpClient::connect(&profile, &password)?;
            if client
                .list(&parent)?
                .entries
                .iter()
                .any(|entry| entry.name == name)
            {
                return Err(format!("{target} already exists. Choose a different name."));
            }
            client.rename(&path, &target)
        } else {
            let mut client = SftpClient::connect(&profile)?;
            if client
                .list(&parent)?
                .entries
                .iter()
                .any(|entry| entry.name == name)
            {
                return Err(format!("{target} already exists. Choose a different name."));
            }
            client.rename(&path, &target)
        }
    })
    .await
    .map_err(|error| format!("Remote rename stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn sftp_delete(
    state: State<'_, AppState>,
    profile_id: String,
    path: String,
    recursive: bool,
) -> Result<(), String> {
    validate_remote_delete(&path)?;
    let profile = profiles::get_profile(&state, &profile_id)?;
    if crate::cloud::is_cloud(&profile.protocol) {
        let client = crate::cloud::CloudClient::connect(&profile)?;
        return tauri::async_runtime::spawn_blocking(move || client.delete(&path, recursive))
            .await
            .map_err(|error| format!("The cloud drive request stopped unexpectedly: {error}"))?;
    }
    let password = (profile.protocol != "ssh")
        .then(|| vault::password_for(&state, &profile))
        .transpose()?;
    let s3_token = (profile.protocol == "s3")
        .then(|| vault::s3_token_for(&state, &profile_id))
        .transpose()?
        .flatten();
    tauri::async_runtime::spawn_blocking(move || {
        if profile.protocol == "webdav" {
            crate::webdav::WebDavClient::connect(
                &profile,
                password.as_ref().map_or("", |value| value.as_str()),
            )?
            .delete(&path, recursive)
        } else if profile.protocol == "s3" {
            crate::s3::S3Client::connect_with_token(
                &profile,
                password.as_ref().map_or("", |value| value.as_str()),
                s3_token.as_ref().map(|value| value.as_str()),
            )?
            .delete(&path, recursive)
        } else if profile.protocol == "azure" {
            crate::azure::AzureClient::connect(
                &profile,
                password.as_ref().map_or("", |value| value.as_str()),
            )?
            .delete(&path, recursive)
        } else if let Some(password) = password {
            FtpClient::connect(&profile, &password)?.delete(&path, recursive)
        } else {
            let mut client = SftpClient::connect(&profile)?;
            delete_remote(&mut client, &path, recursive)
        }
    })
    .await
    .map_err(|error| format!("Remote delete stopped unexpectedly: {error}"))?
}

fn delete_remote(client: &mut SftpClient, path: &str, recursive: bool) -> Result<(), String> {
    let attrs = client.stat(path)?;
    let is_dir = attrs.permissions.unwrap_or(0) & 0o170000 == 0o040000;
    if is_dir && recursive {
        for entry in client.list(path)?.entries {
            delete_remote(client, &entry.path, true)?;
        }
    }
    client.remove(path, is_dir)
}

#[tauri::command]
pub async fn sftp_chmod(
    state: State<'_, AppState>,
    profile_id: String,
    path: String,
    mode: u32,
) -> Result<(), String> {
    if mode > 0o7777 {
        return Err("Permissions must be an octal value between 0000 and 7777.".into());
    }
    let profile = profiles::get_profile(&state, &profile_id)?;
    if crate::cloud::is_cloud(&profile.protocol) {
        return Err("Cloud drives do not use Unix file permissions. Change sharing in the provider's website.".into());
    }
    let password = (profile.protocol != "ssh")
        .then(|| vault::password_for(&state, &profile))
        .transpose()?;
    tauri::async_runtime::spawn_blocking(move || {
        if profile.protocol == "webdav" {
            Err("WebDAV does not expose Unix file permissions. Use a server access-control setting instead.".into())
        } else if profile.protocol == "s3" {
            Err("S3 object permissions are managed by bucket policies and access controls, not Unix file modes.".into())
        } else if profile.protocol == "azure" {
            Err("Azure blobs have no Unix file permissions. Manage access in the Azure portal.".into())
        } else if let Some(password) = password {
            FtpClient::connect(&profile, &password)?.chmod(&path, mode)
        } else {
            SftpClient::connect(&profile)?.chmod(&path, mode)
        }
    })
    .await
    .map_err(|error| format!("Permission change stopped unexpectedly: {error}"))?
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        Err("Enter a single file or folder name without slashes.".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_refused_sign_ins() {
        let profile = Profile {
            id: "p".into(),
            name: "HPC".into(),
            host: "203.0.113.10".into(),
            protocol: "ssh".into(),
            port: 22,
            username: "Ada".into(),
            key_path: None,
            jump_host: None,
            color: String::new(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let refused = sign_in_error(
            &profile,
            "Ada@203.0.113.10: Permission denied (publickey,password,keyboard-interactive).",
        );
        assert!(
            refused.contains("“Ada”") && refused.contains("case matters"),
            "{refused}"
        );
        let keys = sign_in_error(&profile, "Permission denied (publickey).");
        assert!(keys.contains("only accepts SSH keys"), "{keys}");
        assert!(sign_in_error(&profile, "Connection refused").contains("Connection refused"));
    }
    use rusqlite::{params, Connection};
    use std::{collections::HashMap, fs, io::Write, sync::Mutex};
    use tauri::Manager;
    use zeroize::Zeroizing;

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires the local SSH test server"]
    fn reuses_an_authenticated_ssh_master_for_sftp() {
        let port = std::env::var("WEAVE_SSH_TEST_PORT")
            .expect("Set WEAVE_SSH_TEST_PORT to the local SSH test server port");
        let key = std::env::var("WEAVE_SSH_TEST_KEY")
            .expect("Set WEAVE_SSH_TEST_KEY to the test private key path");
        let known_hosts = std::env::var("WEAVE_SSH_TEST_KNOWN_HOSTS")
            .expect("Set WEAVE_SSH_TEST_KNOWN_HOSTS to the test known-hosts path");
        let username = std::env::var("WEAVE_SSH_TEST_USER")
            .expect("Set WEAVE_SSH_TEST_USER to the local test account name");
        let root = std::env::temp_dir().join(format!("weave-ssh-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("notes.txt"), "connected once").unwrap();
        let profile = profiles::Profile {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Local SSH test".into(),
            host: "127.0.0.1".into(),
            protocol: "ssh".into(),
            port: port.parse().unwrap(),
            username: username.clone(),
            key_path: Some(key.clone()),
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        };
        let socket = crate::ssh::control_path(&profile).unwrap();
        let socket_arg = format!("ControlPath={}", socket.display());
        let known_hosts_arg = format!("UserKnownHostsFile={known_hosts}");
        let master = crate::platform::command("ssh")
            .args([
                "-MNf",
                "-o",
                "ControlMaster=yes",
                "-o",
                "ControlPersist=10",
                "-o",
                &socket_arg,
                "-o",
                &known_hosts_arg,
                "-o",
                "StrictHostKeyChecking=yes",
                "-p",
                &port,
                "-i",
                &key,
                "-l",
                &username,
                "127.0.0.1",
            ])
            .output()
            .unwrap();
        assert!(
            master.status.success(),
            "{}",
            String::from_utf8_lossy(&master.stderr)
        );

        let mut first = SftpClient::connect(&profile).unwrap();
        let mut second = SftpClient::connect(&profile).unwrap();
        assert!(first
            .list(&root.to_string_lossy())
            .unwrap()
            .entries
            .iter()
            .any(|item| item.name == "notes.txt"));
        assert_eq!(
            second
                .stat(&root.join("notes.txt").to_string_lossy())
                .unwrap()
                .size,
            14
        );
        drop(first);
        drop(second);

        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, protocol TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT, color TEXT, favorite INTEGER, tls_pin_sha256 TEXT, s3_region TEXT);").unwrap();
        db.execute(
            "INSERT INTO profiles VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, 0, NULL, NULL)",
            params![
                profile.id,
                profile.name,
                profile.host,
                profile.protocol,
                profile.port,
                profile.username,
                profile.key_path,
                profile.color
            ],
        )
        .unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::new()),
        });
        let state = app.state::<AppState>();
        let notes_path = root.join("notes.txt").to_string_lossy().to_string();
        let checked_out = crate::transfer::checkout_edit(
            &state,
            &root.join("external-edits"),
            profile.id.clone(),
            notes_path.clone(),
        )
        .unwrap();
        assert_eq!(
            fs::read(&checked_out.local_path).unwrap(),
            b"connected once"
        );
        let edited = (0..1_100_000)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(&checked_out.local_path, &edited).unwrap();
        let mut session = state
            .external_edits
            .lock()
            .unwrap()
            .remove(&checked_out.id)
            .unwrap();
        let saved =
            crate::transfer::upload_edit(&state, &mut session, checked_out.id.clone()).unwrap();
        assert!(saved.changed);
        assert_eq!(fs::read(&notes_path).unwrap(), edited);
        drop(session);
        let terminal_id = crate::terminal::open_terminal(
            app.handle().clone(),
            state.clone(),
            Some(profile.id.clone()),
            Some(root.to_string_lossy().to_string()),
            80,
            24,
        )
        .unwrap();
        let terminal_file = root.join("terminal-check.txt");
        for _ in 0..30 {
            crate::terminal::term_write(
                state.clone(),
                terminal_id.clone(),
                "printf 'terminal connected' > terminal-check.txt\n".into(),
            )
            .unwrap();
            if terminal_file.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        assert_eq!(
            fs::read_to_string(&terminal_file).unwrap(),
            "terminal connected"
        );
        crate::terminal::term_close(state, terminal_id).unwrap();

        let _ = crate::platform::command("ssh")
            .args([
                "-S",
                &socket.to_string_lossy(),
                "-O",
                "exit",
                "-p",
                &port,
                "-l",
                &username,
                "127.0.0.1",
            ])
            .output();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires the local FTP test server"]
    fn browser_rename_keeps_an_existing_ftp_file() {
        let port = std::env::var("WEAVE_FTP_TEST_PORT")
            .expect("Set WEAVE_FTP_TEST_PORT to the local FTP test server port");
        let profile = profiles::Profile {
            id: uuid::Uuid::new_v4().to_string(),
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
        let folder = format!("/rename-{}", uuid::Uuid::new_v4());
        let first = join_remote(&folder, "first.txt");
        let second = join_remote(&folder, "second.txt");
        let mut client = FtpClient::connect(&profile, "weave-test").unwrap();
        client.mkdir(&folder).unwrap();
        for (path, contents) in [
            (&first, b"first".as_slice()),
            (&second, b"second".as_slice()),
        ] {
            let mut upload = client.open_write(path, false).unwrap();
            upload.write_all(contents).unwrap();
            upload.finish().unwrap();
        }
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT, host TEXT, protocol TEXT, port INTEGER, username TEXT, key_path TEXT, jump_host TEXT, color TEXT, favorite INTEGER, tls_pin_sha256 TEXT, s3_region TEXT);").unwrap();
        db.execute(
            "INSERT INTO profiles VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7, 0, NULL, NULL)",
            params![
                profile.id,
                profile.name,
                profile.host,
                profile.protocol,
                profile.port,
                profile.username,
                profile.color
            ],
        )
        .unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState {
            db: Mutex::new(db),
            terminals: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            queue_lock: Mutex::new(()),
            external_edits: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(Vec::new()),
            session_s3_tokens: Mutex::new(HashMap::new()),
            session_passwords: Mutex::new(HashMap::from([(
                profile.id.clone(),
                Zeroizing::new("weave-test".into()),
            )])),
        });
        let error = tauri::async_runtime::block_on(sftp_rename(
            app.state::<AppState>(),
            profile.id,
            first,
            "second.txt".into(),
        ))
        .unwrap_err();
        assert!(error.contains("already exists"));
        let mut download = client.open_read(&second, 0).unwrap();
        let mut contents = String::new();
        download.read_to_string(&mut contents).unwrap();
        download.finish().unwrap();
        assert_eq!(contents, "second");
        client.delete(&folder, true).unwrap();
    }

    #[test]
    fn protects_remote_root_from_delete_aliases() {
        for path in ["/", "//", ".", "/folder/..", "/folder/./file"] {
            assert!(validate_remote_delete(path).is_err(), "{path}");
        }
        assert!(validate_remote_delete("/folder/file").is_ok());
    }

    fn local_server() -> Option<SftpClient> {
        let path = "/usr/libexec/sftp-server";
        if !std::path::Path::new(path).exists() {
            return None;
        }
        SftpClient::from_command(crate::platform::command(path)).ok()
    }

    #[test]
    fn reads_and_writes_with_real_sftp_server() {
        let Some(mut client) = local_server() else {
            return;
        };
        let root = std::env::temp_dir().join(format!("weave-sftp-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let folder = root.to_string_lossy().to_string();
        let remote = join_remote(&folder, "hello.txt");
        let handle = client.open_write(&remote, true).unwrap();
        client.write(&handle, 0, b"hello via sftp").unwrap();
        client.close(&handle).unwrap();
        client.set_mtime(&remote, 1_600_000_000).unwrap();
        assert_eq!(client.stat(&remote).unwrap().modified, Some(1_600_000_000));
        let listing = client.list(&folder).unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].size, 14);
        let handle = client.open_read(&remote).unwrap();
        assert_eq!(
            client.read(&handle, 0, 100).unwrap().unwrap(),
            b"hello via sftp"
        );
        client.close(&handle).unwrap();
        if client.supports_copy_data() {
            let copied = join_remote(&folder, "copied.txt");
            let source_handle = client.open_read(&remote).unwrap();
            let target_handle = client.open_write(&copied, true).unwrap();
            client
                .copy_data(&source_handle, &target_handle, 0, 14)
                .unwrap();
            client.close(&source_handle).unwrap();
            client.close(&target_handle).unwrap();
            assert_eq!(fs::read(&copied).unwrap(), b"hello via sftp");
            client.remove(&copied, false).unwrap();
        }
        client
            .rename(&remote, &join_remote(&folder, "renamed.txt"))
            .unwrap();
        client
            .remove(&join_remote(&folder, "renamed.txt"), false)
            .unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
