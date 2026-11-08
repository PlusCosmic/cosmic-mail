# Omarchy integration

Cosmic Mail is designed as an Omarchy-native application rather than a portable webmail
wrapper. Its desktop integration currently covers theming, notifications, and the
background application lifecycle.

## Live theming

The application reads `colors.toml` from the active theme and watches the active-theme
symlink for changes. Switching themes recolours the application without a restart,
including HTML mail, which the reader renders as themed text. If the active theme cannot be read, Cosmic Mail falls back
to a Kanagawa-inspired palette.

The active theme lives at `$XDG_STATE_HOME/omarchy/current` (default
`~/.local/state/omarchy/current`, containing `theme/`, `theme.name` and `background`).
Older Omarchy releases kept it at `~/.config/omarchy/current`; Cosmic Mail falls back to
that location when only it exists. `omarchy theme current` prints the active theme name.

`theme/colors.toml` uses named colours (`accent`, `selection`, `muted`, `background`,
`darker_background`, `foreground`, `bright_foreground`, `red`..`magenta`,
`bright_red`..`bright_magenta`, ...). Cosmic Mail maps them onto the 16-slot terminal
palette that drives the UI's `c[0]`..`c[15]` slots and the avatar colours: `color0` =
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

One Cosmic Mail process owns the session: it holds the IMAP IDLE connections and delivers
notifications whether or not its window is mapped. Closing the window only hides it.
The first launch claims `$XDG_RUNTIME_DIR/cosmic-mail.sock`; a later Walker launch hands
its request (with the launcher's XDG activation token) over that socket and exits, so the
same process shows and focuses its window instead of starting another sync engine.
`--background` starts the process without a window, which is how the packaged systemd
user service (bound to the graphical session) runs it. See [RELEASING.md](RELEASING.md).

## System tray

The tray uses Rune's built-in StatusNotifierItem implementation (`rune::tray`, over
D-Bus), so it appears wherever a tray host such as Waybar runs; without a host the app
starts anyway and logs that the tray is unavailable. A left click shows and focuses the
window (forwarding the host's activation token). Its menu provides **Open Cosmic Mail**,
**Sync now**, and **Quit**. Open reuses the window-activation path, including the
Hyprland focus fallback; Quit terminates the background process rather than only hiding
its window. Sync now restarts every configured account's background sync task.
