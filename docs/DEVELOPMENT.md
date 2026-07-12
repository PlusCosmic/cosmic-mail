# Development guide

## Commands

```sh
# repo root
npm install            # once
npm run tauri dev      # run the app (vite on :1420 + cargo build + window)
npm test               # pure frontend logic tests (node:test)
npm run check          # svelte-check — must be 0 errors
npm run build          # static build incl. prerender — must pass
npm run promote:local  # verify, build, and manually promote the launcher-visible daily build

# src-tauri/
cargo check
cargo clippy           # keep at zero warnings
cargo test --lib       # unit tests (fast, no network)
cargo test --lib -- --ignored   # live-network tests (ISPDB, provider autoconfig, DNS)
cargo fmt
```

See [RELEASING.md](RELEASING.md) for installation layout, rollback, and the explicit
promotion policy.

Full debug binary: `cargo build` → `src-tauri/target/debug/cosmic-mail` (debug builds
expect the vite dev server; use `npm run tauri dev` normally).

The promoted daily build runs as `cosmic-mail.service` for the lifetime of
`graphical-session.target`. It starts with `--background`, so no window is mapped until the
launcher or a live notification action activates it. Closing the window hides it without stopping
sync. Useful checks after promotion:

```sh
systemctl --user status cosmic-mail.service
systemctl --user restart cosmic-mail.service
journalctl --user -u cosmic-mail.service -f
```

Do not run `npm run tauri dev` at the same time as the daily service when testing ownership: both
builds use the same application identifier, and the first process to own the session D-Bus name is
the one that receives launcher activations and runs sync.

## Platform and environment specifics

- On NVIDIA/Wayland systems, WebKitGTK may need `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
  `main.rs` sets it automatically. Don't remove it.
- omarchy is installed; mako is the notification daemon; current theme files live at
  `~/.config/omarchy/current/theme/` (symlink swapped by `omarchy-theme-set`).
- Gmail OAuth credential setup and precedence are documented in [SETUP.md](SETUP.md#gmail).
  Without a credential source, Gmail sign-in and token refresh fail with a clear error;
  `obtaining Gmail access token` warnings mean credentials are missing in that process.
- Port 1420 is vite's fixed dev port — a stray vite from a previous session blocks new
  `tauri dev` runs (`pkill -f 'vite dev'`). Also check for orphaned
  `target/debug/cosmic-mail` processes after killing dev sessions.

## Local state (wipe these to reset the app)

| what | where |
|---|---|
| account configs (no secrets) | `~/.config/cosmic-mail/accounts.json` |
| message cache (SQLite, WAL) | `~/.local/share/cosmic-mail/mail.db` |
| secrets | Secret Service keyring, service `dev.pluscosmic.mail`, keys `imap-password:<id>` / `oauth-refresh-token:<id>` |

Inspect sync progress without the UI:
`sqlite3 ~/.local/share/cosmic-mail/mail.db 'SELECT name, role, total_count, unread_count FROM folders;'`

## Testing each subsystem

- **Theme integration**: run `omarchy-theme-set <name>` while the app is running — expect
  exactly one `emitted omarchy theme change` log line and a full UI re-tint. (Watcher
  events are access-filtered and change-deduped; see GOTCHAS.)
- **Notifications**: the `test_notification` command sends a sample through mako.
  Real-path test: with the app running and synced, send yourself a mail; IDLE should
  produce a mako notification within seconds. Batching: >3 new in one sync pass → one
  summary notification. Test click-to-focus on the live notification before it expires;
  restoring a mako history item cannot reach the original app-side action listener.
- **Background lifecycle**: promote a build, verify the service is active with no Cosmic Mail
  window, then launch it twice from Walker (one window, one service PID). Close the window and
  verify the PID remains; a live notification action or another Walker launch must reveal and
  focus that same window. `pgrep -a cosmic-mail` and `hyprctl clients` are useful corroborating
  checks.
- **Pane layout**: drag the separator between the message list and reader, switch between unified
  and account/folder views, and resize or maximize the window. Both panes must remain usable and
  the reader must receive surplus wide-screen space. Restart the app to verify persistence. With
  the separator focused, test Left/Right (Shift for larger steps), Home/End, and double-click reset.
- **Compose/send**: automated tests validate construction only and never send external mail. For a
  live check, send through one Gmail and one password account, verify receipt and provider-side Sent
  behavior, then test `c`, `r`, Reply all, Escape, and Ctrl+Enter from the compose dialog.
- **Body prefetch/privacy**: after an inbox sync, recent/unread messages up to 1 MiB should open
  immediately without changing unread state before selection. Open an HTML message with a controlled
  remote image and verify no request occurs until `Load remote images` is pressed; selecting another
  message must restore the default block. `npm test` covers the generated iframe resource policy.
- **Discovery**: `cargo test --lib -- --ignored` covers ISPDB (gmx.de), provider
  autoconfig (fastmail.com), and the MX→provider path (pluscosmic.dev/Purelymail).
- **Gmail e2e**: verified working 2026-07-09 (OAuth → XOAUTH2 → folder LIST → envelope
  sync → UI). Do not wipe local account data unless the test explicitly requires it.

## Logging

`tracing` with env-filter; default `cosmic_mail_lib=info,warn`. Override:
`RUST_LOG=cosmic_mail_lib=debug npm run tauri dev`. Never log secrets or tokens.
