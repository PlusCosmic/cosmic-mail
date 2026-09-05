# Development guide

## Commands

```sh
# once
go install -tags gtk3 github.com/wailsapp/wails/v3/cmd/wails3@v3.0.0-beta.16
(cd frontend && npm install)

# repo root
wails3 dev             # run the app (vite on :9245 + go build + window, live reload)
wails3 task build      # production binary with the frontend embedded -> bin/cosmic-mail
wails3 task build DEV=true   # development binary (automation bridge + test hooks) -> bin/cosmic-mail
wails3 generate bindings -ts -i -clean=true -d frontend/bindings -f "-tags gtk3"
gofmt -l .             # must print nothing
go vet -tags gtk3 ./...
go test -tags gtk3 ./...             # unit + hermetic integration tests (in-memory IMAP server)
COSMIC_MAIL_LIVE_TESTS=1 go test -tags gtk3 ./internal/autoconfig/   # live-network discovery tests

# frontend/
npm run check          # svelte-check — must be 0 errors
npm run build          # static build incl. prerender — must pass
npm test               # pure frontend logic tests (node:test)
npm run promote:local  # = scripts/promote-local.sh: verify, build, and promote the daily build
```

Every Go command needs `-tags gtk3` (the root `Taskfile.yml` defaults it for `wails3`
commands): Wails 3 otherwise links GTK4 + WebKitGTK 6.0, and the Arch package depends on
webkit2gtk-4.1. `frontend/bindings` is generated and committed; regenerate it whenever
`app.go` or `internal/models` changes — CI diffs it.

See [RELEASING.md](RELEASING.md) for installation layout, rollback, and the explicit
promotion policy.

Build modes: `wails3 dev` and `wails3 task build DEV=true` compile without the `production`
tag, which keeps the automation bridge and the `COSMIC_MAIL_EXTRA_CA` /
`COSMIC_MAIL_TEST_IMAP_PASSWORD` hooks in (`internal/buildinfo`). `wails3 task build` sets
`production`, compiling them out. Both embed `frontend/dist/app` (the tracked `frontend/dist/.gitkeep`
keeps the Go checks working before a frontend build); there is no separate
"dev-mode binary that needs a Vite server" — only `wails3 dev` uses the dev server, via
`FRONTEND_DEVSERVER_URL`.

The promoted daily build runs as `cosmic-mail.service` for the lifetime of
`graphical-session.target`. It starts with `--background`, so no window is mapped until the
launcher or a live notification action activates it. Closing the window hides it without stopping
sync. Useful checks after promotion:

```sh
systemctl --user status cosmic-mail.service
systemctl --user restart cosmic-mail.service
journalctl --user -u cosmic-mail.service -f
```

Do not run `wails3 dev` at the same time as the daily service when testing ownership: both
builds use the same single-instance identifier, and the first process to own it is the one
that receives launcher activations and runs sync.

## Platform and environment specifics

- On NVIDIA/Wayland systems, WebKitGTK may need `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
  `main.go` sets it automatically. Don't remove it.
- omarchy is installed; mako is the notification daemon; current theme files live at
  `~/.config/omarchy/current/theme/` (symlink swapped by `omarchy-theme-set`).
- Gmail OAuth credential setup and precedence are documented in [SETUP.md](SETUP.md#gmail).
  Without a credential source, Gmail sign-in and token refresh fail with a clear error;
  `obtaining Gmail access token` warnings mean credentials are missing in that process.
- Port 9245 is vite's fixed dev port for `wails3 dev` — a stray vite from a previous session
  blocks new runs (`pkill -f 'vite dev'`). Also check for orphaned `bin/cosmic-mail`
  processes after killing dev sessions.

## Local state (wipe these to reset the app)

| what | where |
|---|---|
| account configs (no secrets) | `~/.config/cosmic-mail/accounts.json` |
| global preferences (no secrets) | `~/.config/cosmic-mail/settings.json` |
| message cache (SQLite, WAL) | `~/.local/share/cosmic-mail/mail.db` |
| secrets | Secret Service keyring, service `dev.pluscosmic.mail`, keys `imap-password:<id>` / `oauth-refresh-token:<id>` |

Inspect sync progress without the UI:
`sqlite3 ~/.local/share/cosmic-mail/mail.db 'SELECT name, role, total_count, unread_count FROM folders;'`

## Testing each subsystem

- **Sync engine, hermetically**: `go test -tags gtk3 ./internal/sync/` runs the whole engine
  against go-imap's in-memory IMAP server over TLS (`internal/testimap`): initial sweep, folder
  roles, events, body prefetch with shipment detection, and a message arriving during IDLE
  (wakeup + notification). `go run ./cmd/fakeimap -ca-out /tmp/ca.pem` serves the same fixture
  mailbox on `127.0.0.1:3993` for the UI e2e suite without Docker — see `e2e/README.md`.
- **Theme integration**: run `omarchy-theme-set <name>` while the app is running — expect
  exactly one `emitted omarchy theme change` log line and a full UI re-tint. (Watcher
  events are debounced and change-deduped; see GOTCHAS.)
- **Notifications**: the `TestNotification` command sends a sample through mako.
  Real-path test: with the app running and synced, send yourself a mail; IDLE should
  produce a mako notification within seconds. Batching: >3 new in one sync pass → one
  summary notification. Test click-to-focus on the live notification before it expires;
  restoring a mako history item cannot reach the original app-side action listener.
- **Background lifecycle**: promote a build, verify the service is active with no Cosmic Mail
  window, then launch it twice from Walker (one window, one service PID). Close the window and
  verify the PID remains; a live notification action or another Walker launch must reveal and
  focus that same window. `pgrep -a cosmic-mail` and `hyprctl clients` are useful corroborating
  checks.
- **System tray**: with a desktop tray host running, start normally and with `--background`; in
  both cases expect one Cosmic Mail tray icon with **Open Cosmic Mail**, **Sync now**, and **Quit**.
  `busctl --user get-property org.kde.StatusNotifierWatcher /StatusNotifierWatcher
  org.kde.StatusNotifierWatcher RegisteredStatusNotifierItems` lists the registration. Open must
  reveal/focus the existing window. Sync now must emit a new syncing state for every configured
  account without creating another process. Quit must cleanly stop the process; for a promoted
  build, verify `cosmic-mail.service` remains inactive rather than restarting it.
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
- **Search**: `go test ./internal/store/` covers the store layer — FTS5 availability, the
  MATCH-expression builder (quoting/escaping, operators as literals), per-column and prefix
  matching, account scoping, trigger sync (rows appear after `UpsertMessage`/`SetBody`, disappear
  on delete). For a live check: sync an account, type a term in the header field and press Enter —
  the list shows relevance-ranked results across the current scope (all accounts in the unified
  inbox, one account in account view) and matches cached bodies too; Escape or the header's clear
  control restores the normal view. `sqlite3 ~/.local/share/cosmic-mail/mail.db "SELECT count(*)
  FROM messages_fts;"` should track the cached message count.
- **Attachments**: `go test ./internal/mailparse/ ./internal/attachments/` covers extraction
  (metadata incl. RFC 2047 filenames, mime, size, inline flag, content-id), the deterministic part
  index, the cid→data rewrite happening under caps and being skipped over the per-part/total
  budgets, `has_attachments` reconciliation from the real parse, filename sanitization, and
  non-overwriting collision naming (temp dir). `npm test` covers `formatBytes`. For a live check:
  open a message with an attachment, click its chip in the reader, and confirm a byte-identical
  file appears in the downloads directory (a second save suffixes `name (1).ext`); confirm an
  inline `cid:` image renders in the body while an over-cap inline image renders blank without any
  network request.
- **Message actions**: `go test ./internal/store/` covers the store layer — `SetFlagged`,
  `RemoveMessage` count adjustments (seen vs. unseen, floored at 0) with the FTS delete trigger and
  attachment cascade, `FindFolderByRole`, and `MessageActionContext`. `npm test` covers the
  next-selection helper. Live check on **both** account kinds (Gmail advertises `MOVE`; a
  plain-IMAP provider such as Purelymail may exercise the `UID COPY` + `\Deleted` + `UID EXPUNGE`,
  or plain-`EXPUNGE`, fallback): with a synced account, press `f` to flag (star round-trips to the
  server), `a` to archive (Gmail: lands in All Mail and leaves the inbox), `d` to delete (moves to
  Trash; a second `d` from within Trash deletes permanently), and `m` to open the folder picker and
  move. Confirm the selected row is removed locally, the next message is selected, folder
  totals/unread settle after the next STATUS, and the message reappears in the destination on its
  next sync with no duplicates.
- **Discovery**: `COSMIC_MAIL_LIVE_TESTS=1 go test ./internal/autoconfig/` covers ISPDB (gmx.de),
  provider autoconfig (fastmail.com), and the MX→provider path (pluscosmic.dev/Purelymail).
- **Gmail e2e**: verified working 2026-07-09 on the Rust build (OAuth → XOAUTH2 → folder LIST →
  envelope sync → UI); the Go port has not yet been confirmed against a live Gmail account. Do not
  wipe local account data unless the test explicitly requires it.

## Logging

`log/slog` on stderr; default level info. Override with `COSMIC_MAIL_LOG=debug` (also `warn`,
`error`): `COSMIC_MAIL_LOG=debug wails3 dev`. Never log secrets or tokens.
