# Setup and usage

Cosmic Mail currently runs from source and targets Omarchy on Arch Linux. Hosted release
artifacts and distro packages are planned for a later stage of the project.

## Requirements

- Node.js and npm
- A current Rust toolchain
- Tauri's Linux development dependencies
- Arch packages: `webkit2gtk-4.1`, `gtk3`, `librsvg`, and `openssl`
- A running Secret Service implementation for credential storage

Omarchy's default environment provides the expected Hyprland, Mako, and Secret Service
integration.

## Run from source

```sh
git clone https://github.com/PlusCosmic/cosmic-mail.git
cd cosmic-mail
npm install
npm run tauri dev
```

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
  without the per-message "Load remote images" prompt. This relaxes **images only**: message
  sanitization and iframe sandboxing stay on, and scripts, forms, and every other remote resource
  remain blocked. Leave it off if you want tracking-pixel protection on a per-message basis.

Preferences are stored, without secrets, in `~/.config/cosmic-mail/settings.json`.

## Keyboard shortcuts

| key | action |
|---|---|
| `j` / `k` | select the next or previous message |
| `gg` / `G` | jump to the first or last message |
| `Enter` | open the selected message |
| `Esc` | return to the message list |
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
| `Ctrl+K` | open the command palette |
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
| message cache | `~/.local/share/cosmic-mail/mail.db` |
| IMAP passwords and OAuth refresh tokens | Secret Service keyring, service `dev.pluscosmic.mail` |
