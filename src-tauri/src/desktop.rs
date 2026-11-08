//! Desktop process and window lifecycle.
//!
//! The application process owns background sync for the whole graphical
//! session. Its main window is only a view onto that process: closing the view
//! hides it, while launcher and notification activations show the same window.

use std::ffi::OsStr;

use tauri::{AppHandle, Manager};

pub const BACKGROUND_ARG: &str = "--background";
pub const MAIN_WINDOW: &str = "main";

/// Whether argv requests a silent, background-only startup.
pub fn is_background_launch<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .any(|arg| arg.as_ref() == OsStr::new(BACKGROUND_ARG))
}

/// Show, unminimize, and focus the existing main window.
///
/// This is shared by normal launcher re-entry and notification actions so all
/// explicit user activations follow the same Wayland/Hyprland behavior.
pub fn activate_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        tracing::warn!("main window is unavailable for activation");
        return;
    };

    if let Err(err) = window.unminimize() {
        tracing::warn!(error = %err, "could not unminimize main window");
    }
    if let Err(err) = window.show() {
        tracing::warn!(error = %err, "could not show main window");
    }
    if let Err(err) = window.set_focus() {
        tracing::warn!(error = %err, "could not focus main window");
    }

    // Wayland compositors may reject focus requests from a background client.
    // Cosmic Mail targets Omarchy/Hyprland, whose IPC can honor an explicit
    // user activation. Keep fixed arguments and avoid a shell.
    #[cfg(target_os = "linux")]
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        tracing::debug!("requesting application focus through hyprctl");
        match std::process::Command::new("hyprctl")
            .args(["dispatch", "focuswindow", "class:^(cosmic-mail)$"])
            .status()
        {
            Ok(status) if status.success() => {}
            Ok(status) => tracing::warn!(%status, "hyprctl application focus failed"),
            Err(err) => tracing::warn!(error = %err, "could not run hyprctl application focus"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_background_argument() {
        assert!(is_background_launch(["cosmic-mail", BACKGROUND_ARG]));
        assert!(is_background_launch([
            "cosmic-mail",
            "--unrelated",
            BACKGROUND_ARG,
        ]));
    }

    #[test]
    fn normal_launch_is_not_background() {
        assert!(!is_background_launch(["cosmic-mail"]));
        assert!(!is_background_launch(["cosmic-mail", "--background-task",]));
    }
}
