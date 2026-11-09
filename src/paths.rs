//! Where the profile lives: `$XDG_CONFIG_HOME` / `$XDG_DATA_HOME` when set, on every platform.
//!
//! `dirs` only reads the XDG variables on Linux; on Windows it asks the shell for the known
//! folders (`%APPDATA%`) and ignores them. Tests and the E2E harness point these variables at a
//! throwaway profile, so without this they would read and write the real one on Windows.

use std::path::PathBuf;

/// The base config directory (the app adds `cosmic-mail/`).
pub fn config_dir() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME").or_else(dirs::config_dir)
}

/// The base data directory (the app adds `cosmic-mail/`).
pub fn data_dir() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME").or_else(dirs::data_dir)
}

/// The variable as a path, if set and absolute (the XDG spec says relative ones are ignored).
fn xdg(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}
