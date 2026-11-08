# Gotchas — hard-won lessons

Every entry here cost real debugging time in this codebase. Read before touching the
related area. When you learn a new one, add it.

## Dependencies / crate APIs

- **Crate APIs have drifted from what models remember.** keyring 4 (keyring-core 1.0),
  oauth2 5 (typestate client), reqwest 0.13 (`rustls-tls` feature renamed to `rustls`),
  notify 8, async-imap 0.11, hickory-resolver 0.26. **Do not write dep-facing code from
  memory** — read the vendored sources in `~/.cargo/registry/src/*/<crate>-<version>/`
  first. (hickory 0.26: `TokioResolver::builder_tokio()?.build()?`; records expose the
  public **field** `record.data`, not a method.)
- **keyring 4 needs the `v1` feature** for the ergonomic `Entry` API with automatic
  Secret Service setup; the granular `zbus-secret-service-keyring-store` feature alone
  only enables the backend dep without that API.
- **`cargo add ... | tail` masks failures** — the pipeline exit code is `tail`'s. Two
  deps silently didn't get added this way once. Check `Cargo.toml` after batched adds.
- **socket2 0.6's `TcpKeepalive` setters are per-platform cfg-gated**, not just
  no-ops on unsupported targets: `with_retries` (`TCP_KEEPCNT`) additionally requires the
  crate's `all` feature to even exist, and `with_interval`/`with_retries` are absent
  entirely on some `target_os`es (see the `#[cfg(...)]` lists in the vendored
  `socket2-0.6.4/src/lib.rs`). Enable `features = ["all"]` on the `socket2` dep, or
  `.with_retries(...)` won't compile — this project only targets Linux, where all three
  setters (`with_time`/`with_interval`/`with_retries`) are available.

## TLS / auth

- **rustls panics at the first TLS connection, not at startup**, when both crypto
  backends are in the tree (`lettre` → `ring`, `reqwest`/`oauth2` → `aws-lc-rs`):
  "Could not automatically determine the process-level CryptoProvider". Fix lives at the
  top of `app.rs::run()`: `rustls::crypto::ring::default_provider().install_default()`
  (the E2E harness installs it too).
  If you add/remove TLS-using deps, keep that line, and remember a panic in a tokio
  worker **kills the task silently, not the app** — the UI keeps running while sync is dead.
- **async-imap base64-encodes SASL authenticator responses itself.** `Authenticator::
  process()` must return the **raw** XOAUTH2 string (`user=<email>\x01auth=Bearer
  <token>\x01\x01`). Pre-encoding double-encodes and Gmail rejects it with
  `Invalid SASL argument` (its own test suite proves the encoding: `"foo"` → `Zm9v`).
- Gmail `AUTHENTICATE` failing *after* a token was obtained = wire-format problem;
  failing at "obtaining Gmail access token" = missing client-id env or keyring issue.
- **Google OAuth clients in "Testing" publishing status expire refresh tokens after 7
  days.** The failure surfaces as `invalid_grant` on the *refresh exchange*, never at
  IMAP `AUTHENTICATE`. `auth/oauth.rs` tags exactly that case (plus a missing keyring
  entry) with the `AuthExpired` marker so sync emits `needsReauth: true` and Settings
  offers in-place **Reconnect** (`Backend::reauth_gmail_account`). Never tell users to remove and
  re-add the account for expired auth — that wipes their local mail cache for nothing.
  Don't broaden the classification: network/server/keyring-outage failures must stay
  plain retryable errors or a flaky network starts demanding pointless re-consents.

## Platform / desktop

- **The `notify` crate reports file *access* events on inotify.** A watcher whose
  handler *reads files inside the watched directory* re-triggers itself forever (ours
  looped at exactly the 300ms debounce period). `omarchy.rs` filters
  `event.kind.is_access()` AND dedupes by comparing the parsed theme to the last one.
  Apply the same pattern to any new watcher.
- omarchy's `current/theme` is a **symlink swapped atomically** — watch the parent dir
  `current` non-recursively; a real theme switch arrives as rename/create events.
- **omarchy moved `current` from `~/.config/omarchy/current` to
  `$XDG_STATE_HOME/omarchy/current`** (`~/.local/state/omarchy/current`). Watching only
  the config-dir path silently logged `omarchy current dir not present; theme watcher
  inactive` and the app stayed on kanagawa. `omarchy::current_dir` prefers the state dir
  and falls back to the config dir only when that alone exists.
- **omarchy's `colors.toml` no longer has `color0..color15` / `cursor` / `selection_*`**;
  it has named colours (`red`, `bright_red`, `muted`, `darker_background`, `selection`,
  ...). Only `accent`/`foreground`/`background` overlap with the old dialect, so parsing
  the old keys alone leaves the whole palette on the kanagawa fallback while the base
  colours track the theme. See [OMARCHY.md](OMARCHY.md#live-theming) for the mapping;
  check `/usr/share/omarchy/themes/*/colors.toml` before adding a key.
- **A background client's focus request can be ignored by Hyprland/Wayland focus-stealing
  prevention.** Activation (notification click, tray, second launch) first goes through
  `AppHandle::open`/`activate` (with the launcher's XDG activation token when there is one),
  then `desktop::hyprland_focus` runs a fixed-argument (no shell)
  `hyprctl dispatch focuswindow class:^(cosmic-mail)$` when `HYPRLAND_INSTANCE_SIGNATURE` is
  present. The window's app id/class must stay `cosmic-mail` (`rune::tooling::set_app_id`). Test actions on a currently-live notification:
  restored mako history belongs to the old listener and cannot validate the callback.
- **Unix socket paths over 108 bytes fail to bind.** The single-instance socket lives at
  `$XDG_RUNTIME_DIR/cosmic-mail.sock`; a very long `XDG_RUNTIME_DIR` (seen in test sandboxes)
  makes `bind` fail, and the app then logs `single-instance socket unavailable` and runs
  without single-instance handoff rather than refusing to start. Normal sessions
  (`/run/user/<uid>`) are far below the limit.

## IMAP sync

- **Do not implement the 200-message initial-sync cap as `UID FETCH 1:*` followed by a local
  trim.** A 1,497-message Purelymail inbox proved that this downloads/parses the whole mailbox
  and leaves the UI looking empty until the work completes. Use dense message sequence numbers
  from `STATUS MESSAGES` to `FETCH max(1, exists-199):*`, requesting `UID` in the response.
- Folder `total_count` / `unread_count` are authoritative server `STATUS MESSAGES` / `UNSEEN`
  values, not counts of locally cached rows. A successful local `\\Seen` change adjusts unread
  by one; the next STATUS reconciles it. Never recompute these columns from the capped cache.
- **A plain `TcpStream` with no keepalive makes a dead socket byte-for-byte indistinguishable
  from a quiet mailbox — for as long as whatever wall-clock deadline eventually forces a
  reconnect.** (Issue #41.) Laptop suspend/resume, wifi roam, VPN flap, NAT idle-timeout, and a
  server dropping the connection without a FIN/RST all leave the client sitting in `IDLE`
  receiving nothing, and nothing about the TCP/TLS/IMAP layers tells it the peer is gone — the
  socket just looks idle. Before this fix the only bound was `FULL_RESYNC_INTERVAL` (25 min), so
  every laptop resume bought up to a 25-minute window of "notifications have silently stopped."
  This is exactly the class of invisible transport assumption this file exists for: nothing in
  the type signatures (`TcpStream::connect`, `Session::idle()`) hints that the connection can go
  silently dark without an error, so it is easy to ship a sync loop that is provably correct
  about *protocol* handling (drain-checks, UID ranges, etc. — see the entries above and issue
  #39/#40) while still being blind to *transport* death. The fix has two independent layers, on
  purpose — neither alone closes the window: `SO_KEEPALIVE` on the socket (`socket2::SockRef`
  applied to the `tokio::net::TcpStream` in `sync::imap::tls_client`, before the TLS handshake —
  60s idle / 10s interval / 3 probes, so a dead peer surfaces as an IO error in ~90s) catches a
  transport that is *dead*; splitting the IDLE re-issue cadence (5 min) from the full-resync
  deadline (25 min, previously the same constant) bounds how long any single command sits
  unexercised even when keepalive doesn't fire for some reason (e.g. a middlebox that eats
  keepalive probes but still passes data). Re-issuing IDLE without also draining
  `unsolicited_responses` first would reopen the exact race issue #39 closed — a `DONE`/`IDLE`
  round trip is itself a window an `EXISTS` can land in — so this had to land after #39, and the
  re-issue path reuses the same drain-check as every other path into IDLE.

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
  entries (returns None if no SSL entry) until STARTTLS support lands.

## Windows

- **Cargo.lock can hand `gpu-allocator` a different `windows` crate than `wgpu-hal`.**
  `gpu-allocator` accepts `windows >=0.53, <=0.62`, and the resolver unified it with the
  `windows 0.61` that notify-rust's WinRT backend needs, while `wgpu-hal` 30 uses 0.62.
  wgpu's D3D12 backend then fails to compile on Windows only ("expected `ID3D12Device`,
  found a different `ID3D12Device`"); Linux builds never notice. The lock points
  `gpu-allocator` at `windows 0.62.2`, as Rune's own lockfile does. After any
  `cargo update`, check `cargo check --target x86_64-pc-windows-gnu` (the `windows`
  workflow does this on every PR).
- **aws-lc-sys needs NASM on x86_64 MSVC** unless `AWS_LC_SYS_PREBUILT_NASM=1` lets it
  use its prebuilt objects; the `windows` workflow sets it.
- **Unix-only pieces are `cfg`-gated**: the single-instance Unix socket (Windows uses
  `rune::single_instance`, a per-user named pipe), notification actions and the
  desktop-entry hint (freedesktop only), and `xdg-open` (Windows uses
  `rundll32 url.dll,FileProtocolHandler`, which takes the URL without `cmd.exe` parsing).

## Rune (UI)

- **Rune is pre-release: pin a git rev, and the toolchain.** `Cargo.toml` pins a `dev` rev
  (never rewritten upstream); `rust-toolchain.toml` pins 1.99.1, which Rune requires — an older
  toolchain fails to build it. Its APIs are newer than model memory: read the checkout under
  `~/.cargo/git/checkouts/rune-*/<rev>/crates/rune/` (its `README.md` is the reference) before
  writing UI code.
- **`.grow()` is CSS `flex: g 1 0`** (basis 0). A scroll viewport that grows inside an
  unbounded column collapses to 0 pt. Bound an ancestor (`.h_full()`) and put `.min_h(0.0)` on
  every column between it and the viewport, or the content overflows and never scrolls.
- **`text_input` wraps unless told otherwise.** Single-line fields need `.single_line()` (the
  placeholder otherwise wraps word by word inside a 30 pt box).
- **`El::border_sides` takes `[top, right, bottom, left]`** (CSS order) — easy to get the
  header's bottom rule on its left edge.
- **`memo(id, version, ..)` reuses the row until `version` changes**, so the version must
  include everything the row reads — including derived strings like the relative date, or
  "5m" never ages.
- **Handlers outlive the commit that built them.** Re-find items by id inside a handler,
  never by the index they had when the view ran; dialog-field accessors fall back to
  `Ui::scratch_edit` because an edit can arrive after its dialog closed.
- **Backend results land through `Inbox`.** In the app it delivers jobs with
  `AppHandle::update`, which runs on the UI thread even while the window is hidden; in tests
  and headless runs there is no handle, jobs queue and `Ui::poll` drains them — a test must
  call `poll` (via `Runtime::update`) to see backend results or events.
- **A refetch issued before an optimistic removal can land after it** and resurrect the row
  (the E2E caught this). `Ui::pending_removals` filters rows whose move/delete hasn't been
  answered out of every refetch and live insert until the server replies.
- **html2text ignores presentational `<b>`/`<i>`** (it annotates only `<strong>`/`<em>`), and
  mail uses them constantly. `ui::html` rewrites the tags first, carefully leaving `<br>`,
  `<body>`, `<blockquote>`, `<img>` alone.

## Wire format

- **Every wire struct keeps `#[serde(rename_all = "camelCase")]`.** `accounts.json` and
  `settings.json` are persisted in that shape, and the contract in ARCHITECTURE.md describes
  types in it; renaming a field silently breaks existing profiles.
