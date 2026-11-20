use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<u64>,
    pub readonly: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileListing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<FileEntry>,
}

#[tauri::command]
pub fn files_home() -> String {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/"))
        .to_string_lossy()
        .to_string()
}

#[tauri::command]
pub fn files_list(path: String) -> Result<FileListing, String> {
    let directory =
        fs::canonicalize(&path).map_err(|error| format!("Cannot open {path}: {error}"))?;
    if !directory.is_dir() {
        return Err(format!("{} is not a folder.", directory.display()));
    }
    let mut entries = Vec::new();
    for result in fs::read_dir(&directory)
        .map_err(|error| format!("Cannot list {}: {error}", directory.display()))?
    {
        let item = result.map_err(|error| format!("Cannot read a folder entry: {error}"))?;
        let metadata = fs::symlink_metadata(item.path())
            .map_err(|error| format!("Cannot read {}: {error}", item.path().display()))?;
        let is_symlink = metadata.file_type().is_symlink();
        let is_dir = if is_symlink {
            item.path().is_dir()
        } else {
            metadata.is_dir()
        };
        entries.push(FileEntry {
            name: item.file_name().to_string_lossy().to_string(),
            path: item.path().to_string_lossy().to_string(),
            is_dir,
            is_symlink,
            size: metadata.len(),
            modified: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs()),
            readonly: metadata.permissions().readonly(),
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(FileListing {
        path: directory.to_string_lossy().to_string(),
        parent: directory
            .parent()
            .map(|path| path.to_string_lossy().to_string()),
        entries,
    })
}

#[tauri::command]
pub fn files_mkdir(parent: String, name: String) -> Result<(), String> {
    let child = safe_child(&parent, &name)?;
    fs::create_dir(&child).map_err(|error| format!("Could not create {}: {error}", child.display()))
}

#[tauri::command]
pub fn files_rename(path: String, name: String) -> Result<(), String> {
    let source = Path::new(&path);
    if fs::symlink_metadata(source)
        .map_err(|error| format!("Cannot inspect {path}: {error}"))?
        .is_dir()
        && protected_directory(source)?
    {
        return Err(
            "The filesystem root and your home folder cannot be renamed from Files.".into(),
        );
    }
    let parent = source.parent().ok_or("Cannot rename the root folder.")?;
    let target = safe_child(&parent.to_string_lossy(), &name)?;
    if occupied(&target)? {
        return Err(format!(
            "{} already exists. Choose a different name.",
            target.display()
        ));
    }
    fs::rename(source, &target)
        .map_err(|error| format!("Could not rename {}: {error}", source.display()))
}

#[tauri::command]
pub fn files_delete(path: String) -> Result<(), String> {
    let target = Path::new(&path);
    let metadata =
        fs::symlink_metadata(target).map_err(|error| format!("Cannot inspect {path}: {error}"))?;
    if metadata.is_dir() && protected_directory(target)? {
        return Err("The filesystem root and your home folder cannot be deleted from Files. Choose a folder inside them.".into());
    }
    let result = if metadata.is_dir() {
        fs::remove_dir_all(target)
    } else {
        fs::remove_file(target)
    };
    result.map_err(|error| format!("Could not delete {path}: {error}"))
}

#[tauri::command]
pub async fn files_transfer(
    source: String,
    destination_dir: String,
    conflict: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        transfer(Path::new(&source), Path::new(&destination_dir), &conflict)
    })
    .await
    .map_err(|error| format!("Transfer stopped unexpectedly: {error}"))?
    .map(|path| path.to_string_lossy().to_string())
}

fn safe_child(parent: &str, name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err("Enter a single file or folder name without slashes.".into());
    }
    Ok(Path::new(parent).join(name))
}

fn protected_directory(path: &Path) -> Result<bool, String> {
    let directory = fs::canonicalize(path)
        .map_err(|error| format!("Cannot inspect folder {}: {error}", path.display()))?;
    let home = dirs::home_dir().and_then(|home| fs::canonicalize(home).ok());
    Ok(directory.parent().is_none() || home.is_some_and(|home| home == directory))
}

fn occupied(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Cannot inspect {}: {error}", path.display())),
    }
}

fn transfer(source: &Path, destination_dir: &Path, conflict: &str) -> Result<PathBuf, String> {
    let source =
        fs::canonicalize(source).map_err(|error| format!("Cannot find source: {error}"))?;
    let destination_dir = fs::canonicalize(destination_dir)
        .map_err(|error| format!("Cannot open destination: {error}"))?;
    if !destination_dir.is_dir() {
        return Err("Transfer destination must be a folder.".into());
    }
    let name = source.file_name().ok_or("Cannot copy a root folder.")?;
    let mut target = destination_dir.join(name);
    if source.is_dir() && destination_dir.starts_with(&source) {
        return Err("Cannot copy a folder into itself or one of its subfolders.".into());
    }
    if target == source {
        return Err("Source and destination are the same location.".into());
    }
    if occupied(&target)? {
        match conflict {
            "skip" => return Ok(target),
            "rename" => {
                let stem = Path::new(name)
                    .file_stem()
                    .unwrap_or(name)
                    .to_string_lossy();
                let extension = Path::new(name)
                    .extension()
                    .map(|value| format!(".{}", value.to_string_lossy()))
                    .unwrap_or_default();
                let mut index = 2;
                loop {
                    let candidate = destination_dir.join(format!("{stem} ({index}){extension}"));
                    if !occupied(&candidate)? {
                        target = candidate;
                        break;
                    }
                    index += 1;
                }
            }
            "overwrite" => {}
            _ => return Err("Choose overwrite, skip, or rename for existing files.".into()),
        }
    }
    let temporary = destination_dir.join(format!(".weave-copy-{}", uuid::Uuid::new_v4()));
    if let Err(error) = copy_tree(&source, &temporary) {
        let _ = remove_path(&temporary);
        return Err(error);
    }
    if occupied(&target)? {
        let backup = destination_dir.join(format!(".weave-backup-{}", uuid::Uuid::new_v4()));
        fs::rename(&target, &backup)
            .map_err(|error| format!("Cannot replace {}: {error}", target.display()))?;
        if let Err(error) = fs::rename(&temporary, &target) {
            let _ = fs::rename(&backup, &target);
            let _ = remove_path(&temporary);
            return Err(format!(
                "Cannot finish copy to {}: {error}",
                target.display()
            ));
        }
        remove_path(&backup).map_err(|error| {
            format!(
                "Copy finished, but could not remove the old version at {}: {error}",
                backup.display()
            )
        })?;
    } else {
        fs::rename(&temporary, &target)
            .map_err(|error| format!("Cannot finish copy to {}: {error}", target.display()))?;
    }
    Ok(target)
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("Cannot inspect {}: {error}", source.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "{} is a symbolic link. Copying links is not supported yet.",
            source.display()
        ));
    }
    if metadata.is_dir() {
        fs::create_dir(target)
            .map_err(|error| format!("Cannot create {}: {error}", target.display()))?;
        for item in fs::read_dir(source)
            .map_err(|error| format!("Cannot read {}: {error}", source.display()))?
        {
            let item = item.map_err(|error| format!("Cannot read folder entry: {error}"))?;
            copy_tree(&item.path(), &target.join(item.file_name()))?;
        }
    } else {
        fs::copy(source, target)
            .map_err(|error| format!("Cannot copy {}: {error}", source.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_copy_into_own_subfolder() {
        let root = std::env::temp_dir().join(format!("weave-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("source/child")).unwrap();
        let result = transfer(&root.join("source"), &root.join("source/child"), "rename");
        assert!(result.unwrap_err().contains("into itself"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_delete_root_or_home_folder() {
        assert!(files_delete("/..".into())
            .unwrap_err()
            .contains("cannot be deleted"));
        if let Some(home) = dirs::home_dir() {
            assert!(files_delete(home.to_string_lossy().to_string())
                .unwrap_err()
                .contains("cannot be deleted"));
            assert!(
                files_rename(home.to_string_lossy().to_string(), "new-home".into())
                    .unwrap_err()
                    .contains("cannot be renamed")
            );
            #[cfg(unix)]
            {
                let link =
                    std::env::temp_dir().join(format!("weave-home-link-{}", uuid::Uuid::new_v4()));
                std::os::unix::fs::symlink(&home, &link).unwrap();
                files_delete(link.to_string_lossy().to_string()).unwrap();
                assert!(fs::symlink_metadata(link).is_err());
            }
        }
    }

    #[test]
    fn keeps_both_files_on_conflict() {
        let root = std::env::temp_dir().join(format!("weave-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(root.join("a/note.txt"), "new").unwrap();
        fs::write(root.join("b/note.txt"), "old").unwrap();
        let target = transfer(&root.join("a/note.txt"), &root.join("b"), "rename").unwrap();
        assert_eq!(fs::read_to_string(target).unwrap(), "new");
        assert_eq!(fs::read_to_string(root.join("b/note.txt")).unwrap(), "old");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_overwrite_preserves_existing_file() {
        let root = std::env::temp_dir().join(format!("weave-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        std::os::unix::fs::symlink("missing", root.join("a/note.txt")).unwrap();
        fs::write(root.join("b/note.txt"), "keep me").unwrap();
        assert!(transfer(&root.join("a/note.txt"), &root.join("b"), "overwrite").is_err());
        assert_eq!(
            fs::read_to_string(root.join("b/note.txt")).unwrap(),
            "keep me"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn broken_links_still_count_as_occupied_names() {
        let root = std::env::temp_dir().join(format!("weave-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("source")).unwrap();
        fs::create_dir_all(root.join("destination")).unwrap();
        fs::write(root.join("source/note.txt"), "new").unwrap();
        fs::write(root.join("destination/other.txt"), "other").unwrap();
        std::os::unix::fs::symlink("missing", root.join("destination/note.txt")).unwrap();

        assert!(files_rename(
            root.join("destination/other.txt")
                .to_string_lossy()
                .to_string(),
            "note.txt".into()
        )
        .unwrap_err()
        .contains("already exists"));
        let copy = transfer(
            &root.join("source/note.txt"),
            &root.join("destination"),
            "rename",
        )
        .unwrap();
        assert_eq!(copy.file_name().unwrap(), "note (2).txt");
        assert_eq!(fs::read_to_string(copy).unwrap(), "new");
        assert!(fs::symlink_metadata(root.join("destination/note.txt"))
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_dir_all(root).unwrap();
    }
}
