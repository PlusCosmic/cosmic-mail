# Omarchy integration

Cosmic Mail is designed as an Omarchy-native application rather than a portable webmail
wrapper. Its desktop integration covers notifications and the background application lifecycle.

## Appearance

Cosmic Mail takes its look from the [UI foundation](https://github.com/PlusCosmic/ui-foundation)
(`rune-theme-foundation`), not from the active Omarchy theme. Settings → Appearance picks one of
its five colour schemes (Stacks, Oxblood, Ink, Walnut, Graphite) and System, Light or Dark;
System follows the desktop's light or dark preference. Both choices persist in
`$XDG_CONFIG_HOME/cosmic-mail/ui.json` (`colorScheme`, `appearance`). The fonts (Instrument
Sans, with Fraunces for display) are bundled, so no system font is needed.

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

The window has a title bar by default, as on any desktop. Under Hyprland you will usually
want it off: Settings → Window → "Show the window title bar", then restart Cosmic Mail
(`systemctl --user restart cosmic-mail.service` if the service runs it).

## System tray

The tray uses Rune's built-in StatusNotifierItem implementation (`rune::tray`, over
D-Bus), so it appears wherever a tray host such as Waybar runs; without a host the app
starts anyway and logs that the tray is unavailable. A left click shows and focuses the
window (forwarding the host's activation token). Its menu provides **Open Cosmic Mail**,
**Sync now**, and **Quit**. Open reuses the window-activation path, including the
Hyprland focus fallback; Quit terminates the background process rather than only hiding
its window. Sync now restarts every configured account's background sync task.
