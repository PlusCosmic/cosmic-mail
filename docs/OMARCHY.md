# Omarchy integration

Cosmic Mail is designed as an Omarchy-native application rather than a portable webmail
wrapper. Its desktop integration currently covers theming, notifications, and the
background application lifecycle.

## Live theming

The application reads `~/.config/omarchy/current/theme/colors.toml` and watches the
active-theme symlink for changes. Switching themes recolours the application without a
restart, including unstyled HTML mail. If the active theme cannot be read, Cosmic Mail
falls back to a Kanagawa-inspired palette.

## Notifications

New-mail notifications use the freedesktop notification interface and are rendered by
Mako. They identify themselves with the stable app name `Cosmic Mail` and the matching
desktop-entry hint, allowing per-application Mako rules.

Notifications are deduplicated with a per-folder high-water mark and suppressed during
an account's first sync. More than three new messages in one sync pass are combined into
a summary. Activating a live notification reveals and focuses the existing Cosmic Mail
window.

## Background lifecycle

Locally promoted builds run as a systemd user service for the graphical session. The
service starts without mapping a window and continues to own IMAP IDLE connections and
notification delivery when the window is closed. Walker launches and notification
actions reveal the same single application process rather than starting another sync
engine.

See [Local daily releases](RELEASING.md) for promotion, service management, and rollback.

## System tray

The tray integration uses Tauri's built-in cross-platform tray API rather than calling
`libappindicator` directly. Its menu provides **Open Cosmic Mail**, **Sync now**, and **Quit**.
Open reuses the existing window-activation path, including the Hyprland focus fallback; Quit
explicitly terminates the background process rather than only hiding its window. Sync now restarts
every configured account's background sync task.

Tauri's current Linux tray backend is menu-oriented. It does not emit direct tray pointer events,
does not support tooltips or tray-rectangle queries, and cannot remove or replace a menu after it
has been attached (although the menu's contents can change). Consequently, Waybar interaction
will go through the tray menu instead of making a left click directly reveal the window. The Linux
bundle metadata declares the detected compatible AppIndicator/Ayatana runtime library through
Tauri's packaging path, but that is an implementation detail rather than an API Cosmic Mail uses
directly.
