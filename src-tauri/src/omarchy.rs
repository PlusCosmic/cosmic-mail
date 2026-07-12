//! Omarchy theme integration: read the active theme's colors and watch for
//! theme switches, emitting `omarchy:theme-changed` to the frontend.
//!
//! The active theme lives at `~/.config/omarchy/current/theme`, a symlink that
//! `omarchy-theme-set` swaps atomically. We watch the parent directory
//! `~/.config/omarchy/current` (non-recursively), debounce, re-read, and emit.

use std::path::PathBuf;
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

/// Raw `colors.toml` shape as written by omarchy themes.
#[derive(Debug, Deserialize)]
struct ColorsToml {
    accent: Option<String>,
    foreground: Option<String>,
    background: Option<String>,
    cursor: Option<String>,
    selection_foreground: Option<String>,
    selection_background: Option<String>,
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

/// `~/.config/omarchy/current`.
fn current_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("omarchy").join("current"))
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
    let dir = current_dir().context("no config dir")?;
    let colors_path = dir.join("theme").join("colors.toml");
    let name_path = dir.join("theme.name");

    let raw = std::fs::read_to_string(&colors_path)
        .with_context(|| format!("reading {}", colors_path.display()))?;
    let colors: ColorsToml =
        toml::from_str(&raw).with_context(|| format!("parsing {}", colors_path.display()))?;

    let name = std::fs::read_to_string(&name_path)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let fb = kanagawa_fallback();
    let palette: Vec<String> = [
        colors.color0,
        colors.color1,
        colors.color2,
        colors.color3,
        colors.color4,
        colors.color5,
        colors.color6,
        colors.color7,
        colors.color8,
        colors.color9,
        colors.color10,
        colors.color11,
        colors.color12,
        colors.color13,
        colors.color14,
        colors.color15,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, c)| c.unwrap_or_else(|| fb.palette[i].clone()))
    .collect();

    Ok(OmarchyTheme {
        name,
        accent: colors.accent.unwrap_or(fb.accent),
        foreground: colors.foreground.unwrap_or(fb.foreground),
        background: colors.background.unwrap_or(fb.background),
        cursor: colors.cursor.unwrap_or(fb.cursor),
        selection_foreground: colors
            .selection_foreground
            .unwrap_or(fb.selection_foreground),
        selection_background: colors
            .selection_background
            .unwrap_or(fb.selection_background),
        palette,
    })
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
