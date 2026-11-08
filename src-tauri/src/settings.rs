//! Global application preferences persisted to
//! `$XDG_CONFIG_HOME/cosmic-mail/settings.json`.
//!
//! Mirrors the config-dir JSON persistence pattern in [`crate::accounts`].
//! No secrets are involved. The read path never fails: a missing or malformed
//! file yields [`Settings::default`] so a corrupt config can never brick the UI.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// User-facing global preferences.
///
/// `#[serde(default)]` tolerates missing keys; unknown keys are ignored, so
/// older configs load cleanly and forward-compatible fields survive downgrades.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// When true, HTML messages may load HTTP(S) images without per-message
    /// consent. Sanitization, iframe sandboxing, and all non-image network
    /// restrictions are unchanged. Defaults to `false`.
    pub always_download_remote_images: bool,
}

/// Path to `settings.json`: `$XDG_CONFIG_HOME/cosmic-mail/settings.json`.
pub fn settings_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("could not determine config dir (XDG_CONFIG_HOME)")?
        .join("cosmic-mail");
    Ok(dir.join("settings.json"))
}

/// Load settings from the given path, falling back to defaults on any read or
/// parse problem (missing file, unreadable file, malformed JSON).
fn load_from(path: &Path) -> Settings {
    let Ok(data) = std::fs::read(path) else {
        return Settings::default();
    };
    serde_json::from_slice(&data).unwrap_or_default()
}

/// Atomically persist settings to the given path.
fn save_to(path: &Path, settings: &Settings) -> Result<()> {
    let dir = path
        .parent()
        .context("settings.json has no parent directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let json = serde_json::to_vec_pretty(settings).context("serializing settings")?;

    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("creating temp file in {}", dir.display()))?;
    tmp.write_all(&json).context("writing settings temp file")?;
    tmp.flush().context("flushing settings temp file")?;
    tmp.persist(path)
        .with_context(|| format!("persisting {}", path.display()))?;
    Ok(())
}

/// Load global settings, returning defaults if the file is absent or malformed.
///
/// This never errors: settings are non-critical preferences and a broken file
/// must not block the UI from loading.
pub fn load_settings() -> Settings {
    match settings_path() {
        Ok(path) => load_from(&path),
        Err(err) => {
            tracing::warn!(error = %err, "could not resolve settings path; using defaults");
            Settings::default()
        }
    }
}

/// Persist global settings to disk.
pub fn save_settings(settings: &Settings) -> Result<()> {
    let path = settings_path()?;
    save_to(&path, settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_file_missing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("settings.json");
        assert_eq!(load_from(&path), Settings::default());
        assert!(!load_from(&path).always_download_remote_images);
    }

    #[test]
    fn defaults_when_malformed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{ this is not valid json").expect("write");
        assert_eq!(load_from(&path), Settings::default());
    }

    #[test]
    fn tolerates_unknown_and_missing_keys() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, br#"{"somethingNew": 42}"#).expect("write");
        // Missing known key -> default; unknown key ignored.
        assert_eq!(load_from(&path), Settings::default());
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("settings.json");
        let settings = Settings {
            always_download_remote_images: true,
        };
        save_to(&path, &settings).expect("save");
        assert_eq!(load_from(&path), settings);
    }

    #[test]
    fn persists_camelcase_key_on_disk() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("settings.json");
        save_to(
            &path,
            &Settings {
                always_download_remote_images: true,
            },
        )
        .expect("save");
        let raw = std::fs::read_to_string(&path).expect("read back");
        assert!(raw.contains("alwaysDownloadRemoteImages"));
        assert!(!raw.contains("always_download_remote_images"));
    }
}
