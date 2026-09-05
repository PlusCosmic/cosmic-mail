# Gotchas — hard-won lessons

Every entry here cost real debugging time in this codebase. Read before touching the
related area. When you learn a new one, add it. Entries from the Rust/Tauri build that
still describe the *protocol* or *platform* (IMAP, discovery, WebKitGTK, Hyprland) are kept;
the crate-specific ones were replaced by their Go equivalents in the port.

## Dependencies / library APIs

- **Library APIs have drifted from what models remember.** Wails v3 (beta.16), go-imap v2
  (beta.8), go-message 0.18, go-keyring 0.2.8, modernc sqlite. **Do not write dep-facing
  code from memory** — read the module sources in `~/go/pkg/mod/` first. (go-imap v2:
  `Fetch(UIDSet, …)` issues `UID FETCH` on its own; `Move` falls back to COPY+STORE+EXPUNGE
  itself; a `UIDRange{Stop: 0}` renders as `n:*`; `IdleCommand.Close()` sends DONE and
  `Wait()` must follow it.)
- **Linux needs `-tags gtk3` on every Go command**, including `go install …/cmd/wails3`.
  Wails v3 otherwise links GTK4 + webkitgtk-6.0, which the Arch package does not depend on
  (and which the CI runner does not install). The root `Taskfile.yml` defaults it for
  `wails3 dev`/`wails3 build`; `go vet`, `go test`, `go build` need it spelled out. Wails
  says the tag goes away in v3.1 — plan the GTK4 switch before upgrading past 3.0.x.
- **`wails3 update build-assets` writes the GTK4 deps into `build/linux/nfpm/nfpm.yaml`**
  regardless of the tag. Re-check that file after regenerating assets.
- **A nil Go slice serialises as `null`,** and the generator types every slice `T[] | null`.
  Every list a service method or event carries is built non-nil (`models.NonNil`, or a
  `make(…, 0, n)`), and `frontend/src/lib/api.ts` narrows the generated types once. Don't
  null-check across the components.
- **`wails3 generate bindings` only discovers `RegisterEvent` calls with constant names in
  `init`** — `main.go` registers the four events with string literals for that reason. Data
  types are matched exactly at emit time (a `[]T` registered event rejects a `[]*T`).
- **Our package `internal/sync` shadows the stdlib `sync` name.** Import it as `mailsync` (and
  the stdlib as `gosync` where both are needed) or the mutex line stops compiling in a
  confusing way.
- **modernc sqlite pragmas are per connection.** The DSN carries `_pragma=journal_mode(WAL)`
  and `foreign_keys(ON)`, and the pool is pinned to one connection
  (`SetMaxOpenConns(1)`) behind the store mutex — an extra connection would silently have
  foreign keys off and the cascade tests would pass only by luck.

## TLS / auth

- **go-imap and go-smtp base64-encode the SASL initial response themselves.**
  `oauth.XOAuth2Client.Start()` must return the **raw** XOAUTH2 string
  (`user=<email>\x01auth=Bearer <token>\x01\x01`). Pre-encoding double-encodes and Gmail
  rejects it with `Invalid SASL argument`.
- Gmail `AUTHENTICATE` failing *after* a token was obtained = wire-format problem;
  failing at "obtaining Gmail access token" = missing client-id config or keyring issue.
- **Google OAuth clients in "Testing" publishing status expire refresh tokens after 7
  days.** The failure surfaces as `invalid_grant` on the *refresh exchange*, never at
  IMAP `AUTHENTICATE`. `internal/oauth` wraps exactly that case (plus a missing keyring
  entry, `accounts.ErrNoSecret`) with `ErrAuthExpired` so sync emits `needsReauth: true`
  and Settings offers in-place **Reconnect** (`ReauthGmailAccount`). Never tell users to
  remove and re-add the account for expired auth — that wipes their local mail cache for
  nothing. Don't broaden the classification: network/server/keyring-outage failures must
  stay plain retryable errors or a flaky network starts demanding pointless re-consents.
- **go-keyring only searches the login collection** (the Rust keyring searched all
  collections) and uses the same `service` + `username` attributes, so entries the Rust
  build stored in the default collection carry over; ones a user moved elsewhere do not.

## Platform / desktop

- **WebKitGTK + NVIDIA + Wayland/Hyprland crashes the display connection** (`Gdk Error 71
  (Protocol error)`) via the DMA-BUF renderer. `main.go` sets
  `WEBKIT_DISABLE_DMABUF_RENDERER=1` before Wails initialises GTK (respects an explicit user
  override). Wails only sets it itself when it detects an NVIDIA GPU; hybrid setups need it
  unconditionally. Don't remove it.
- **Hyprland 0.56 turned `hyprctl dispatch` into Lua.** `hyprctl dispatch focuswindow
  class:…` — the fallback the Rust build shipped — fails with exit 7 (`')' expected near
  'class'`) on Omarchy 4.x. `desktop.hyprlandFocus` tries `hl.dsp.focus({ window =
  "class:cosmic-mail" })` first and the classic form second, the same pair omarchy's own
  `omarchy-launch-or-focus` uses. Test focus fallbacks against the installed Hyprland, not
  against the wiki.
- **A background window focus call can be ignored by Hyprland/Wayland focus-stealing
  prevention.** Activation first unminimises/shows/focuses portably, then runs the fixed-argument
  `hyprctl` fallback above when `HYPRLAND_INSTANCE_SIGNATURE` is present. Test actions on a
  currently-live notification: restored mako history belongs to the old listener and cannot
  validate the callback.
- **fsnotify does not report access events** (inotify's IN_ACCESS is not subscribed), so the
  omarchy watcher re-reading theme files from inside its own handler cannot re-trigger itself —
  the feedback loop the Rust `notify` crate had. The change-dedupe (compare the parsed theme to
  the last one) is still required: the watcher fires for unrelated churn in the dir.
- omarchy's `current/theme` is a **symlink swapped atomically** — watch the parent dir
  `current` non-recursively; a real theme switch arrives as rename/create events.
- **omarchy moved `current` from `~/.config/omarchy/current` to
  `$XDG_STATE_HOME/omarchy/current`** (`~/.local/state/omarchy/current`). Watching only
  the config-dir path silently logged `omarchy current dir not present; theme watcher
  inactive` and the app stayed on kanagawa. `omarchy.CurrentDir` prefers the state dir and
  falls back to the config dir only when that alone exists.
- **omarchy's `colors.toml` no longer has `color0..color15` / `cursor` / `selection_*`**;
  it has named colours (`red`, `bright_red`, `muted`, `darker_background`, `selection`,
  ...). Only `accent`/`foreground`/`background` overlap with the old dialect, so parsing
  the old keys alone leaves the whole palette on the kanagawa fallback while the base
  colours track the theme. See [OMARCHY.md](OMARCHY.md#live-theming) for the mapping;
  check `/usr/share/omarchy/themes/*/colors.toml` before adding a key.
- **`Window.ExecJS` has no return path, and Wails queues it until the runtime reports ready.**
  The automation bridge gets results back through the webview's raw message channel
  (`RawMessageHandler`), and its `/health` gates on a real eval round-trip for exactly this
  reason; never weaken it back to a window-existence check.
- **Wails' Linux single instance is a D-Bus name too** (`org.wails_app_dev_pluscosmic_mail
  .SingleInstance`) — different from the Tauri daily's `dev.pluscosmic.mail.SingleInstance`,
  so a Go dev build and an installed Rust daily do *not* see each other. Stop the daily before
  testing ownership regardless; both would sync the same accounts.
- **The tray needs a StatusNotifier host.** Under `dbus-run-session` (the e2e harness) there
  is none and Wails logs `systray error: failed to register`; that is expected there and must
  stay non-fatal. On the real session bus the item registers as
  `…/org/ayatana/NotificationItem/tray_icon_tray_app_cosmic_mail`.
- **WebKitGTK never exposes a sandboxed `srcdoc` iframe's document to its parent** — even
  with `sandbox="allow-same-origin"`, `contentDocument` goes null once the real document
  loads (and the frame fires `load` more than once). Chromium happily allows it, so the
  delegated-click-listener pattern from web articles silently does nothing here. Anything
  that must observe events inside the reader frame has to run *inside* it (injected
  script + `postMessage`, with `sandbox="allow-scripts"` for an opaque origin — see the
  reader links bullet in ARCHITECTURE.md). Verify iframe behavior in a WebKit2GTK
  harness, not a Chromium browser.

## Mail parsing

- **go-message is stricter than mail-parser.** `message.Read` can fail on a malformed
  header block; `mailparse.Parse` treats that as a single text/plain body holding the raw
  bytes rather than dropping the message. Unknown charsets/encodings come back as errors that
  still carry a readable entity — check `IsUnknownCharset`/`IsUnknownEncoding` before
  treating them as failures.
- **`part_index` must keep mail-parser's flat DFS numbering** (root = 0, containers count),
  or attachments cached by the Rust build re-extract the wrong part. `mailparse.walk` and
  its tests pin this.
- **Chrono's `to_rfc3339` wrote `+00:00`, not `Z`.** `store.RFC3339` formats dates the same
  way so new rows sort next to the old ones; `time.RFC3339` would not.

## IMAP sync

- **Do not implement the 200-message initial-sync cap as `UID FETCH 1:*` followed by a local
  trim.** A 1,497-message Purelymail inbox proved that this downloads/parses the whole mailbox
  and leaves the UI looking empty until the work completes. Use dense message sequence numbers
  from `STATUS MESSAGES` to `FETCH max(1, exists-199):*`, requesting `UID` in the response.
- Folder `total_count` / `unread_count` are authoritative server `STATUS MESSAGES` / `UNSEEN`
  values, not counts of locally cached rows. A successful local `\\Seen` change adjusts unread
  by one; the next STATUS reconciles it. Never recompute these columns from the capped cache.
- **A plain TCP socket with no keepalive makes a dead socket byte-for-byte indistinguishable
  from a quiet mailbox — for as long as whatever wall-clock deadline eventually forces a
  reconnect.** (Issue #41.) Laptop suspend/resume, wifi roam, VPN flap, NAT idle-timeout, and a
  server dropping the connection without a FIN/RST all leave the client sitting in `IDLE`
  receiving nothing, and nothing about the TCP/TLS/IMAP layers tells it the peer is gone — the
  socket just looks idle. Before this fix the only bound was `FullResyncInterval` (25 min), so
  every laptop resume bought up to a 25-minute window of "notifications have silently stopped."
  This is exactly the class of invisible transport assumption this file exists for: nothing in
  the type signatures hints that the connection can go silently dark without an error, so it is
  easy to ship a sync loop that is provably correct about *protocol* handling (drain-checks, UID
  ranges, etc. — see the entries above and issue #39/#40) while still being blind to *transport*
  death. The fix has two independent layers, on purpose — neither alone closes the window:
  `SO_KEEPALIVE` on the socket (`net.Dialer.KeepAliveConfig` in `imap.Connect`, before the TLS
  handshake — 60s idle / 10s interval / 3 probes, so a dead peer surfaces as an IO error in ~90s)
  catches a transport that is *dead*; splitting the IDLE re-issue cadence (5 min) from the
  full-resync deadline (25 min, previously the same constant) bounds how long any single command
  sits unexercised even when keepalive doesn't fire for some reason (e.g. a middlebox that eats
  keepalive probes but still passes data). Re-issuing IDLE without also draining the
  mailbox-changed flag first would reopen the exact race issue #39 closed — a `DONE`/`IDLE` round
  trip is itself a window an `EXISTS` can land in — so the re-issue path reuses the same
  drain-check as every other path into IDLE.
- **go-imap's `UnilateralDataHandler.Mailbox` fires from the reader goroutine** while a command
  may be in flight. It must only set a flag and do a non-blocking channel send
  (`internal/imap`); anything that calls back into the client from there deadlocks.

## Autoconfig discovery

- **HTTP 200 ≠ valid autoconfig.** Websites with SPA catch-alls return 200 + HTML for
  `/.well-known/autoconfig/...` (pluscosmic.dev does). The XML parser must be the
  gatekeeper — never trust status codes alone.
- **ISPDB 404 means "domain not listed", not "wrong URL"** — and big providers are
  missing: fastmail.com is NOT in the ISPDB (they self-host `autoconfig.fastmail.com`).
  Test ISPDB code against `gmx.de` or `gmail.com`, not fastmail.
- **Custom domains often resolve only via the MX provider's own autoconfig endpoint**
  (`https://autoconfig.{mx-registrable-domain}/mail/config-v1.1.xml?emailaddress=…`) —
  e.g. Purelymail. That's chain step 4a, before the ISPDB retry; Thunderbird does the same.
- Our IMAP connector is **implicit-TLS only** — discovery must skip STARTTLS-only IMAP
  entries (returns no config if no SSL entry) until STARTTLS support lands.

## Wails / wire format

- **Every wire-crossing struct needs camelCase `json` tags** — including event payload
  structs, which is exactly where it was forgotten once in the Rust build (frontend read
  `accountId`, wire had `account_id`; events silently did nothing). Keep every such struct in
  `internal/models` so the generator sees it; when adding an event, register it in `main.go`
  and match the name in `frontend/src/lib/api.ts`.
- **Service method parameters keep their Go names in the bindings** (`messageID`, not
  `message_id`); `api.ts` is where the frontend's camelCase wrappers live. Prefer scalar
  parameters over ad-hoc request structs so the TS signature stays readable.
- **Only `wails3 dev` uses the Vite dev server.** Both `wails3 task build` and `… DEV=true`
  embed `frontend/dist` — there is no longer a "dev-mode binary that shows `about:blank`
  without Vite". What `DEV=true` (or a plain `go build -tags gtk3`) changes is the
  `production` tag: without it the automation bridge and the fixture env hooks are compiled in.
