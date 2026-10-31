use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const FILE_NAME: &str = "capture-settings.json";
const MAX_SETTINGS_BYTES: usize = 16 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct StoredPreferences {
    capture_enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedPreferences {
    pub saved_enabled: bool,
    pub enabled: bool,
    pub environment_override: Option<bool>,
}

pub fn load(data_dir: &Path) -> Result<bool, String> {
    let path = data_dir.join(FILE_NAME);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("unable to inspect capture settings: {error}")),
    };
    if !metadata.file_type().is_file() {
        return Err("capture settings must be a regular file".into());
    }
    if metadata.len() > MAX_SETTINGS_BYTES as u64 {
        return Err("capture settings file is oversized".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| {
            file.take((MAX_SETTINGS_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| format!("unable to read capture settings: {error}"))?;
    if bytes.len() > MAX_SETTINGS_BYTES {
        return Err("capture settings file is oversized".into());
    }
    let preferences: StoredPreferences = serde_json::from_slice(&bytes)
        .map_err(|_| "capture settings file is invalid".to_string())?;
    Ok(preferences.capture_enabled)
}

pub fn resolve(
    data_dir: &Path,
    environment_override: Option<bool>,
) -> Result<ResolvedPreferences, String> {
    let saved_enabled = load(data_dir)?;
    Ok(ResolvedPreferences {
        saved_enabled,
        enabled: environment_override.unwrap_or(saved_enabled),
        environment_override,
    })
}

pub fn save(data_dir: &Path, enabled: bool) -> Result<(), String> {
    fs::create_dir_all(data_dir)
        .map_err(|error| format!("unable to create capture settings directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(data_dir, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("unable to secure capture settings directory: {error}"))?;
    }

    let destination = data_dir.join(FILE_NAME);
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        if !metadata.file_type().is_file() {
            return Err("capture settings must be a regular file".into());
        }
    }

    let staging = temp_path(data_dir);
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&staging)
            .map_err(|error| format!("unable to create capture settings: {error}"))?;
        let preferences = StoredPreferences {
            capture_enabled: enabled,
        };
        serde_json::to_writer(&mut file, &preferences)
            .map_err(|error| format!("unable to encode capture settings: {error}"))?;
        file.write_all(b"\n")
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("unable to save capture settings: {error}"))?;
        drop(file);
        fs::rename(&staging, &destination)
            .map_err(|error| format!("unable to publish capture settings: {error}"))?;
        sync_parent(data_dir).map_err(|error| {
            format!("capture settings saved but directory sync failed: {error}")
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}

fn temp_path(data_dir: &Path) -> PathBuf {
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    data_dir.join(format!(".{FILE_NAME}.{}.{}.tmp", std::process::id(), id))
}

fn sync_parent(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "chat-hub-capture-settings-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn defaults_to_off_and_persists_a_private_setting() {
        let directory = temp_dir();
        assert_eq!(resolve(&directory, None).unwrap().enabled, false);
        save(&directory, true).unwrap();
        assert!(resolve(&directory, None).unwrap().enabled);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(directory.join(FILE_NAME))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_environment_setting_overrides_the_saved_preference() {
        let directory = temp_dir();
        save(&directory, false).unwrap();
        let resolved = resolve(&directory, Some(true)).unwrap();
        assert!(!resolved.saved_enabled);
        assert!(resolved.enabled);
        assert_eq!(resolved.environment_override, Some(true));
        assert!(resolved.enabled);
        save(&directory, true).unwrap();
        let resolved = resolve(&directory, Some(false)).unwrap();
        assert!(resolved.saved_enabled);
        assert!(!resolved.enabled);
        assert_eq!(resolved.environment_override, Some(false));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_setting_is_rejected() {
        let directory = temp_dir();
        fs::write(directory.join(FILE_NAME), b"not json").unwrap();
        assert_eq!(
            load(&directory).unwrap_err(),
            "capture settings file is invalid"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn oversized_setting_is_rejected_before_reading_its_contents() {
        let directory = temp_dir();
        let path = directory.join(FILE_NAME);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        file.set_len((MAX_SETTINGS_BYTES + 1) as u64).unwrap();
        drop(file);
        assert_eq!(
            load(&directory).unwrap_err(),
            "capture settings file is oversized"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
