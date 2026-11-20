use crate::{
    ftp::FtpClient, profiles, s3::S3Client, sftp::SftpClient, vault, webdav::WebDavClient, AppState,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};
use tauri::State;

const MAX_TEXT_SIZE: u64 = 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocument {
    pub content: String,
    pub sha256: String,
}

fn document(bytes: Vec<u8>) -> Result<TextDocument, String> {
    if bytes.len() as u64 > MAX_TEXT_SIZE {
        return Err("This file is larger than 1 MB. Open it in an external editor instead.".into());
    }
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let content = String::from_utf8(bytes)
        .map_err(|_| "This file is not UTF-8 text. Open it in an external editor instead.")?;
    Ok(TextDocument { content, sha256 })
}

fn read_local(path: &str) -> Result<TextDocument, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("Could not inspect {path}: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(
            "Choose a regular text file. Folders and links cannot open in this editor.".into(),
        );
    }
    if metadata.len() > MAX_TEXT_SIZE {
        return Err("This file is larger than 1 MB. Open it in an external editor instead.".into());
    }
    document(fs::read(path).map_err(|error| format!("Could not read {path}: {error}"))?)
}

fn save_local(path: &str, expected: &str, content: &str) -> Result<TextDocument, String> {
    let current = read_local(path)?;
    if current.sha256 != expected {
        return Err("This file changed since you opened it. Copy your edits, reopen the file, and try again.".into());
    }
    let bytes = content.as_bytes();
    let next = document(bytes.to_vec())?;
    let original = Path::new(path);
    let temporary = original.with_file_name(format!(".weave-edit-{}", uuid::Uuid::new_v4()));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("Could not write temporary file: {error}"))?;
    let result = (|| {
        let permissions = fs::metadata(original)
            .map_err(|error| format!("Could not read file permissions: {error}"))?
            .permissions();
        fs::set_permissions(&temporary, permissions)
            .map_err(|error| format!("Could not preserve file permissions: {error}"))?;
        fs::rename(&temporary, original)
            .map_err(|error| format!("Could not replace the file: {error}"))?;
        Ok(next)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn read_remote(client: &mut SftpClient, path: &str) -> Result<TextDocument, String> {
    let attrs = client.stat(path)?;
    let kind = attrs.permissions.unwrap_or(0) & 0o170000;
    if kind == 0o120000 || kind == 0o040000 {
        return Err(
            "Choose a regular text file. Folders and links cannot open in this editor.".into(),
        );
    }
    if attrs.size > MAX_TEXT_SIZE {
        return Err("This file is larger than 1 MB. Open it in an external editor instead.".into());
    }
    let handle = client.open_read(path)?;
    let result = (|| {
        let mut bytes = Vec::with_capacity(attrs.size as usize);
        while let Some(chunk) = client.read(&handle, bytes.len() as u64, 32 * 1024)? {
            if chunk.is_empty() {
                return Err("The SFTP server stopped before finishing the file.".into());
            }
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > MAX_TEXT_SIZE {
                return Err("The remote file grew beyond the 1 MB editor limit.".into());
            }
        }
        if bytes.len() as u64 != attrs.size {
            return Err(
                "The remote file changed while it was opening. Reopen it to get the full contents."
                    .into(),
            );
        }
        document(bytes)
    })();
    let closed = client.close(&handle);
    let doc = result?;
    closed?;
    Ok(doc)
}

fn save_remote(
    client: &mut SftpClient,
    path: &str,
    expected: &str,
    content: &str,
) -> Result<TextDocument, String> {
    let current = read_remote(client, path)?;
    if current.sha256 != expected {
        return Err("This remote file changed since you opened it. Copy your edits, reopen the file, and try again.".into());
    }
    let next = document(content.as_bytes().to_vec())?;
    let canonical = client.realpath(path)?;
    let suffix = uuid::Uuid::new_v4();
    let temporary = format!("{canonical}.weave-edit-{suffix}");
    let backup = format!("{canonical}.weave-backup-{suffix}");
    let permissions = client.stat(&canonical)?.permissions.unwrap_or(0o100644) & 0o7777;
    let handle = client.open_write(&temporary, true)?;
    let written = (|| {
        for (index, chunk) in content.as_bytes().chunks(32 * 1024).enumerate() {
            client.write(&handle, (index * 32 * 1024) as u64, chunk)?;
        }
        client.close(&handle)?;
        client.chmod(&temporary, permissions)?;
        if read_remote(client, &temporary)?.sha256 != next.sha256 {
            return Err("The remote server returned different data after the save. The original file was kept.".into());
        }
        client.rename(&canonical, &backup)?;
        if let Err(error) = client.rename(&temporary, &canonical) {
            let _ = client.rename(&backup, &canonical);
            return Err(format!("Could not replace the remote file: {error}"));
        }
        client.remove(&backup, false).map_err(|error| {
            format!("File saved, but its temporary backup could not be removed: {error}")
        })?;
        Ok(next)
    })();
    if written.is_err() {
        let _ = client.close(&handle);
        let _ = client.remove(&temporary, false);
    }
    written
}

fn read_ftp(client: &mut FtpClient, path: &str) -> Result<TextDocument, String> {
    let item = client.stat(path)?;
    if item.is_dir || item.is_symlink {
        return Err(
            "Choose a regular text file. Folders and links cannot open in this editor.".into(),
        );
    }
    if item.size > MAX_TEXT_SIZE {
        return Err("This file is larger than 1 MB. Open it in an external editor instead.".into());
    }
    let mut stream = client.open_read(path, 0)?;
    let mut bytes = Vec::with_capacity(item.size as usize);
    Read::by_ref(&mut stream)
        .take(MAX_TEXT_SIZE + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read FTP file {path}: {error}"))?;
    if bytes.len() as u64 > MAX_TEXT_SIZE {
        return Err("The FTP file grew beyond the 1 MB editor limit.".into());
    }
    stream.finish()?;
    if bytes.len() as u64 != item.size {
        return Err(
            "The FTP file changed while it was opening. Reopen it to get the full contents.".into(),
        );
    }
    document(bytes)
}

fn save_ftp(
    client: &mut FtpClient,
    path: &str,
    expected: &str,
    content: &str,
) -> Result<TextDocument, String> {
    let current = read_ftp(client, path)?;
    if current.sha256 != expected {
        return Err("This FTP file changed since you opened it. Copy your edits, reopen the file, and try again.".into());
    }
    let next = document(content.as_bytes().to_vec())?;
    let suffix = uuid::Uuid::new_v4();
    let temporary = format!("{path}.weave-edit-{suffix}");
    let backup = format!("{path}.weave-backup-{suffix}");
    let result = (|| {
        let mut stream = client.open_write(&temporary, false)?;
        stream
            .write_all(content.as_bytes())
            .map_err(|error| format!("Could not upload edited FTP file: {error}"))?;
        stream.finish()?;
        if read_ftp(client, &temporary)?.sha256 != next.sha256 {
            return Err("The FTP server returned different data after the save. The original file was kept.".into());
        }
        client.rename(path, &backup)?;
        if let Err(error) = client.rename(&temporary, path) {
            let _ = client.rename(&backup, path);
            return Err(format!(
                "Could not replace the FTP file: {error}. The original file was preserved."
            ));
        }
        client.remove(&backup, false).map_err(|error| {
            format!("File saved, but its temporary backup could not be removed: {error}")
        })?;
        Ok(next)
    })();
    if result.is_err() {
        let _ = client.remove(&temporary, false);
    }
    result
}

fn read_webdav(client: &WebDavClient, path: &str) -> Result<TextDocument, String> {
    let item = client.stat(path)?;
    if item.is_dir {
        return Err("Choose a file, not a WebDAV folder.".into());
    }
    if item.size > MAX_TEXT_SIZE {
        return Err("This file is larger than 1 MB. Open it in an external editor instead.".into());
    }
    let bytes = client.read_small(path, MAX_TEXT_SIZE)?;
    if bytes.len() as u64 != item.size {
        return Err(
            "The WebDAV file changed while it was opening. Reopen it to get the full contents."
                .into(),
        );
    }
    document(bytes)
}

fn save_webdav(
    client: &WebDavClient,
    path: &str,
    expected: &str,
    content: &str,
) -> Result<TextDocument, String> {
    if read_webdav(client, path)?.sha256 != expected {
        return Err("This WebDAV file changed since you opened it. Copy your edits, reopen the file, and try again.".into());
    }
    let next = document(content.as_bytes().to_vec())?;
    let suffix = uuid::Uuid::new_v4();
    let temporary = format!("{path}.weave-edit-{suffix}");
    let backup = format!("{path}.weave-backup-{suffix}");
    let result = (|| {
        client.write_small(&temporary, content.as_bytes())?;
        if read_webdav(client, &temporary)?.sha256 != next.sha256 {
            return Err("The WebDAV server returned different data after upload. The original file was kept.".into());
        }
        client.rename(path, &backup)?;
        if let Err(error) = client.rename(&temporary, path) {
            let _ = client.rename(&backup, path);
            return Err(format!(
                "Could not replace the WebDAV file: {error}. The original file was preserved."
            ));
        }
        client.delete(&backup, false).map_err(|error| {
            format!("File saved, but its temporary backup could not be removed: {error}")
        })?;
        Ok(next)
    })();
    if result.is_err() {
        let _ = client.delete(&temporary, false);
    }
    result
}

fn read_cloud(client: &crate::cloud::CloudClient, path: &str) -> Result<TextDocument, String> {
    let item = client.stat(path)?;
    if item.is_dir {
        return Err("Choose a file, not a folder.".into());
    }
    if item.size > MAX_TEXT_SIZE {
        return Err("This file is larger than 1 MB. Open it in an external editor instead.".into());
    }
    let mut bytes = Vec::new();
    client
        .open_read(path, 0)?
        .take(MAX_TEXT_SIZE + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read {path}: {error}"))?;
    document(bytes)
}

fn read_azure(client: &crate::azure::AzureClient, path: &str) -> Result<TextDocument, String> {
    let item = client.stat(path)?;
    if item.is_dir {
        return Err("Choose a file, not a folder.".into());
    }
    if item.size > MAX_TEXT_SIZE {
        return Err("This file is larger than 1 MB. Open it in an external editor instead.".into());
    }
    let mut bytes = Vec::new();
    client
        .open_read(path, 0)?
        .take(MAX_TEXT_SIZE + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read {path}: {error}"))?;
    document(bytes)
}

/// Azure replaces a blob in one step, so there is no half-written state to guard against.
fn save_azure(
    client: &crate::azure::AzureClient,
    path: &str,
    expected: &str,
    content: &str,
) -> Result<TextDocument, String> {
    if read_azure(client, path)?.sha256 != expected {
        return Err("This file changed since you opened it. Copy your edits, reopen the file, and try again.".into());
    }
    let next = document(content.as_bytes().to_vec())?;
    client.write_small(path, content.as_bytes())?;
    Ok(next)
}

fn read_s3(client: &S3Client, path: &str) -> Result<TextDocument, String> {
    let item = client.stat(path)?;
    if item.is_dir {
        return Err("Choose a file, not an S3 folder.".into());
    }
    if item.size > MAX_TEXT_SIZE {
        return Err("This file is larger than 1 MB. Open it in an external editor instead.".into());
    }
    let mut response = client.open_read(path, 0)?;
    let mut bytes = Vec::with_capacity(item.size as usize);
    Read::by_ref(&mut response)
        .take(MAX_TEXT_SIZE + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read S3 file {path}: {error}"))?;
    if bytes.len() as u64 != item.size {
        return Err(
            "The S3 file changed while opening. Reopen it to get the full contents.".into(),
        );
    }
    document(bytes)
}

fn save_s3(
    client: &S3Client,
    path: &str,
    expected: &str,
    content: &str,
) -> Result<TextDocument, String> {
    if read_s3(client, path)?.sha256 != expected {
        return Err("This S3 file changed since you opened it. Copy your edits, reopen the file, and try again.".into());
    }
    let next = document(content.as_bytes().to_vec())?;
    let suffix = uuid::Uuid::new_v4();
    let temporary = format!("{path}.weave-edit-{suffix}");
    let backup = format!("{path}.weave-backup-{suffix}");
    let result = (|| {
        client.write_small(&temporary, content.as_bytes())?;
        if read_s3(client, &temporary)?.sha256 != next.sha256 {
            return Err(
                "The S3 server returned different data after upload. The original file was kept."
                    .into(),
            );
        }
        client.rename(path, &backup)?;
        if let Err(error) = client.rename(&temporary, path) {
            let restored = client.rename(&backup, path);
            return Err(match restored {
                Ok(()) => format!("Could not replace the S3 file: {error}. The original was restored."),
                Err(restore_error) => format!("Could not replace the S3 file: {error}. Restore the original from {backup}: {restore_error}"),
            });
        }
        client.delete(&backup, false)?;
        Ok(next)
    })();
    if result.is_err() {
        let _ = client.delete(&temporary, false);
    }
    result
}

#[tauri::command]
pub async fn editor_read(
    state: State<'_, AppState>,
    profile_id: Option<String>,
    path: String,
) -> Result<TextDocument, String> {
    let profile = profile_id
        .as_deref()
        .map(|id| profiles::get_profile(&state, id))
        .transpose()?;
    if let Some(profile) = profile
        .as_ref()
        .filter(|profile| crate::cloud::is_cloud(&profile.protocol))
    {
        let client = crate::cloud::CloudClient::connect(profile)?;
        return tauri::async_runtime::spawn_blocking(move || read_cloud(&client, &path))
            .await
            .map_err(|error| format!("The text editor stopped unexpectedly: {error}"))?;
    }
    let password = profile
        .as_ref()
        .filter(|profile| profile.protocol != "ssh")
        .map(|profile| vault::password_for(&state, profile))
        .transpose()?;
    let s3_token = profile
        .as_ref()
        .filter(|profile| profile.protocol == "s3")
        .map(|profile| vault::s3_token_for(&state, &profile.id))
        .transpose()?
        .flatten();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(profile) = profile {
            if profile.protocol == "webdav" {
                read_webdav(
                    &WebDavClient::connect(
                        &profile,
                        password.as_ref().map_or("", |value| value.as_str()),
                    )?,
                    &path,
                )
            } else if profile.protocol == "s3" {
                read_s3(
                    &S3Client::connect_with_token(
                        &profile,
                        password.as_ref().map_or("", |value| value.as_str()),
                        s3_token.as_ref().map(|value| value.as_str()),
                    )?,
                    &path,
                )
            } else if profile.protocol == "azure" {
                read_azure(
                    &crate::azure::AzureClient::connect(
                        &profile,
                        password.as_ref().map_or("", |value| value.as_str()),
                    )?,
                    &path,
                )
            } else if let Some(password) = password {
                read_ftp(&mut FtpClient::connect(&profile, &password)?, &path)
            } else {
                read_remote(&mut SftpClient::connect(&profile)?, &path)
            }
        } else {
            read_local(&path)
        }
    })
    .await
    .map_err(|error| format!("The text editor stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub async fn editor_save(
    state: State<'_, AppState>,
    profile_id: Option<String>,
    path: String,
    expected_sha256: String,
    content: String,
) -> Result<TextDocument, String> {
    let profile = profile_id
        .as_deref()
        .map(|id| profiles::get_profile(&state, id))
        .transpose()?;
    if let Some(profile) = profile
        .as_ref()
        .filter(|profile| crate::cloud::is_cloud(&profile.protocol))
    {
        let client = crate::cloud::CloudClient::connect(profile)?;
        return tauri::async_runtime::spawn_blocking(move || {
            if read_cloud(&client, &path)?.sha256 != expected_sha256 {
                return Err("This file changed since you opened it. Copy your edits, reopen the file, and try again.".into());
            }
            let next = document(content.as_bytes().to_vec())?;
            client.write_small(&path, content.as_bytes())?;
            Ok(next)
        })
        .await
        .map_err(|error| format!("The text editor stopped unexpectedly: {error}"))?;
    }
    let password = profile
        .as_ref()
        .filter(|profile| profile.protocol != "ssh")
        .map(|profile| vault::password_for(&state, profile))
        .transpose()?;
    let s3_token = profile
        .as_ref()
        .filter(|profile| profile.protocol == "s3")
        .map(|profile| vault::s3_token_for(&state, &profile.id))
        .transpose()?
        .flatten();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(profile) = profile {
            if profile.protocol == "webdav" {
                save_webdav(
                    &WebDavClient::connect(
                        &profile,
                        password.as_ref().map_or("", |value| value.as_str()),
                    )?,
                    &path,
                    &expected_sha256,
                    &content,
                )
            } else if profile.protocol == "s3" {
                save_s3(
                    &S3Client::connect_with_token(
                        &profile,
                        password.as_ref().map_or("", |value| value.as_str()),
                        s3_token.as_ref().map(|value| value.as_str()),
                    )?,
                    &path,
                    &expected_sha256,
                    &content,
                )
            } else if profile.protocol == "azure" {
                save_azure(
                    &crate::azure::AzureClient::connect(
                        &profile,
                        password.as_ref().map_or("", |value| value.as_str()),
                    )?,
                    &path,
                    &expected_sha256,
                    &content,
                )
            } else if let Some(password) = password {
                save_ftp(
                    &mut FtpClient::connect(&profile, &password)?,
                    &path,
                    &expected_sha256,
                    &content,
                )
            } else {
                save_remote(
                    &mut SftpClient::connect(&profile)?,
                    &path,
                    &expected_sha256,
                    &content,
                )
            }
        } else {
            save_local(&path, &expected_sha256, &content)
        }
    })
    .await
    .map_err(|error| format!("The text editor stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the local WebDAV test server"]
    fn webdav_editor_detects_stale_content() {
        let port = std::env::var("WEAVE_WEBDAV_TEST_PORT").expect("Set WEAVE_WEBDAV_TEST_PORT");
        let profile = profiles::Profile {
            id: "webdav-editor-test".into(),
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
        let client = WebDavClient::connect(&profile, "weave-test").unwrap();
        let path = format!("/editor-{}.txt", uuid::Uuid::new_v4());
        client.write_small(&path, b"old\n").unwrap();
        let original = read_webdav(&client, &path).unwrap();
        let saved = save_webdav(&client, &path, &original.sha256, "new\n").unwrap();
        assert_eq!(saved.content, "new\n");
        assert!(save_webdav(&client, &path, &original.sha256, "stale\n").is_err());
        assert_eq!(read_webdav(&client, &path).unwrap().content, "new\n");
        client.delete(&path, false).unwrap();
    }

    #[test]
    #[ignore = "requires the local S3 test server"]
    fn s3_integration_editor_detects_stale_content() {
        let port = std::env::var("WEAVE_S3_TEST_PORT").unwrap();
        let profile = profiles::Profile {
            id: "s3-editor-test".into(),
            name: "Local S3".into(),
            host: format!("http://127.0.0.1:{port}/weave-test"),
            protocol: "s3".into(),
            port: 443,
            username: "AKIDEXAMPLE".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: Some("us-east-1".into()),
        };
        let client = S3Client::connect(&profile, "weave-test-secret").unwrap();
        let path = format!("/editor-{}.txt", uuid::Uuid::new_v4());
        client.write_small(&path, b"old\n").unwrap();
        let original = read_s3(&client, &path).unwrap();
        let saved = save_s3(&client, &path, &original.sha256, "new\n").unwrap();
        assert_eq!(saved.content, "new\n");
        assert!(save_s3(&client, &path, &original.sha256, "stale\n").is_err());
        assert_eq!(read_s3(&client, &path).unwrap().content, "new\n");
        client.delete(&path, false).unwrap();
    }

    #[test]
    #[ignore = "requires the local FTP test server"]
    fn ftp_editor_saves_and_detects_stale_content() {
        let port = std::env::var("WEAVE_FTP_TEST_PORT")
            .expect("Set WEAVE_FTP_TEST_PORT to the local FTP test server port");
        let profile = profiles::Profile {
            id: "editor-test".into(),
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
        let path = format!("/editor-{}.txt", uuid::Uuid::new_v4());
        let mut upload = client.open_write(&path, false).unwrap();
        upload.write_all(b"old\n").unwrap();
        upload.finish().unwrap();
        let original = read_ftp(&mut client, &path).unwrap();
        let saved = save_ftp(&mut client, &path, &original.sha256, "new\n").unwrap();
        assert_eq!(saved.content, "new\n");
        assert!(save_ftp(&mut client, &path, &original.sha256, "stale\n").is_err());
        assert_eq!(read_ftp(&mut client, &path).unwrap().content, "new\n");
        client.remove(&path, false).unwrap();
    }

    #[test]
    fn local_editor_preserves_changes_and_detects_stale_content() {
        let root = std::env::temp_dir().join(format!("weave-editor-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("notes.txt");
        fs::write(&path, "old\n").unwrap();
        let name = path.to_string_lossy();
        let original = read_local(&name).unwrap();
        let saved = save_local(&name, &original.sha256, "new\n").unwrap();
        assert_eq!(saved.content, "new\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert!(save_local(&name, &original.sha256, "stale\n").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn remote_editor_saves_through_sftp_and_detects_stale_content() {
        let root = std::env::temp_dir().join(format!("weave-editor-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("notes.txt");
        fs::write(&path, "old\n").unwrap();
        let name = path.to_string_lossy();
        let mut client =
            SftpClient::from_command(crate::platform::command("/usr/libexec/sftp-server")).unwrap();
        let original = read_remote(&mut client, &name).unwrap();
        let saved = save_remote(&mut client, &name, &original.sha256, "new\n").unwrap();
        assert_eq!(saved.content, "new\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert!(save_remote(&mut client, &name, &original.sha256, "stale\n").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        fs::remove_dir_all(root).unwrap();
    }
}
