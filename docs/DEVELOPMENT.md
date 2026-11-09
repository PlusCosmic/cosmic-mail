# Development guide

## Commands

```sh
cargo run                       # run the app (window)
cargo run -- --background       # no window: tray + background sync only
cargo test                      # unit tests + UI behaviour tests (fast, no network)
cargo test -- --ignored         # live-network tests (ISPDB, provider autoconfig, DNS) + E2E
cargo clippy --all-targets -- -D warnings
cargo fmt
cargo build --release           # → target/release/cosmic-mail
```

The toolchain is pinned to 1.99.1 by `rust-toolchain.toml` (Rune requires it); rustup
installs it on first use. Rune itself is pinned by git rev in `Cargo.toml` — bump the rev
deliberately, never track `dev`.

Build dependencies: Arch `libxkbcommon wayland vulkan-icd-loader` plus a Vulkan driver;
Ubuntu `libxkbcommon-dev libwayland-dev libdbus-1-dev` (and `libxkbcommon-x11-0` for
windowed runs under X11). **Python 3** on `PATH` (`python3`; `python.exe` on Windows) —
Stylo, the CSS engine behind the HTML mail layout, generates code with it at build time;
set `PYTHON3=/path/to/python` if it lives elsewhere (see GOTCHAS, "Blitz").

### Demo window (debug builds)

`COSMIC_MAIL_DEMO=1 cargo run [-- --scene NAME]` opens a real window on the demo mailbox,
with no profile, network, tray or single-instance hand-off, so it runs beside an installed
Cosmic Mail. Use it to try window behaviour by hand (titlebar drag, snap layouts, caption
buttons, resizing). Release builds ignore the variable.

### Headless renders (Rune tooling)

The binary doubles as Rune's headless tool. These runs render the built-in demo mailbox
(`src/ui/demo.rs`) and never touch the profile, the network, or the single-instance
socket:

```sh
cargo run -- --list-scenes                       # default welcome folder unread search compose
                                                 # reply palette move add-account settings toasts light
cargo run -- --snap out.png --scene palette --scale 2
cargo run -- --tree tree.json --scene default    # element tree: keys, roles, text, rects
cargo run -- --script s.json --out frames/       # scripted input (see Rune's README)
```

Without a GPU, install `mesa-vulkan-drivers` and set
`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json` (lavapipe). Every UI change should
come with a `--snap` of the affected scene, checked by eye.

### Lifecycle

One process per session: the first launch binds `$XDG_RUNTIME_DIR/cosmic-mail.sock`; a
later launch (including `--background`) hands its request over the socket and exits, and
the running process shows and focuses its window. Closing the window hides it without
stopping sync; **Quit** in the tray ends the process.

If your install runs Cosmic Mail as `cosmic-mail.service` (bound to
`graphical-session.target`, started with `--background`), a `cargo run` while it is
active just activates the service's window and exits. Stop it to test a dev build:

```sh
systemctl --user stop cosmic-mail.service
journalctl --user -u cosmic-mail.service -f
```

## Platform and environment specifics

- omarchy is installed; mako is the notification daemon; current theme files live at
  `~/.local/state/omarchy/current/theme/` (symlink swapped by `omarchy-theme-set`;
  `~/.config/omarchy/current/` on older releases).
- The window (1280×800, min 900×560) draws its own titlebar band on Windows and macOS (search
  lives in it there). On Linux it has the platform title bar unless Settings → Window →
  "Show the window title bar" is off (read at startup), with app id / X11 class
  `cosmic-mail`, which Hyprland rules and the `hyprctl` focus fallback match on.
- Gmail OAuth credential setup and precedence are documented in [SETUP.md](SETUP.md#gmail).
  Without a credential source, Gmail sign-in and token refresh fail with a clear error;
  `obtaining Gmail access token` warnings mean credentials are missing in that process.

## Local state (wipe these to reset the app)

| what | where |
|---|---|
| account configs (no secrets) | `~/.config/cosmic-mail/accounts.json` |
| global preferences (no secrets) | `~/.config/cosmic-mail/settings.json` |
| UI preferences (pane split) | `~/.config/cosmic-mail/ui.json` |
| message cache (SQLite, WAL) | `~/.local/share/cosmic-mail/mail.db` |
| secrets | Secret Service keyring, service `dev.pluscosmic.mail`, keys `imap-password:<id>` / `oauth-refresh-token:<id>` |

Inspect sync progress without the UI:
`sqlite3 ~/.local/share/cosmic-mail/mail.db 'SELECT name, role, total_count, unread_count FROM folders;'`

## Testing each subsystem

- **Appearance**: Settings → Appearance; each scheme card and System/Light/Dark re-colours
  the window at once and survives a restart (`ui.json`). `--scene oxblood|ink|walnut|graphite|light`
  renders the others headless.
- **Notifications**: `Backend::test_notification` sends a sample through mako.
  Real-path test: with the app running and synced, send yourself a mail; IDLE should
  produce a mako notification within seconds. Batching: >3 new in one sync pass → one
  summary notification. Test click-to-focus on the live notification before it expires;
  restoring a mako history item cannot reach the original app-side action listener.
- **Background lifecycle**: start with `--background`, verify there is no Cosmic Mail
  window, then launch it twice from Walker (one window, one PID). Close the window and
  verify the PID remains; a live notification action or another Walker launch must reveal and
  focus that same window. `pgrep -a cosmic-mail` and `hyprctl clients` are useful corroborating
  checks.
- **System tray**: with a desktop tray host running, start normally and with `--background`; in
  both cases expect one Cosmic Mail tray icon with **Open Cosmic Mail**, **Sync now**, and **Quit**.
  Open must reveal/focus the existing window. Sync now must emit a new syncing state for every
  configured account without creating another process. Quit must cleanly stop the process; under
  `cosmic-mail.service`, verify it remains inactive rather than restarting.
- **Pane layout**: drag the separator between the message list and reader, switch between unified
  and account/folder views, and resize or maximize the window. Both panes must remain usable and
  the reader must receive surplus wide-screen space. Restart the app to verify persistence
  (`~/.config/cosmic-mail/ui.json`). With the separator focused, test Left/Right (Shift for
  larger steps), Home/End, and double-click reset.
- **UI behaviour**: `cargo test` runs `src/ui/tests.rs` — the real view and key handling on
  Rune's headless `Runtime` over the demo mailbox (j/k/gg/G, filters, search, palette, compose
  validation, reply seeding, optimistic flag/read/delete, live events, toasts, scheme and appearance,
  every scene renders). The GreenMail E2E (`tests/e2e_greenmail.rs`, see `e2e/README.md`)
  drives the same view over the real backend and IMAP.
- **Compose/send**: automated tests validate construction only and never send external mail. For a
  live check, send through one Gmail and one password account, verify receipt and provider-side Sent
  behavior, then test `c`, `r`, Reply all, Escape, and Ctrl+Enter from the compose dialog.
- **Body prefetch/privacy**: after an inbox sync, recent/unread messages up to 1 MiB should open
  immediately without changing unread state before selection. HTML bodies render as styled text
  (`src/ui/html.rs`): scripts, styles and forms never run, and only `http(s)` links open, in the
  browser. Open an HTML message with a controlled remote image and verify no request occurs until
  `Load remote images` is pressed; selecting another message must restore the default block.
  `cargo test` covers the HTML conversion and the remote-image banner.
- **Search**: `cargo test --lib` covers the store layer — FTS5 availability, the MATCH-expression
  builder (quoting/escaping, operators as literals), per-column and prefix matching, account
  scoping, trigger sync (rows appear after `upsert_message`/`set_body`, disappear on delete). For a
  live check: sync an account, type a term in the header field and press Enter — the list shows
  relevance-ranked results across the current scope (all accounts in the unified inbox, one account
  in account view) and matches cached bodies too; Escape or the header's clear control restores the
  normal view. `sqlite3 ~/.local/share/cosmic-mail/mail.db "SELECT count(*) FROM messages_fts;"`
  should track the cached message count.
- **Attachments**: `cargo test --lib` covers extraction (metadata incl. RFC 2047 filenames, mime,
  size, inline flag, content-id), the deterministic part index, the cid→data rewrite happening
  under caps and being skipped over the per-part/total budgets, `has_attachments` reconciliation
  from the real parse, filename sanitization, and non-overwriting collision naming (temp dir).
  `cargo test` covers `format_bytes`. For a live check: open a message with an attachment, click its
  chip in the reader, and confirm a byte-identical file appears in the downloads directory (a
  second save suffixes `name (1).ext`); confirm an inline `cid:` image renders in the body while an
  over-cap inline image renders blank without any network request.
- **Message actions**: `cargo test --lib` covers the store layer — `set_flagged`, `remove_message`
  count adjustments (seen vs. unseen, floored at 0) with the FTS delete trigger and attachment
  cascade, `find_folder_by_role`, and `message_action_context`. `cargo test` covers the
  next-selection helper and the optimistic UI paths (the E2E covers a real failed delete rolling back). The IMAP round-trips cannot be unit-tested without a server. Live check on
  **both** account kinds (Gmail advertises `MOVE`; a plain-IMAP provider such as Purelymail may
  exercise the `UID COPY` + `\Deleted` + `UID EXPUNGE`, or plain-`EXPUNGE`, fallback): with a synced
  account, press `f` to flag (star round-trips to the server), `a` to archive (Gmail: lands in All
  Mail and leaves the inbox), `d` to delete (moves to Trash; a second `d` from within Trash deletes
  permanently), and `m` to open the folder picker and move. Confirm the selected row is removed
  locally, the next message is selected, folder totals/unread settle after the next STATUS, and the
  message reappears in the destination on its next sync with no duplicates.
- **Discovery**: `cargo test --lib -- --ignored` covers ISPDB (gmx.de), provider
  autoconfig (fastmail.com), and the MX→provider path (pluscosmic.dev/Purelymail).
- **Gmail e2e**: verified working 2026-07-09 (OAuth → XOAUTH2 → folder LIST → envelope
  sync → UI). Do not wipe local account data unless the test explicitly requires it.

## Logging

`tracing` with env-filter; default `cosmic_mail=info,warn`. Override:
`RUST_LOG=cosmic_mail=debug cargo run`. Never log secrets or tokens.
