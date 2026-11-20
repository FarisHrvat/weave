//! SMB (Windows/NAS), NFS and WebDAV shares through the system's own
//! mounting (macOS, File Explorer, or the Linux desktop's GVFS), so passwords
//! stay with the system and the share appears as a folder.

use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MountedShare {
    pub kind: String,
    pub source: String,
    pub path: String,
}

fn valid_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.contains(['/', '\\', '@', ':', '?', '#', '\0'])
        && !value.chars().any(char::is_control)
}

/// Builds the URL Finder mounts, for example smb://ada@nas.local/Photos.
pub(crate) fn share_url(
    kind: &str,
    server: &str,
    share: &str,
    user: &str,
) -> Result<String, String> {
    let server = server.trim();
    if !crate::remote_access::valid_host(server) {
        return Err("Enter the server name or IP address, such as nas.local.".into());
    }
    let share = share.trim().trim_matches('/');
    let encode = |text: &str| {
        percent_encoding::utf8_percent_encode(text, percent_encoding::NON_ALPHANUMERIC).to_string()
    };
    match kind {
        "smb" => {
            if !valid_part(share) {
                return Err("Enter the share name, such as Photos.".into());
            }
            let user = user.trim();
            if !user.is_empty() && !valid_part(&user.replace(';', "")) {
                return Err("The user name contains characters SMB does not allow.".into());
            }
            let login = if user.is_empty() {
                String::new()
            } else {
                format!("{}@", encode(user))
            };
            Ok(format!("smb://{login}{server}/{}", encode(share)))
        }
        "nfs" => {
            if share.is_empty() || share.contains("..") || share.chars().any(char::is_control) {
                return Err("Enter the exported folder, such as /volume1/data.".into());
            }
            let path: Vec<String> = share
                .split('/')
                .filter(|part| !part.is_empty())
                .map(encode)
                .collect();
            Ok(format!("nfs://{server}/{}", path.join("/")))
        }
        _ => Err("Choose SMB or NFS.".into()),
    }
}

/// Network file systems from `mount` output.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_mounts(output: &str) -> Vec<MountedShare> {
    output
        .lines()
        .filter_map(|line| {
            let (source, rest) = line.split_once(" on ")?;
            let (path, details) = rest.rsplit_once(" (")?;
            let kind = details.split([',', ')']).next()?.trim();
            let kind = match kind {
                "smbfs" => "smb",
                "afpfs" => "afp",
                "nfs" => "nfs",
                "webdav" => "webdav",
                "ftp" => "ftp",
                _ => return None,
            };
            Some(MountedShare {
                kind: kind.to_string(),
                source: source.to_string(),
                path: path.to_string(),
            })
        })
        .collect()
}

/// How each system mounts a share address and lists what is mounted. The
/// system asks for the password itself; Weave never passes it along.
#[cfg(target_os = "macos")]
mod system {
    use super::{parse_mounts, MountedShare};
    use std::path::Path;

    pub fn mounted() -> Result<Vec<MountedShare>, String> {
        let output = crate::platform::command("/sbin/mount")
            .output()
            .map_err(|error| format!("Could not list mounted shares: {error}"))?;
        Ok(parse_mounts(&String::from_utf8_lossy(&output.stdout)))
    }

    /// SMB and NFS go through `open`, which shows macOS's own sign-in;
    /// WebDAV and FTP through Finder's "mount volume".
    pub fn request(url: &str) -> Result<(), String> {
        let status = if url.starts_with("smb://") || url.starts_with("nfs://") {
            crate::platform::command("/usr/bin/open").arg(url).status()
        } else {
            crate::platform::command("/usr/bin/osascript")
                .args([
                    "-e",
                    "on run argv",
                    "-e",
                    "mount volume (item 1 of argv)",
                    "-e",
                    "end run",
                    url,
                ])
                .status()
        }
        .map_err(|error| format!("Could not ask macOS to connect: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("macOS could not connect. Check the address and sign-in, then try again.".into())
        }
    }

    pub fn expected_path(_url: &str) -> Option<String> {
        None
    }

    pub fn eject(path: &str) -> Result<(), String> {
        if !Path::new(path).starts_with("/Volumes") {
            return Err("That folder is not a mounted network share.".into());
        }
        let output = crate::platform::command("/usr/sbin/diskutil")
            .args(["unmount", path])
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err("The share is in use. Close files on it, then eject again.".into())
        }
    }
}

#[cfg(windows)]
mod system {
    use super::MountedShare;

    /// Network connections from `net use`.
    pub(crate) fn parse_net_use(output: &str) -> Vec<MountedShare> {
        output
            .lines()
            .filter_map(|line| {
                let words: Vec<&str> = line.split_whitespace().collect();
                let source_index = words.iter().position(|word| word.starts_with(r"\\"))?;
                let source = words[source_index].to_string();
                let drive = words[..source_index]
                    .iter()
                    .find(|word| word.len() == 2 && word.ends_with(':'))
                    .map(|drive| format!("{drive}\\"));
                let kind = if line.contains("Web Client") {
                    "webdav"
                } else if line.contains("NFS") {
                    "nfs"
                } else {
                    "smb"
                };
                Some(MountedShare {
                    kind: kind.into(),
                    path: drive.unwrap_or_else(|| source.clone()),
                    source,
                })
            })
            .collect()
    }

    pub fn mounted() -> Result<Vec<MountedShare>, String> {
        let output = crate::platform::command("net")
            .arg("use")
            .output()
            .map_err(|error| format!("Could not list network drives: {error}"))?;
        Ok(parse_net_use(&String::from_utf8_lossy(&output.stdout)))
    }

    /// The folder path Windows uses for a share address:
    /// smb://nas/Photos is \\nas\Photos and https://dav.example.com/files is
    /// \\dav.example.com@SSL\DavWWWRoot\files.
    pub(crate) fn unc_path(url: &str) -> Result<String, String> {
        let parsed = url::Url::parse(url).map_err(|_| "This share address is not valid.")?;
        let host = parsed
            .host_str()
            .ok_or("This share address has no server.")?;
        let decode = |part: &str| {
            percent_encoding::percent_decode_str(part)
                .decode_utf8_lossy()
                .into_owned()
        };
        let path: Vec<String> = parsed
            .path_segments()
            .map(|parts| parts.filter(|part| !part.is_empty()).map(decode).collect())
            .unwrap_or_default();
        let server = match parsed.scheme() {
            "smb" | "nfs" => host.to_string(),
            "https" => match parsed.port() {
                Some(port) => format!("{host}@SSL@{port}"),
                None => format!("{host}@SSL"),
            },
            "http" => match parsed.port() {
                Some(port) => format!("{host}@{port}"),
                None => host.to_string(),
            },
            _ => return Err("Windows can open SMB, NFS and WebDAV shares as folders. Use Files in Weave for FTP.".into()),
        };
        let mut unc = format!(r"\\{server}");
        if parsed.scheme().starts_with("http") {
            unc.push_str(r"\DavWWWRoot");
        }
        for part in path {
            unc.push('\\');
            unc.push_str(&part);
        }
        Ok(unc)
    }

    /// Opens the share in File Explorer, which asks for the sign-in.
    pub fn request(url: &str) -> Result<(), String> {
        let unc = unc_path(url)?;
        crate::platform::command("explorer.exe")
            .arg(&unc)
            .spawn()
            .map_err(|error| format!("Could not ask Windows to connect: {error}"))?;
        Ok(())
    }

    pub fn expected_path(url: &str) -> Option<String> {
        unc_path(url).ok()
    }

    pub fn eject(path: &str) -> Result<(), String> {
        let output = crate::platform::command("net")
            .args(["use", path.trim_end_matches('\\'), "/delete", "/y"])
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err("The share is in use. Close files on it, then disconnect again.".into())
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod system {
    use super::MountedShare;
    use std::path::{Path, PathBuf};

    fn gvfs_dir() -> PathBuf {
        PathBuf::from(format!("/run/user/{}/gvfs", unsafe { libc::getuid() }))
    }

    /// Shares mounted by the desktop (GVFS), from their folder names such as
    /// `smb-share:server=nas,share=photos`.
    pub(crate) fn parse_gvfs(names: &[String], base: &Path) -> Vec<MountedShare> {
        names
            .iter()
            .filter_map(|name| {
                let (kind, details) = name.split_once(':')?;
                let kind = match kind {
                    "smb-share" => "smb",
                    "nfs" => "nfs",
                    "dav" | "davs" => "webdav",
                    "ftp" | "ftps" => "ftp",
                    "sftp" => "sftp",
                    _ => return None,
                };
                let field = |key: &str| {
                    details
                        .split(',')
                        .find_map(|part| part.strip_prefix(&format!("{key}=")))
                };
                let server = field("host").or_else(|| field("server")).unwrap_or("");
                let share = field("share").or_else(|| field("prefix")).unwrap_or("");
                Some(MountedShare {
                    kind: kind.into(),
                    source: format!("{server}/{}", share.trim_start_matches('/')),
                    path: base.join(name).to_string_lossy().into_owned(),
                })
            })
            .collect()
    }

    /// Network file systems the kernel mounted, from /proc/mounts.
    pub(crate) fn parse_proc_mounts(text: &str) -> Vec<MountedShare> {
        text.lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let (source, path, kind) = (fields.next()?, fields.next()?, fields.next()?);
                let kind = match kind {
                    "cifs" | "smb3" => "smb",
                    "nfs" | "nfs4" => "nfs",
                    "davfs" => "webdav",
                    _ => return None,
                };
                let unescape = |text: &str| text.replace("\\040", " ");
                Some(MountedShare {
                    kind: kind.into(),
                    source: unescape(source),
                    path: unescape(path),
                })
            })
            .collect()
    }

    pub fn mounted() -> Result<Vec<MountedShare>, String> {
        let base = gvfs_dir();
        let names: Vec<String> = std::fs::read_dir(&base)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let mut shares = parse_gvfs(&names, &base);
        if let Ok(text) = std::fs::read_to_string("/proc/mounts") {
            shares.extend(parse_proc_mounts(&text));
        }
        Ok(shares)
    }

    /// GVFS names WebDAV dav:// and davs://.
    pub(crate) fn gio_url(url: &str) -> String {
        if let Some(rest) = url.strip_prefix("https://") {
            format!("davs://{rest}")
        } else if let Some(rest) = url.strip_prefix("http://") {
            format!("dav://{rest}")
        } else {
            url.to_string()
        }
    }

    /// Opens the share in the file manager, which shows the desktop's own
    /// sign-in and mounts it through GVFS.
    pub fn request(url: &str) -> Result<(), String> {
        let gio = crate::platform::find_tool("gio")
            .ok_or("Connecting to shares needs GVFS (the gio tool), which GNOME, KDE and most desktops include.")?;
        crate::platform::command(gio)
            .args(["open", &gio_url(url)])
            .spawn()
            .map_err(|error| format!("Could not ask the desktop to connect: {error}"))?;
        Ok(())
    }

    pub fn expected_path(_url: &str) -> Option<String> {
        None
    }

    pub fn eject(path: &str) -> Result<(), String> {
        let output = if Path::new(path).starts_with(gvfs_dir()) {
            let gio = crate::platform::find_tool("gio").ok_or("The gio tool is missing.")?;
            crate::platform::command(gio)
                .args(["mount", "-u", path])
                .output()
        } else {
            crate::platform::command("umount").arg(path).output()
        }
        .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err("The share is in use, or needs an administrator to unmount. Close files on it, then try again.".into())
        }
    }
}

use system::mounted;

/// Asks the system to mount `url` and waits up to two minutes for the new
/// folder to appear.
fn connect_and_wait(url: &str) -> Result<String, String> {
    let before: Vec<String> = mounted()?.into_iter().map(|item| item.path).collect();
    system::request(url)?;
    let expected = system::expected_path(url);
    for _ in 0..240 {
        std::thread::sleep(Duration::from_millis(500));
        if let Some(path) = expected
            .as_ref()
            .filter(|path| std::path::Path::new(path).is_dir())
        {
            return Ok(path.clone());
        }
        if let Some(share) = mounted()?
            .into_iter()
            .find(|item| !before.contains(&item.path))
        {
            return Ok(share.path);
        }
    }
    Err("The share did not appear within two minutes. Finish signing in, or check the server and share name.".into())
}

#[tauri::command]
pub fn shares_mounted() -> Result<Vec<MountedShare>, String> {
    mounted()
}

/// Asks the system to mount the share (it may show its own sign-in window)
/// and waits up to two minutes for the folder to appear.
#[tauri::command]
pub async fn shares_connect(
    kind: String,
    server: String,
    share: String,
    user: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let url = share_url(&kind, &server, &share, &user)?;
        connect_and_wait(&url)
    })
    .await
    .map_err(|error| format!("Connecting stopped unexpectedly: {error}"))?
}

/// The address Finder mounts for a saved WebDAV or FTP connection. Finder
/// mounts FTP read-only and has no FTPS, SFTP or S3 support of its own.
pub(crate) fn finder_url(profile: &crate::profiles::Profile) -> Result<String, String> {
    match profile.protocol.as_str() {
        "webdav" => {
            crate::webdav::validate_url(&profile.host)?;
            let mut url = url::Url::parse(&profile.host)
                .map_err(|_| "This WebDAV address is not valid.")?;
            if !profile.username.is_empty() {
                url.set_username(&profile.username)
                    .map_err(|_| "This user name cannot be used in an address.")?;
            }
            url.set_password(None).ok();
            Ok(url.to_string())
        }
        "ftp" => {
            if !crate::remote_access::valid_host(&profile.host) {
                return Err("This FTP host name is not valid.".into());
            }
            let user = percent_encoding::utf8_percent_encode(
                &profile.username,
                percent_encoding::NON_ALPHANUMERIC,
            );
            Ok(format!("ftp://{user}@{}:{}/", profile.host, profile.port))
        }
        "ftps" | "ftps-implicit" => Err(format!(
            "{} cannot mount FTPS. Use Files in Weave, or WebDAV or SMB for a drive.",
            crate::platform::file_manager_name()
        )),
        _ => Err(format!(
            "{} can mount WebDAV and FTP connections. For SFTP, S3, Azure and cloud drives, use Files in Weave.",
            crate::platform::file_manager_name()
        )),
    }
}

/// Mounts a saved connection as a Finder drive. macOS asks for the password
/// itself and can keep it in the Keychain; Weave never passes it along.
#[tauri::command]
pub async fn profile_mount_in_finder(
    state: tauri::State<'_, crate::AppState>,
    profile_id: String,
) -> Result<String, String> {
    let profile = crate::profiles::get_profile(&state, &profile_id)?;
    let url = finder_url(&profile)?;
    tauri::async_runtime::spawn_blocking(move || connect_and_wait(&url))
        .await
        .map_err(|error| format!("Connecting stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub fn shares_eject(path: String) -> Result<(), String> {
    if !mounted()?.iter().any(|item| item.path == path) {
        return Err("That folder is not a mounted network share.".into());
    }
    system::eject(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(protocol: &str, host: &str, user: &str, port: u16) -> crate::profiles::Profile {
        crate::profiles::Profile {
            id: "p".into(),
            name: "P".into(),
            host: host.into(),
            protocol: protocol.into(),
            port,
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
    fn builds_finder_addresses_without_passwords() {
        assert_eq!(
            finder_url(&profile(
                "webdav",
                "https://dav.example.com/remote.php/dav/",
                "ada lovelace",
                443
            ))
            .unwrap(),
            "https://ada%20lovelace@dav.example.com/remote.php/dav/"
        );
        assert_eq!(
            finder_url(&profile("ftp", "files.example.com", "a@b", 2121)).unwrap(),
            "ftp://a%40b@files.example.com:2121/"
        );
        assert!(finder_url(&profile("ftps", "files.example.com", "a", 21))
            .unwrap_err()
            .contains("FTPS"));
        assert!(finder_url(&profile("ssh", "host", "a", 22))
            .unwrap_err()
            .contains("Files in Weave"));
        assert!(finder_url(&profile("ftp", "-oops", "a", 21)).is_err());
    }

    #[test]
    fn builds_share_addresses() {
        assert_eq!(
            share_url("smb", "nas.local", "Photos", "ada").unwrap(),
            "smb://ada@nas.local/Photos"
        );
        assert_eq!(
            share_url("smb", "10.0.0.5", "Team Files", "").unwrap(),
            "smb://10.0.0.5/Team%20Files"
        );
        assert_eq!(
            share_url("smb", "nas", "x", "CORP;ada").unwrap(),
            "smb://CORP%3Bada@nas/x"
        );
        assert_eq!(
            share_url("nfs", "nas.local", "/volume1/data", "").unwrap(),
            "nfs://nas.local/volume1/data"
        );
        assert!(share_url("smb", "nas", "a/b", "").is_err());
        assert!(share_url("smb", "bad host", "x", "").is_err());
        assert!(share_url("ftp", "nas", "x", "").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn finds_network_mounts() {
        let output = "/dev/disk3s1 on / (apfs, local, journaled)\n//ada@nas.local/Photos on /Volumes/Photos (smbfs, nodev, nosuid, mounted by ada)\nnas:/volume1/data on /Volumes/data (nfs, nodev)\n";
        assert_eq!(
            parse_mounts(output),
            vec![
                MountedShare {
                    kind: "smb".into(),
                    source: "//ada@nas.local/Photos".into(),
                    path: "/Volumes/Photos".into()
                },
                MountedShare {
                    kind: "nfs".into(),
                    source: "nas:/volume1/data".into(),
                    path: "/Volumes/data".into()
                },
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn turns_share_addresses_into_windows_paths() {
        use super::system::{parse_net_use, unc_path};
        assert_eq!(
            unc_path("smb://ada@nas.local/Photos").unwrap(),
            r"\\nas.local\Photos"
        );
        assert_eq!(
            unc_path("https://dav.example.com:8443/remote.php/dav/").unwrap(),
            r"\\dav.example.com@SSL@8443\DavWWWRoot\remote.php\dav"
        );
        assert!(unc_path("ftp://files.example.com/").is_err());
        let shares = parse_net_use(
            "OK           Z:        \\\\nas\\Photos       Microsoft Windows Network\r\n",
        );
        assert_eq!(shares[0].path, "Z:\\");
        assert_eq!(shares[0].source, r"\\nas\Photos");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn reads_linux_share_mounts() {
        use super::system::{gio_url, parse_gvfs, parse_proc_mounts};
        let gvfs = parse_gvfs(
            &["smb-share:server=nas,share=photos".into(), "trash".into()],
            std::path::Path::new("/run/user/1000/gvfs"),
        );
        assert_eq!(gvfs.len(), 1);
        assert_eq!(gvfs[0].kind, "smb");
        assert_eq!(gvfs[0].source, "nas/photos");
        let kernel =
            parse_proc_mounts("//nas/data /mnt/my\\040data cifs rw 0 0\n/dev/sda1 / ext4 rw 0 0\n");
        assert_eq!(kernel.len(), 1);
        assert_eq!(kernel[0].path, "/mnt/my data");
        assert_eq!(
            gio_url("https://dav.example.com/x"),
            "davs://dav.example.com/x"
        );
    }
}
