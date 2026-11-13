# Desktop integration

Cosmic Mail is a native app on Windows, macOS and Linux, with no platform favoured. This
page covers how it fits each desktop: appearance, notifications, the background process,
and the tray. Where a platform lacks something, it is listed as a gap, not a footnote.

## Appearance

Cosmic Mail takes its look from the [UI foundation](https://github.com/PlusCosmic/ui-foundation)
(`rune-theme-foundation`) on every platform. Settings → Appearance picks one of its five
colour schemes (Stacks, Oxblood, Ink, Walnut, Graphite) and System, Light or Dark; System
follows the desktop's light or dark preference. Both choices persist in `ui.json` in the
config directory (`colorScheme`, `appearance`; see [SETUP.md](SETUP.md#local-data)). The
fonts (Instrument Sans, with Fraunces for display) are bundled, so no system font is needed.

On Windows and macOS the app draws its own titlebar band (search lives in it) under the
platform's window controls. On Linux the window has the platform title bar by default;
Settings → Window → "Show the window title bar" turns it off for a tiling window manager
that draws its own borders (applies on the next start).

## Notifications

New mail in an inbox raises a notification on every platform. Notifications are
deduplicated with a per-folder high-water mark and suppressed during an account's first
sync; more than three new messages in one sync pass are combined into a summary.
Settings → Notifications has **Play a sound for new mail** (on by default; only the
first notification of a batch sounds) and **Send a test notification**.

| | delivered through | sound | click |
|---|---|---|---|
| **macOS** | Notification Center, as the `Cosmic Mail.app` bundle (`dev.pluscosmic.mail`); allow or mute it under System Settings → Notifications | the system alert sound | shows the window (reopening it if closed) and brings the app forward |
| **Windows** | a toast under the AppUserModelID `dev.pluscosmic.mail`, labelled Cosmic Mail; allow or mute it under Settings → System → Notifications. The installer's Start-menu shortcut carries the id, and the app registers it for the current user on its first notification, so the portable zip is named too | the "Mail" toast sound | while the toast is on screen: shows the window (reopening it if closed) and brings it forward |
| **Linux** | the freedesktop notification interface (any daemon: mako, dunst, GNOME, KDE, ...), app name `Cosmic Mail` plus a desktop-entry hint, so per-app daemon rules work | the `message-new-email` sound-name hint; daemons that play sounds honour it (mako doesn't, but an `on-notify` rule can) | shows and focuses the existing window |

## Background process

One Cosmic Mail process owns the session on every platform: it holds the IMAP IDLE
connections and notifies whether or not its window is showing. Closing the window only
hides it, and launching the app again brings the same window back instead of starting a
second sync engine. `--background` starts the process without a window.

- **Windows**: a per-user named pipe enforces the single instance. The installer can add
  a sign-in entry that runs `cosmic-mail.exe --background`.
- **macOS**: closing the window keeps the app in the Dock; clicking the Dock icon brings
  it back and the application menu's Quit ends it. Nothing installs a login item yet: add
  Cosmic Mail under System Settings → General → Login Items.
- **Linux**: the first launch claims `$XDG_RUNTIME_DIR/cosmic-mail.sock`; a later launch
  hands its request (with the launcher's XDG activation token) over the socket and exits.
  The Arch package ships a systemd user service that runs `--background` for the graphical
  session (see [RELEASING.md](RELEASING.md)). Under Hyprland, focus requests are followed by
  a fixed-argument `hyprctl` fallback, because Wayland focus-stealing prevention can ignore
  a background client.

## System tray

The tray icon offers **Open Cosmic Mail**, **Sync now** (restart sync for every account)
and **Quit**; a left click shows and focuses the window. Quit ends the process rather than
hiding the window.

- **Windows**: the notification area (Rune's Win32 tray).
- **Linux**: Rune's StatusNotifierItem tray over D-Bus, wherever a tray host (Waybar, KDE,
  GNOME with an AppIndicator extension, ...) runs; without one the app starts anyway and
  logs that the tray is unavailable.
- **macOS**: no menu-bar icon yet (Rune's tray has no macOS backend); the Dock covers
  showing the window and quitting.
