# Setup and usage

Cosmic Mail runs on Windows, macOS and Linux. Installers and the Arch package are
described in the [README](../README.md#install); this page covers running from source,
accounts, settings, shortcuts and where data lives.

## Requirements (from source)

- Rust via rustup (the pinned 1.99.1 toolchain in `rust-toolchain.toml` installs on first build)
- Python 3 on `PATH` (a build script needs it)
- **macOS**: the Xcode command line tools
- **Windows**: the MSVC build tools
- **Linux**: `libxkbcommon`, `wayland` (or X11), `vulkan-icd-loader` and your GPU's Vulkan
  driver, and a running Secret Service implementation (GNOME Keyring, KWallet, KeePassXC,
  ...) for credential storage

## Run from source

```sh
git clone https://github.com/PlusCosmic/cosmic-mail.git
cd cosmic-mail
cargo run --release
```

`--background` starts without a window (sync and notifications only). Launching again while
it runs shows and focuses the existing window instead of starting a second process.

## Account setup

### IMAP

Enter an email address and password in the add-account dialog. Cosmic Mail attempts
provider autoconfiguration, Mozilla ISPDB lookup, DNS discovery, and conventional IMAP
hostnames. The connector currently supports implicit TLS on port 993; STARTTLS-only
providers are not yet supported.

The password is stored in the platform keyring (see [Local data](#local-data)) and is never
written to the account
configuration file or message database.

### Gmail

Gmail sign-in requires a Google OAuth client with the **Desktop app** application type.
Create one in the [Google Cloud credentials
console](https://console.cloud.google.com/apis/credentials), enable the Gmail API, and
save the credentials to `google-oauth.json` in the config directory (see
[Local data](#local-data)):

```json
{
  "clientId": "…apps.googleusercontent.com",
  "clientSecret": "…"
}
```

The client secret is optional for a desktop OAuth client. For development, the
`COSMIC_MAIL_GOOGLE_CLIENT_ID` and `COSMIC_MAIL_GOOGLE_CLIENT_SECRET` environment
variables override the JSON file. Packaged builds can provide defaults with
`COSMIC_MAIL_BUILD_GOOGLE_CLIENT_ID` and `COSMIC_MAIL_BUILD_GOOGLE_CLIENT_SECRET` at
compile time.

Cosmic Mail opens an RFC 8252 loopback PKCE flow in the browser, then uses SASL XOAUTH2
with Gmail's IMAP service. The OAuth refresh token is stored only in the platform
keyring.

## Settings

The cog in the account rail's bottom action group (below Compose and Add account) opens the
Settings dialog; the command palette's "Open settings" entry opens it too. Escape closes it and
changes save immediately.

- **Always download remote images** — off by default. When on, HTML messages load HTTP(S) images
  without the per-message "Load remote images" prompt. This relaxes **images only**: HTML mail is
  always rendered as styled text, so scripts, styles, forms, and every other remote resource are
  never loaded. Leave it off if you want tracking-pixel protection on a per-message basis.
- **Play a sound for new mail** — on by default. New-mail notifications play the platform's
  notification sound (only the first of a batch). **Send a test notification** beside it shows
  a sample, to check notifications are allowed and audible. Per-platform details:
  [DESKTOP.md](DESKTOP.md#notifications).
- **Show the window title bar** (Linux only; Windows and macOS always use Cosmic Mail's own
  titlebar) — on by default: the system title bar and borders, to move,
  minimise and close the window. Turn it off under a tiling window manager (Hyprland, Sway,
  ...) that draws its own. Takes effect the next time Cosmic Mail starts (restart the
  `cosmic-mail.service` if it runs in the background).

Preferences are stored, without secrets, in `settings.json` in the config directory.

## Background and tray

Closing the window only hides it: mail keeps syncing and notifying until you quit. On
Windows and Linux a tray icon (Linux: wherever a StatusNotifier host such as Waybar or KDE's
panel runs) shows and focuses the window on click, and its menu offers **Open Cosmic Mail**,
**Sync now** (restart sync for every account), and **Quit**. macOS has no menu-bar icon
yet: the app stays in the Dock, which brings the window back, and the application menu's
Quit ends it. See [DESKTOP.md](DESKTOP.md).

## Keyboard shortcuts

| key | action |
|---|---|
| `j` / `k` | select the next or previous message |
| `gg` / `G` | jump to the first or last message |
| `↓` / `↑` | same as `j` / `k` |
| `Enter` | scroll the reader back to the top of the selected message |
| `Space` / `Shift+Space` | page the reader down / up |
| `Esc` | leave the focused control (back to list shortcuts) |
| `/` | focus the header search field |
| `Enter` (in search) | search the whole local cache for the typed text |
| `Esc` (in search) | clear an active search and return to the normal view |
| `u` | toggle read state |
| `c` | compose a new message |
| `r` | reply to the selected message |
| `a` | archive the selected message |
| `d` | delete the selected message (to Trash, or permanently when already in Trash) |
| `f` | toggle the flag on the selected message |
| `m` | move the selected message (opens a folder picker) |
| `Ctrl+K` | open the command palette (`↑`/`↓` or `Ctrl+N`/`Ctrl+P`, `Enter`, `Esc`) |
| `Ctrl+Enter` | send from the compose dialog |
| `1`, `2`, … | switch inbox or account view |

The separator between the message list and reader can be dragged. When focused, use
Left/Right to resize it, hold Shift for larger steps, use Home/End for the bounds, or
double-click to restore the default split.

The header field filters the already-loaded messages as you type. Pressing Enter instead runs a
search across the whole local cache (cached message envelopes and any fetched bodies) for the
current view's scope — the unified inbox searches every account, an account view searches that
account. Results are relevance-ranked, and an indicator shows the search term with a clear control.
Search does not reach mail that has not been synced locally; server-side search is planned.

## Local data

Everything lives under a `cosmic-mail` folder in the platform's config and data
directories (`XDG_CONFIG_HOME` / `XDG_DATA_HOME`, when set, win on every platform):

| | config directory | data directory |
|---|---|---|
| **Windows** | `%APPDATA%\cosmic-mail` | `%APPDATA%\cosmic-mail` |
| **macOS** | `~/Library/Application Support/cosmic-mail` | `~/Library/Application Support/cosmic-mail` |
| **Linux** | `~/.config/cosmic-mail` | `~/.local/share/cosmic-mail` |

| data | file |
|---|---|
| account settings, without secrets | config: `accounts.json` |
| global preferences, without secrets | config: `settings.json` |
| colour scheme, appearance, window layout | config: `ui.json` |
| Gmail OAuth client (optional) | config: `google-oauth.json` |
| message cache | data: `mail.db` |

IMAP passwords and OAuth refresh tokens live only in the platform keyring, service
`dev.pluscosmic.mail`: Windows Credential Manager, the macOS login Keychain, or the Secret
Service on Linux.
