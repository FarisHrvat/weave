//! File side of ZMODEM (`sz`/`rz`) in the terminal: received files go to
//! Downloads without replacing anything, and chosen files are read to send.

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Keeps only the file name a remote host offered, so it cannot point elsewhere.
fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || c == ':' { '_' } else { c })
        .collect();
    match cleaned.as_str() {
        "" | "." | ".." => "received-file".into(),
        _ => cleaned,
    }
}

fn free_path(folder: &Path, name: &str) -> PathBuf {
    let candidate = folder.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| folder.join(format!("{stem} {n}{extension}")))
        .find(|path| !path.exists())
        .expect("some name is free")
}

pub(crate) fn save_into(folder: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    fs::create_dir_all(folder)
        .map_err(|error| format!("Could not open {}: {error}", folder.display()))?;
    let path = free_path(folder, &safe_name(name));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Could not save {}: {error}", path.display()))?;
    file.write_all(bytes)
        .map_err(|error| format!("Could not save {}: {error}", path.display()))?;
    Ok(path)
}

#[tauri::command]
pub fn zmodem_save(name: String, data: String) -> Result<String, String> {
    let bytes = STANDARD
        .decode(data)
        .map_err(|_| "The received file arrived damaged.")?;
    let folder = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        .ok_or("Could not find your Downloads folder.")?;
    Ok(save_into(&folder, &name, &bytes)?
        .to_string_lossy()
        .into_owned())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZmodemFile {
    name: String,
    data: String,
    modified: Option<u64>,
}

#[tauri::command]
pub fn zmodem_read(path: String) -> Result<ZmodemFile, String> {
    let metadata =
        fs::metadata(&path).map_err(|error| format!("Could not open {path}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{path} is not a file. Send folders as an archive."));
    }
    if metadata.len() > MAX_BYTES {
        return Err(format!(
            "{path} is larger than 512 MB. Use Files (SFTP) for big files."
        ));
    }
    let bytes = fs::read(&path).map_err(|error| format!("Could not read {path}: {error}"))?;
    Ok(ZmodemFile {
        name: Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into()),
        data: STANDARD.encode(bytes),
        modified: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_received_names_inside_the_folder_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("C:\\temp\\a.txt"), "a.txt");
        assert_eq!(safe_name(".."), "received-file");
        assert_eq!(safe_name("bad\u{7}name:x"), "bad_name_x");
        let first = save_into(dir.path(), "../report.txt", b"one").unwrap();
        let second = save_into(dir.path(), "report.txt", b"two").unwrap();
        let third = save_into(dir.path(), "Makefile", b"x").unwrap();
        let fourth = save_into(dir.path(), "Makefile", b"y").unwrap();
        assert_eq!(first, dir.path().join("report.txt"));
        assert_eq!(second, dir.path().join("report 2.txt"));
        assert_eq!(fourth, dir.path().join("Makefile 2"));
        assert_eq!(fs::read(&first).unwrap(), b"one");
        assert_eq!(fs::read(&third).unwrap(), b"x");
    }

    #[test]
    fn reads_files_to_send_and_refuses_folders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a b.bin");
        fs::write(&path, [0_u8, 255, 7]).unwrap();
        let file = zmodem_read(path.to_string_lossy().into()).unwrap();
        assert_eq!(file.name, "a b.bin");
        assert_eq!(STANDARD.decode(file.data).unwrap(), vec![0, 255, 7]);
        assert!(file.modified.is_some());
        assert!(zmodem_read(dir.path().to_string_lossy().into())
            .err()
            .unwrap()
            .contains("archive"));
    }
}
