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

