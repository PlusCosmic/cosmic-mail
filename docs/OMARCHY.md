# Omarchy integration

Cosmic Mail is designed as an Omarchy-native application rather than a portable webmail
wrapper. Its desktop integration currently covers theming, notifications, and the
background application lifecycle.

## Live theming

The application reads `colors.toml` from the active theme and watches the active-theme
symlink for changes. Switching themes recolours the application without a restart,
including unstyled HTML mail. If the active theme cannot be read, Cosmic Mail falls back
to a Kanagawa-inspired palette.

The active theme lives at `$XDG_STATE_HOME/omarchy/current` (default
`~/.local/state/omarchy/current`, containing `theme/`, `theme.name` and `background`).
Older Omarchy releases kept it at `~/.config/omarchy/current`; Cosmic Mail falls back to
that location when only it exists. `omarchy theme current` prints the active theme name.

`theme/colors.toml` uses named colours (`accent`, `selection`, `muted`, `background`,
`darker_background`, `foreground`, `bright_foreground`, `red`..`magenta`,
`bright_red`..`bright_magenta`, ...). Cosmic Mail maps them onto the 16-slot terminal
palette that drives `--c0`..`--c15` and the avatar colours: `color0` =
`darker_background` (else `background`), `color1`..`color6` = `red`, `green`, `yellow`,
`blue`, `magenta`, `cyan`, `color7` = `foreground`, `color8` = `muted`,
`color9`..`color14` = the `bright_*` counterparts, `color15` = `bright_foreground`, and
`selection` = the selection background. Cursor and selection foreground default to
`foreground`. The pre-named-colour keys (`color0`..`color15`, `cursor`,
`selection_foreground`, `selection_background`) are still honoured as a fallback for
older installs.

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

The tray is a StatusNotifierItem that Wails registers over D-Bus itself (no
`libappindicator` dependency). Its menu provides **Open Cosmic Mail**, **Sync now**, and **Quit**.
Open reuses the existing window-activation path, including the Hyprland focus fallback; Quit
explicitly terminates the background process rather than only hiding its window. Sync now restarts
every configured account's background sync task.

Linux tray interaction is menu-driven: the omarchy shell (or Waybar) shows the icon and opens the
menu on click, so a left click goes through the menu rather than directly revealing the window.
A missing StatusNotifier host (a headless CI runner) only logs an error; startup continues.
