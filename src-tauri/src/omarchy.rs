//! Omarchy theme integration: read the active theme's colors and watch for
//! theme switches, emitting `omarchy:theme-changed` to the frontend.
//!
//! The active theme lives at `$XDG_STATE_HOME/omarchy/current/theme`
//! (`~/.local/state/omarchy/current/theme`; older omarchy releases used
//! `~/.config/omarchy/current/theme`), a symlink that `omarchy-theme-set` swaps
//! atomically. We watch the parent directory `current` (non-recursively),
//! debounce, re-read, and emit.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Theme colors mirrored to the frontend as CSS custom properties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OmarchyTheme {
    pub name: String,
    pub accent: String,
    pub foreground: String,
    pub background: String,
    pub cursor: String,
    pub selection_foreground: String,
    pub selection_background: String,
    /// color0..color15 as `#rrggbb`.
    pub palette: Vec<String>,
}

/// Raw `colors.toml` shape. Covers both dialects: the current named-colour
/// format (`accent`, `selection`, `muted`, `background`, `red`, `bright_red`,
/// ...) and the older terminal-style one (`cursor`, `selection_*`,
/// `color0`..`color15`). Named keys win; the old keys are the fallback for
/// older installs.
#[derive(Debug, Default, Deserialize)]
struct ColorsToml {
    accent: Option<String>,
    foreground: Option<String>,
    background: Option<String>,
    cursor: Option<String>,
    selection: Option<String>,
    selection_foreground: Option<String>,
    selection_background: Option<String>,
    muted: Option<String>,
    darker_background: Option<String>,
    bright_foreground: Option<String>,
    red: Option<String>,
    green: Option<String>,
    yellow: Option<String>,
    blue: Option<String>,
    magenta: Option<String>,
    cyan: Option<String>,
    bright_red: Option<String>,
    bright_green: Option<String>,
    bright_yellow: Option<String>,
    bright_blue: Option<String>,
    bright_magenta: Option<String>,
    bright_cyan: Option<String>,
    color0: Option<String>,
    color1: Option<String>,
    color2: Option<String>,
    color3: Option<String>,
    color4: Option<String>,
    color5: Option<String>,
    color6: Option<String>,
    color7: Option<String>,
    color8: Option<String>,
    color9: Option<String>,
    color10: Option<String>,
    color11: Option<String>,
    color12: Option<String>,
    color13: Option<String>,
    color14: Option<String>,
    color15: Option<String>,
}

/// `$XDG_STATE_HOME`, falling back to `~/.local/state`.
fn state_dir() -> Option<PathBuf> {
    match std::env::var_os("XDG_STATE_HOME").map(PathBuf::from) {
        Some(p) if p.is_absolute() => Some(p),
        _ => dirs::home_dir().map(|h| h.join(".local").join("state")),
    }
}

/// The omarchy `current` directory locations, in preference order: the state
/// dir (current omarchy) then the config dir (older releases).
fn current_dir_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(state) = state_dir() {
        out.push(state.join("omarchy").join("current"));
    }
    if let Some(cfg) = dirs::config_dir() {
        out.push(cfg.join("omarchy").join("current"));
    }
    out
}

/// The omarchy `current` directory: the first candidate that exists, else the
/// state-dir path so callers report the location current omarchy would use.
fn current_dir() -> Option<PathBuf> {
    let candidates = current_dir_candidates();
    candidates
        .iter()
        .find(|d| d.is_dir())
        .or(candidates.first())
        .cloned()
}

/// Built-in kanagawa fallback used when the omarchy theme files are missing.
pub fn kanagawa_fallback() -> OmarchyTheme {
    OmarchyTheme {
        name: "kanagawa".to_string(),
        accent: "#7e9cd8".to_string(),
        foreground: "#dcd7ba".to_string(),
        background: "#1f1f28".to_string(),
        cursor: "#c8c093".to_string(),
        selection_foreground: "#c8c093".to_string(),
        selection_background: "#2d4f67".to_string(),
        palette: vec![
            "#090618", "#c34043", "#76946a", "#c0a36e", "#7e9cd8", "#957fb8", "#6a9589", "#c8c093",
            "#727169", "#e82424", "#98bb6c", "#e6c384", "#7fb4ca", "#938aa9", "#7aa89f", "#dcd7ba",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
    }
}

/// Read the active omarchy theme, falling back to kanagawa on any problem.
pub fn read_theme() -> OmarchyTheme {
    match try_read_theme() {
        Ok(theme) => theme,
        Err(err) => {
            tracing::debug!(error = %err, "falling back to kanagawa theme");
            kanagawa_fallback()
        }
    }
}

fn try_read_theme() -> Result<OmarchyTheme> {
    let dir = current_dir().context("no state or config dir")?;
    read_theme_from(&dir)
}

/// Read the theme from an omarchy `current` directory (`theme/colors.toml`
/// and `theme.name` inside it).
fn read_theme_from(dir: &Path) -> Result<OmarchyTheme> {
    let colors_path = dir.join("theme").join("colors.toml");
    let name_path = dir.join("theme.name");

    let raw = std::fs::read_to_string(&colors_path)
        .with_context(|| format!("reading {}", colors_path.display()))?;
    let colors: ColorsToml =
        toml::from_str(&raw).with_context(|| format!("parsing {}", colors_path.display()))?;

    let name = std::fs::read_to_string(&name_path)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    Ok(theme_from_colors(name, colors))
}

/// The first present candidate, else the fallback.
fn first(fallback: &str, candidates: &[&Option<String>]) -> String {
    candidates
        .iter()
        .find_map(|c| c.as_ref())
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

fn theme_from_colors(name: String, c: ColorsToml) -> OmarchyTheme {
    let fb = kanagawa_fallback();
    // Named colours (current omarchy) map onto the terminal palette slots;
    // color0..15 (older omarchy) are the fallback, then kanagawa. background
    // and foreground exist in both dialects, so an explicit color0/color7
    // outranks them.
    let slots: [Vec<&Option<String>>; 16] = [
        vec![&c.color0, &c.darker_background, &c.background],
        vec![&c.red, &c.color1],
        vec![&c.green, &c.color2],
        vec![&c.yellow, &c.color3],
        vec![&c.blue, &c.color4],
        vec![&c.magenta, &c.color5],
        vec![&c.cyan, &c.color6],
        vec![&c.color7, &c.foreground],
        vec![&c.muted, &c.color8],
        vec![&c.bright_red, &c.color9],
        vec![&c.bright_green, &c.color10],
        vec![&c.bright_yellow, &c.color11],
        vec![&c.bright_blue, &c.color12],
        vec![&c.bright_magenta, &c.color13],
        vec![&c.bright_cyan, &c.color14],
        vec![&c.bright_foreground, &c.color15],
    ];
    let palette = slots
        .iter()
        .enumerate()
        .map(|(i, cands)| first(&fb.palette[i], cands))
        .collect();
    let foreground = first(&fb.foreground, &[&c.foreground]);
    OmarchyTheme {
        name,
        accent: first(&fb.accent, &[&c.accent]),
        background: first(&fb.background, &[&c.background]),
        cursor: first(&foreground, &[&c.cursor]),
        selection_foreground: first(&foreground, &[&c.selection_foreground]),
        selection_background: first(
            &fb.selection_background,
            &[&c.selection, &c.selection_background],
        ),
        foreground,
        palette,
    }
}

/// Spawn a background thread that watches for omarchy theme changes and emits
/// `omarchy:theme-changed`. Runs for the lifetime of the app.
pub fn spawn_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        if let Err(err) = watch_loop(app) {
            tracing::warn!(error = %err, "omarchy watcher stopped");
        }
    });
}

fn watch_loop(app: AppHandle) -> Result<()> {
    use notify::{RecursiveMode, Watcher};

    let dir = match current_dir() {
        Some(d) if d.exists() => d,
        _ => {
            tracing::info!("omarchy current dir not present; theme watcher inactive");
            return Ok(());
        }
    };

    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        // Ignore access events: our own `read_theme()` reads files inside the
        // watched dir, so reacting to reads creates a 300ms feedback loop
        // (and any other process reading the theme would retrigger us too).
        match &res {
            Ok(event) if event.kind.is_access() => {}
            _ => {
                let _ = tx.send(res);
            }
        }
    })
    .context("creating fs watcher")?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", dir.display()))?;

    tracing::info!(dir = %dir.display(), "watching omarchy theme dir");

    // Block for the next event; the channel closes when the watcher is dropped.
    let mut last = read_theme();
    while let Ok(first) = rx.recv() {
        if first.is_err() {
            continue;
        }
        // Debounce: drain any events that arrive within ~300ms.
        while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}

        // Only emit on an actual change; the watcher fires for unrelated
        // churn in the dir (e.g. background rotation touching symlinks).
        let theme = read_theme();
        if theme == last {
            continue;
        }
        last = theme.clone();
        if let Err(err) = app.emit("omarchy:theme-changed", &theme) {
            tracing::warn!(error = %err, "failed to emit theme change");
        } else {
            tracing::info!(theme = %theme.name, "emitted omarchy theme change");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMED: &str = r##"
accent = "#d79921"
selection = "#504945"
muted = "#928374"
background = "#282828"
darker_background = "#1d2021"
foreground = "#ebdbb2"
bright_foreground = "#fbf1c7"
red = "#cc241d"
green = "#98971a"
yellow = "#d79921"
blue = "#458588"
magenta = "#b16286"
cyan = "#689d6a"
bright_red = "#fb4934"
bright_green = "#b8bb26"
bright_yellow = "#fabd2f"
bright_blue = "#83a598"
bright_magenta = "#d3869b"
bright_cyan = "#8ec07c"
"##;

    fn write_theme(colors: &str, name: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("theme")).unwrap();
        std::fs::write(dir.path().join("theme").join("colors.toml"), colors).unwrap();
        if let Some(n) = name {
            std::fs::write(dir.path().join("theme.name"), format!("{n}\n")).unwrap();
        }
        dir
    }

    #[test]
    fn named_colours_map_onto_the_palette() {
        let dir = write_theme(NAMED, Some("gruvbox"));
        let t = read_theme_from(dir.path()).unwrap();
        assert_eq!(t.name, "gruvbox");
        assert_eq!(t.accent, "#d79921");
        assert_eq!(t.background, "#282828");
        assert_eq!(t.foreground, "#ebdbb2");
        assert_eq!(t.cursor, "#ebdbb2", "cursor defaults to foreground");
        assert_eq!(t.selection_foreground, "#ebdbb2");
        assert_eq!(t.selection_background, "#504945");
        assert_eq!(t.palette[0], "#1d2021", "color0 is darker_background");
        assert_eq!(t.palette[1], "#cc241d");
        assert_eq!(t.palette[6], "#689d6a");
        assert_eq!(t.palette[7], "#ebdbb2", "color7 is foreground");
        assert_eq!(t.palette[8], "#928374", "color8 is muted");
        assert_eq!(t.palette[9], "#fb4934");
        assert_eq!(t.palette[15], "#fbf1c7");
    }

    #[test]
    fn legacy_keys_are_still_honoured() {
        let legacy = r##"
accent = "#7e9cd8"
cursor = "#ffffff"
selection_foreground = "#000000"
selection_background = "#111111"
color0 = "#000001"
color1 = "#000002"
color15 = "#00000f"
"##;
        let dir = write_theme(legacy, None);
        let t = read_theme_from(dir.path()).unwrap();
        assert_eq!(t.name, "unknown");
        assert_eq!(t.cursor, "#ffffff");
        assert_eq!(t.selection_foreground, "#000000");
        assert_eq!(t.selection_background, "#111111");
        assert_eq!(t.palette[0], "#000001");
        assert_eq!(t.palette[1], "#000002");
        assert_eq!(t.palette[15], "#00000f");
        assert_eq!(t.palette[2], kanagawa_fallback().palette[2]);
    }

    #[test]
    fn missing_theme_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_theme_from(dir.path()).is_err());
    }
}
