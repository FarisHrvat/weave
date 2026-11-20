//! File pane extras: find by name pattern, quick preview, symbolic links,
//! owner and group changes, and custom shell commands on selected files.

use crate::{profiles, terminal, transfer, AppState};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use tauri::{AppHandle, Manager};

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn profile(app: &AppHandle, id: &Option<String>) -> Result<Option<profiles::Profile>, String> {
    id.as_deref()
        .filter(|id| !id.is_empty())
        .map(|id| profiles::get_profile(&app.state::<AppState>(), id))
        .transpose()
}

/// Runs a shell script on this Mac or an SSH host; other connections have no shell.
fn shell(
    profile: Option<&profiles::Profile>,
    cwd: Option<&str>,
    script: &str,
) -> Result<String, String> {
    if profile.is_some_and(|profile| profile.protocol != "ssh") {
        return Err(
            "This works on this computer and on SSH hosts. FTP, WebDAV, S3 and cloud drives have no shell."
                .into(),
        );
    }
    let result = terminal::run_script(profile, cwd, script)?;
    let mut output = result.stdout;
    if !result.stderr.trim().is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&result.stderr);
    }
    match result.exit_code {
        Some(0) => Ok(output),
        code => Err(if output.trim().is_empty() {
            format!(
                "The command failed (exit code {}).",
                code.map_or("unknown".into(), |code| code.to_string())
            )
        } else {
            output.trim().lines().last().unwrap_or("").to_string()
        }),
    }
}

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| format!("The file operation stopped unexpectedly: {error}"))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindResult {
    results: Vec<transfer::FoundFile>,
    truncated: bool,
}

#[tauri::command]
pub async fn files_find(
    app: AppHandle,
    profile_id: Option<String>,
    root: String,
    mask: String,
) -> Result<FindResult, String> {
    blocking(move || {
        let (results, truncated) =
            transfer::find_files(&app.state::<AppState>(), &profile_id, &root, &mask, 500)?;
        Ok(FindResult { results, truncated })
    })
    .await
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    kind: String,
    mime: String,
    data: String,
    size: u64,
    truncated: bool,
}

pub(crate) fn mime_for(name: &str) -> (&'static str, &'static str) {
    let extension = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match extension.as_str() {
        "png" => ("image", "image/png"),
        "jpg" | "jpeg" => ("image", "image/jpeg"),
        "gif" => ("image", "image/gif"),
        "webp" => ("image", "image/webp"),
        "svg" => ("image", "image/svg+xml"),
        "bmp" => ("image", "image/bmp"),
        "ico" => ("image", "image/x-icon"),
        "pdf" => ("pdf", "application/pdf"),
        _ => ("text", "text/plain"),
    }
}

pub(crate) fn build_preview(name: &str, bytes: Vec<u8>, size: u64, limit: u64) -> Preview {
    let (kind, mime) = mime_for(name);
    let truncated = size > bytes.len() as u64;
    if kind != "text" {
        return Preview {
            kind: kind.into(),
            mime: mime.into(),
            data: if truncated {
                String::new()
            } else {
                STANDARD.encode(&bytes)
            },
            size,
            truncated: truncated || size > limit,
        };
    }
    let looks_binary = bytes.iter().take(8000).any(|byte| *byte == 0);
    Preview {
        kind: if looks_binary { "binary" } else { "text" }.into(),
        mime: mime.into(),
        data: if looks_binary {
            String::new()
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        },
        size,
        truncated,
    }
}

#[tauri::command]
pub async fn files_preview(
    app: AppHandle,
    profile_id: Option<String>,
    path: String,
) -> Result<Preview, String> {
    blocking(move || {
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        let limit = if mime_for(&name).0 == "text" {
            512 * 1024
        } else {
            20 * 1024 * 1024
        };
        let (bytes, size) =
            transfer::read_bytes(&app.state::<AppState>(), &profile_id, &path, limit)?;
        Ok(build_preview(&name, bytes, size, limit))
    })
    .await
}

#[tauri::command]
pub async fn files_symlink(
    app: AppHandle,
    profile_id: Option<String>,
    target: String,
    link: String,
) -> Result<(), String> {
    blocking(move || {
        if target.trim().is_empty()
            || link.trim().is_empty()
            || link.contains('\0')
            || target.contains('\0')
        {
            return Err("Enter what the link points to and where to create it.".into());
        }
        let profile = profile(&app, &profile_id)?;
        if profile.is_none() {
            crate::platform::symlink(std::path::Path::new(&target), std::path::Path::new(&link))
                .map_err(|error| format!("Could not create the link: {error}"))?;
            return Ok(());
        }
        shell(
            profile.as_ref(),
            None,
            &format!("ln -s -- {} {}", quote(&target), quote(&link)),
        )
        .map(|_| ())
    })
    .await
}

#[tauri::command]
pub async fn files_chown(
    app: AppHandle,
    profile_id: Option<String>,
    paths: Vec<String>,
    owner: String,
    group: String,
    recursive: bool,
) -> Result<(), String> {
    blocking(move || {
        let valid = |name: &str| name.is_empty() || name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
        let (owner, group) = (owner.trim().to_string(), group.trim().to_string());
        if (owner.is_empty() && group.is_empty()) || !valid(&owner) || !valid(&group) {
            return Err("Enter an owner, a group, or both, using letters, numbers, dots, dashes or underscores.".into());
        }
        if paths.is_empty() {
            return Err("Choose files first.".into());
        }
        let spec = match (owner.is_empty(), group.is_empty()) {
            (false, false) => format!("{owner}:{group}"),
            (false, true) => owner,
            _ => format!(":{group}"),
        };
        let list: Vec<String> = paths.iter().map(|path| quote(path)).collect();
        let flag = if recursive { "-R " } else { "" };
        shell(profile(&app, &profile_id)?.as_ref(), None, &format!("chown {flag}{spec} -- {}", list.join(" "))).map(|_| ())
    })
    .await
}

/// Fills `{path}`, `{paths}`, `{name}` and `{dir}` with shell-quoted values.
pub(crate) fn expand_command(template: &str, paths: &[String]) -> Result<String, String> {
    if template.trim().is_empty() {
        return Err("Enter a command.".into());
    }
    let first = paths.first().map(String::as_str).unwrap_or("");
    let name = first.rsplit('/').next().unwrap_or(first);
    let dir = first
        .rsplit_once('/')
        .map(|(dir, _)| if dir.is_empty() { "/" } else { dir })
        .unwrap_or(".");
    let all: Vec<String> = paths.iter().map(|path| quote(path)).collect();
    if (template.contains("{path}")
        || template.contains("{name}")
        || template.contains("{dir}")
        || template.contains("{paths}"))
        && paths.is_empty()
    {
        return Err("Select the files this command should use.".into());
    }
    Ok(template
        .replace("{paths}", &all.join(" "))
        .replace("{path}", &quote(first))
        .replace("{name}", &quote(name))
        .replace("{dir}", &quote(dir)))
}

#[tauri::command]
pub async fn files_run_command(
    app: AppHandle,
    profile_id: Option<String>,
    cwd: String,
    template: String,
    paths: Vec<String>,
) -> Result<String, String> {
    blocking(move || {
        let script = expand_command(&template, &paths)?;
        let profile = profile(&app, &profile_id)?;
        let cwd = Some(cwd.as_str()).filter(|cwd| !cwd.is_empty() && *cwd != ".");
        shell(profile.as_ref(), cwd, &script)
    })
    .await
}

fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0', '\n'])
}

/// A shell script that packs `names` (inside the current folder) into `output`.
pub(crate) fn archive_script(
    names: &[String],
    format: &str,
    output: &str,
) -> Result<String, String> {
    if names.is_empty() {
        return Err("Choose the files to compress.".into());
    }
    if !names.iter().all(|name| plain_name(name)) || !plain_name(output) {
        return Err("Choose files in this folder and a name without slashes.".into());
    }
    let (tool, command) = match format {
        "zip" => ("zip", "zip -q -r -y"),
        "tar.gz" => ("tar", "tar -czf"),
        "tar.bz2" => ("tar", "tar -cjf"),
        "tar.xz" => ("tar", "tar -cJf"),
        "tar" => ("tar", "tar -cf"),
        _ => return Err("Choose zip, tar.gz, tar.bz2, tar.xz or tar.".into()),
    };
    let inputs: Vec<String> = names
        .iter()
        .map(|name| quote(&format!("./{name}")))
        .collect();
    let output = quote(&format!("./{output}"));
    Ok(format!(
        "command -v {tool} >/dev/null 2>&1 || {{ echo '{tool} is not installed here. Choose another format.' >&2; exit 3; }}\n\
         [ -e {output} ] && {{ echo 'A file with that name already exists.' >&2; exit 4; }}\n\
         {command} {output} {}",
        inputs.join(" ")
    ))
}

/// A shell script that unpacks an archive into a new folder next to it,
/// named after the archive, so nothing existing is overwritten.
pub(crate) fn extract_script(name: &str) -> Result<String, String> {
    if !plain_name(name) {
        return Err("Choose an archive in this folder.".into());
    }
    let lower = name.to_lowercase();
    let (stem_length, command) = [
        (".tar.gz", "tar -xzf"),
        (".tgz", "tar -xzf"),
        (".tar.bz2", "tar -xjf"),
        (".tbz2", "tar -xjf"),
        (".tar.xz", "tar -xJf"),
        (".txz", "tar -xJf"),
        (".tar", "tar -xf"),
        (".zip", "unzip -q"),
    ]
    .iter()
    .find(|(suffix, _)| lower.ends_with(suffix))
    .map(|(suffix, command)| (name.len() - suffix.len(), *command))
    .ok_or("Weave can extract zip, tar, tar.gz, tar.bz2 and tar.xz archives.")?;
    let stem = if stem_length == 0 {
        "archive"
    } else {
        &name[..stem_length]
    };
    let tool = command.split(' ').next().unwrap_or("tar");
    let archive = quote(&format!("./{name}"));
    let into = if tool == "unzip" { "-d" } else { "-C" };
    Ok(format!(
        "command -v {tool} >/dev/null 2>&1 || {{ echo '{tool} is not installed here.' >&2; exit 3; }}\n\
         target={}; n=2; while [ -e \"$target\" ]; do target={}\" $n\"; n=$((n+1)); done\n\
         mkdir -- \"$target\" && {command} {archive} {into} \"$target\" && printf '%s' \"$target\"",
        quote(&format!("./{stem}")),
        quote(&format!("./{stem}")),
    ))
}

#[tauri::command]
pub async fn files_archive(
    app: AppHandle,
    profile_id: Option<String>,
    cwd: String,
    names: Vec<String>,
    format: String,
    output: String,
) -> Result<(), String> {
    blocking(move || {
        let script = archive_script(&names, &format, output.trim())?;
        shell(profile(&app, &profile_id)?.as_ref(), Some(&cwd), &script).map(|_| ())
    })
    .await
}

/// Returns the name of the folder the archive was extracted into.
#[tauri::command]
pub async fn files_extract(
    app: AppHandle,
    profile_id: Option<String>,
    cwd: String,
    name: String,
) -> Result<String, String> {
    blocking(move || {
        let script = extract_script(&name)?;
        let folder = shell(profile(&app, &profile_id)?.as_ref(), Some(&cwd), &script)?;
        Ok(folder.trim().trim_start_matches("./").to_string())
    })
    .await
}

/// Opens a local file in its default app.
#[tauri::command]
pub fn open_local_file(path: String) -> Result<(), String> {
    if !std::path::Path::new(&path).is_file() {
        return Err(format!(
            "{path} is not a file on {}.",
            crate::platform::this_computer()
        ));
    }
    crate::platform::open_path(std::path::Path::new(&path))
}

/// "folder", "file" or "missing", for opening paths clicked in a terminal.
#[tauri::command]
pub fn local_path_kind(path: String) -> &'static str {
    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_dir() => "folder",
        Ok(_) => "file",
        Err(_) => "missing",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_placeholders_with_quoting() {
        let paths = vec!["/srv/my file.txt".to_string(), "/srv/it's.log".to_string()];
        assert_eq!(
            expand_command("gzip -k {paths}", &paths).unwrap(),
            "gzip -k '/srv/my file.txt' '/srv/it'\\''s.log'"
        );
        assert_eq!(
            expand_command("cd {dir} && ls -l {name}", &paths).unwrap(),
            "cd '/srv' && ls -l 'my file.txt'"
        );
        assert!(expand_command("ls {path}", &[]).is_err());
        assert_eq!(expand_command("uptime", &[]).unwrap(), "uptime");
    }

    #[test]
    fn runs_local_commands_and_reports_failures() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a b.txt"), "x").unwrap();
        let path = dir.path().join("a b.txt").to_string_lossy().to_string();
        let script = expand_command("wc -c < {path}", &[path]).unwrap();
        assert_eq!(shell(None, None, &script).unwrap().trim(), "1");
        assert!(shell(None, None, "echo nope >&2; exit 3")
            .unwrap_err()
            .contains("nope"));
    }

    #[test]
    fn compresses_and_extracts_every_format() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        std::fs::create_dir(dir.path().join("my folder")).unwrap();
        std::fs::write(dir.path().join("my folder/it's.txt"), "inside").unwrap();
        std::fs::write(dir.path().join("-top.txt"), "top").unwrap();
        let names = vec!["my folder".to_string(), "-top.txt".to_string()];
        for (format, archive) in [
            ("zip", "out.zip"),
            ("tar.gz", "out.tar.gz"),
            ("tar.bz2", "out.tar.bz2"),
            ("tar.xz", "out.tar.xz"),
            ("tar", "out.tar"),
        ] {
            shell(
                None,
                Some(cwd),
                &archive_script(&names, format, archive).unwrap(),
            )
            .unwrap();
            assert!(shell(
                None,
                Some(cwd),
                &archive_script(&names, format, archive).unwrap()
            )
            .unwrap_err()
            .contains("already exists"));
            let folder = shell(None, Some(cwd), &extract_script(archive).unwrap()).unwrap();
            assert_eq!(folder, "./out");
            let root = dir.path().join("out");
            assert_eq!(
                std::fs::read_to_string(root.join("my folder/it's.txt")).unwrap(),
                "inside"
            );
            assert_eq!(
                std::fs::read_to_string(root.join("-top.txt")).unwrap(),
                "top"
            );
            std::fs::remove_dir_all(root).unwrap();
        }
        std::fs::create_dir(dir.path().join("out")).unwrap();
        let folder = shell(None, Some(cwd), &extract_script("out.zip").unwrap()).unwrap();
        assert_eq!(folder, "./out 2");
        assert!(archive_script(&["../x".into()], "zip", "a.zip").is_err());
        assert!(archive_script(&names, "rar", "a.rar").is_err());
        assert!(extract_script("notes.txt").is_err());
    }

    #[test]
    fn builds_previews_by_type() {
        let image = build_preview("photo.PNG", vec![1, 2, 3], 3, 100);
        assert_eq!(
            (image.kind.as_str(), image.data.as_str()),
            ("image", "AQID")
        );
        let text = build_preview("notes.md", b"# Hi".to_vec(), 10, 100);
        assert_eq!((text.kind.as_str(), text.truncated), ("text", true));
        let binary = build_preview("blob.bin", vec![0, 1, 2], 3, 100);
        assert_eq!(binary.kind, "binary");
        let big = build_preview("scan.pdf", vec![1], 500, 100);
        assert!(big.truncated && big.data.is_empty());
    }

    #[test]
    fn tells_folders_from_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        assert_eq!(
            local_path_kind(dir.path().to_string_lossy().into()),
            "folder"
        );
        assert_eq!(
            local_path_kind(dir.path().join("a.txt").to_string_lossy().into()),
            "file"
        );
        assert_eq!(
            local_path_kind(dir.path().join("nope").to_string_lossy().into()),
            "missing"
        );
    }
}
