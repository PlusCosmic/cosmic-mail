# Setup and usage

Cosmic Mail currently runs from source and targets Omarchy on Arch Linux. Hosted release
artifacts and distro packages are planned for a later stage of the project.

## Requirements

- Rust via rustup (the pinned 1.99.1 toolchain in `rust-toolchain.toml` installs on first build)
- Arch packages: `libxkbcommon`, `wayland`, `vulkan-icd-loader`, and your GPU's Vulkan driver
  (`vulkan-radeon`, `vulkan-intel`, `nvidia-utils`, ...)
- A running Secret Service implementation for credential storage

Omarchy's default environment provides the expected Hyprland, Mako, and Secret Service
integration.

## Run from source

```sh
git clone https://github.com/PlusCosmic/cosmic-mail.git
cd cosmic-mail
cargo run --release
```

`--background` starts without a window (tray and sync only). Launching again while it runs
shows and focuses the existing window instead of starting a second process.

## Account setup

### IMAP

Enter an email address and password in the add-account dialog. Cosmic Mail attempts
provider autoconfiguration, Mozilla ISPDB lookup, DNS discovery, and conventional IMAP
hostnames. The connector currently supports implicit TLS on port 993; STARTTLS-only
providers are not yet supported.

The password is stored in the Secret Service keyring and is never written to the account
configuration file or message database.

### Gmail

Gmail sign-in requires a Google OAuth client with the **Desktop app** application type.
Create one in the [Google Cloud credentials
console](https://console.cloud.google.com/apis/credentials), enable the Gmail API, and
save the credentials to `~/.config/cosmic-mail/google-oauth.json`:

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
with Gmail's IMAP service. The OAuth refresh token is stored only in the Secret Service
keyring.

## Settings

The cog in the account rail's bottom action group (below Compose and Add account) opens the
Settings dialog; the command palette's "Open settings" entry opens it too. Escape closes it and
changes save immediately.

- **Always download remote images** — off by default. When on, HTML messages load HTTP(S) images
  without the per-message "Load remote images" prompt. This relaxes **images only**: HTML mail is
  always rendered as styled text, so scripts, styles, forms, and every other remote resource are
  never loaded. Leave it off if you want tracking-pixel protection on a per-message basis.
- **Show the window title bar** (Linux only; Windows and macOS always use Cosmic Mail's own
  titlebar) — on by default: the system title bar and borders, to move,
  minimise and close the window. Turn it off under a tiling window manager such as omarchy's
  Hyprland, which draws its own. Takes effect the next time Cosmic Mail starts (restart the
  `cosmic-mail.service` if it runs in the background).

Preferences are stored, without secrets, in `~/.config/cosmic-mail/settings.json`.

## System tray

A tray icon stays available while Cosmic Mail runs in the background (where a StatusNotifier
host such as waybar runs). Clicking it shows and focuses the window; its menu offers **Open
Cosmic Mail**, **Sync now** (restart sync for every account), and **Quit**. Closing the window
only hides it — mail keeps syncing and notifying until you quit from the tray.

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

| data | location |
|---|---|
| account settings, without secrets | `~/.config/cosmic-mail/accounts.json` |
| global preferences, without secrets | `~/.config/cosmic-mail/settings.json` |
| window layout (list/reader split) | `~/.config/cosmic-mail/ui.json` |
| message cache | `~/.local/share/cosmic-mail/mail.db` |
| IMAP passwords and OAuth refresh tokens | Secret Service keyring, service `dev.pluscosmic.mail` |
