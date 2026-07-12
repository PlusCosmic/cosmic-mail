# Cosmic Mail

A native Linux mail client for the [omarchy](https://omarchy.org) desktop, built with
**Tauri 2 + Rust + Svelte 5**. Supports plain **IMAP** accounts and **Gmail** (OAuth2),
with first-class integration into omarchy's notification and theming systems.

## Status

Early foundation. Working: account setup (IMAP + Gmail OAuth), folder/envelope sync with
IMAP IDLE, local SQLite cache, lazy body fetch, read-flag sync, mako notifications,
live omarchy theme adoption, 3-pane keyboard-driven UI. Not yet: composing/sending
(SMTP deps are wired, engine TODO), search, attachments, offline queueing.

## Omarchy integration

- **Notifications** are plain freedesktop D-Bus notifications rendered by mako, sent with
  `app-name "Cosmic Mail"` and a `desktop-entry` hint — so you can theme or script them in
  your mako config per app-name, and click-to-focus works. New-mail notifications are
  batched (>3 in one sync pass → a single summary), deduplicated via a per-folder
  high-water mark, and suppressed during an account's initial sync.
- **Theming**: the UI reads `~/.config/omarchy/current/theme/colors.toml` and watches for
  theme switches (`omarchy-theme-set`), re-tinting the whole app live — including rendered
  HTML mail that carries no styling of its own. Falls back to kanagawa.

## Development

```sh
npm install
npm run tauri dev      # run the app
npm run check          # svelte-check
cargo check            # from src-tauri/
```

System deps (Arch): `webkit2gtk-4.1 gtk3 librsvg openssl` + a running Secret Service
(omarchy default setup is fine).

### Gmail credentials

Gmail sign-in needs a Google OAuth client (Desktop type): create one at
<https://console.cloud.google.com/apis/credentials> with the Gmail API enabled, then
save it to `~/.config/cosmic-mail/google-oauth.json`:

```json
{ "clientId": "…apps.googleusercontent.com", "clientSecret": "…" }
```

(`clientSecret` optional; desktop-client secrets are non-confidential.) The
`COSMIC_MAIL_GOOGLE_CLIENT_ID`/`COSMIC_MAIL_GOOGLE_CLIENT_SECRET` env vars override the
file for dev, and packaged releases can bake defaults in at build time via
`COSMIC_MAIL_BUILD_GOOGLE_CLIENT_ID`/`COSMIC_MAIL_BUILD_GOOGLE_CLIENT_SECRET`.

The app performs an RFC 8252 loopback PKCE flow in your browser and stores only the
refresh token, in the Secret Service keyring (service `dev.pluscosmic.mail`). IMAP/SMTP
then authenticate with SASL XOAUTH2 — Gmail and plain IMAP share one sync engine.

## Architecture

See [docs/](docs/README.md) — overview, development guide, gotchas, and roadmap.
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) is the binding contract between the Rust
backend and the Svelte frontend (commands, events, wire types, DB schema, sync semantics).

```
src-tauri/src/          Rust core
  omarchy.rs            theme reader + watcher
  notifications.rs      mako/notify-rust integration
  store.rs              SQLite (WAL) message cache
  accounts.rs           accounts.json + keyring secrets
  sync/                 async-imap engine (IDLE, UIDVALIDITY, backoff)
  auth/oauth.rs         Gmail OAuth2 PKCE loopback flow
src/                    Svelte 5 (runes) frontend, 3-pane shell
prototypes/             static HTML design mockups (open in a browser)
```

## UI prototypes

Three design directions live in `prototypes/` (self-contained HTML, live theme switcher):
classic three-pane, keyboard-minimal (walker-style palette), and the hybrid icon-rail
layout the current shell is converging toward. See `prototypes/README.md`.

## Keyboard

`j`/`k` move · `gg`/`G` top/bottom · `Enter` open · `Esc` back · `r` toggle read
