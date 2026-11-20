use crate::profiles::Profile;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::{ffi::OsStrExt, fs::DirBuilderExt, fs::MetadataExt};

/// Options that let several `ssh` runs share one sign-in. The Windows build
/// of OpenSSH cannot share connections, so there each run signs in itself.
pub fn sharing_args(master: &str, persist: &str, socket: &Path) -> Vec<String> {
    if cfg!(windows) {
        return Vec::new();
    }
    vec![
        "-o".into(),
        format!("ControlMaster={master}"),
        "-o".into(),
        format!("ControlPersist={persist}"),
        "-o".into(),
        format!("ControlPath={}", socket.display()),
    ]
}

pub fn control_path(profile: &Profile) -> Result<PathBuf, String> {
    let mut hasher = Sha256::new();
    for field in [
        profile.id.as_str(),
        profile.host.as_str(),
        profile.username.as_str(),
        profile.jump_host.as_deref().unwrap_or(""),
        profile.key_path.as_deref().unwrap_or(""),
    ] {
        hasher.update(field.as_bytes());
        hasher.update([0]);
    }
    hasher.update(profile.port.to_be_bytes());
    let digest = hasher.finalize();
    let name = format!(
        "c-{}",
        digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );

    #[cfg(unix)]
    {
        let uid = unsafe { libc::geteuid() };
        let dir_name = format!("weave-{uid}");
        let preferred = std::env::temp_dir().join(&dir_name);
        let preferred_socket = preferred.join(&name);
        let directory = if preferred_socket.as_os_str().as_bytes().len() <= 80 {
            preferred
        } else {
            Path::new("/private/tmp").join(dir_name)
        };
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!(
                    "Could not prepare private SSH connection folder {}: {error}",
                    directory.display()
                ));
            }
        }
        let metadata = std::fs::symlink_metadata(&directory).map_err(|error| {
            format!(
                "Could not inspect SSH connection folder {}: {error}",
                directory.display()
            )
        })?;
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err(format!(
                "SSH connection folder {} must belong to your account and be private (mode 0700).",
                directory.display()
            ));
        }
        Ok(directory.join(name))
    }

    #[cfg(not(unix))]
    {
        let directory = std::env::temp_dir().join("weave-ssh");
        std::fs::create_dir_all(&directory).map_err(|error| {
            format!(
                "Could not prepare SSH connection folder {}: {error}",
                directory.display()
            )
        })?;
        Ok(directory.join(name))
    }
}

#[cfg(test)]
mod tests {
    use super::control_path;
    use crate::profiles::Profile;

    fn profile(id: &str) -> Profile {
        Profile {
            id: id.into(),
            name: "Test".into(),
            host: "localhost".into(),
            protocol: "ssh".into(),
            port: 22,
            username: "alice".into(),
            key_path: None,
            jump_host: None,
            color: "#9d91ec".into(),
            favorite: false,
            tls_pin_sha256: None,
            s3_region: None,
        }
    }

    #[test]
    fn gives_each_profile_a_stable_short_private_socket() {
        let first_profile = profile("first profile");
        let first = control_path(&first_profile).unwrap();
        assert_eq!(first, control_path(&first_profile).unwrap());
        assert_ne!(first, control_path(&profile("second profile")).unwrap());
        let mut edited = first_profile.clone();
        edited.host = "another.example".into();
        assert_ne!(first, control_path(&edited).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
            assert!(first.as_os_str().as_bytes().len() <= 80);
            assert_eq!(
                std::fs::metadata(first.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }
}
