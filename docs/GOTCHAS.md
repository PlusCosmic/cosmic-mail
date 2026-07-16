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

## TLS / auth

- **rustls panics at the first TLS connection, not at startup**, when both crypto
  backends are in the tree (`lettre` → `ring`, `reqwest`/`oauth2` → `aws-lc-rs`):
  "Could not automatically determine the process-level CryptoProvider". Fix lives at the
  top of `lib.rs::run()`: `rustls::crypto::ring::default_provider().install_default()`.
  If you add/remove TLS-using deps, keep that line, and remember a panic in a tokio
  worker **kills the task silently, not the app** — the UI keeps running while sync is dead.
- **async-imap base64-encodes SASL authenticator responses itself.** `Authenticator::
  process()` must return the **raw** XOAUTH2 string (`user=<email>\x01auth=Bearer
  <token>\x01\x01`). Pre-encoding double-encodes and Gmail rejects it with
  `Invalid SASL argument` (its own test suite proves the encoding: `"foo"` → `Zm9v`).
- Gmail `AUTHENTICATE` failing *after* a token was obtained = wire-format problem;
  failing at "obtaining Gmail access token" = missing client-id env or keyring issue.

## Platform / desktop

- **WebKitGTK + NVIDIA + Wayland/Hyprland crashes the display connection** (`Gdk Error 71
  (Protocol error)`) via the DMA-BUF renderer. `main.rs` sets
  `WEBKIT_DISABLE_DMABUF_RENDERER=1` before GTK init (respects an explicit user
  override). Any secondary webview/window keeps this automatically; don't remove it.
  Downstream symptom in dev: vite logs "transport was disconnected" when the app dies.
- **The `notify` crate reports file *access* events on inotify.** A watcher whose
  handler *reads files inside the watched directory* re-triggers itself forever (ours
  looped at exactly the 300ms debounce period). `omarchy.rs` filters
  `event.kind.is_access()` AND dedupes by comparing the parsed theme to the last one.
  Apply the same pattern to any new watcher.
- omarchy's `current/theme` is a **symlink swapped atomically** — watch the parent dir
  `~/.config/omarchy/current` non-recursively; a real theme switch arrives as
  rename/create events.
- **A background Tauri `set_focus()` can be ignored by Hyprland/Wayland focus-stealing
  prevention.** Notification activation first shows/unminimizes/focuses portably, then uses a
  fixed-argument (no shell) `hyprctl dispatch focuswindow class:^(cosmic-mail)$` fallback when
  `HYPRLAND_INSTANCE_SIGNATURE` is present. Test actions on a currently-live notification:
  restored mako history belongs to the old listener and cannot validate the callback.
- **WebKitGTK never exposes a sandboxed `srcdoc` iframe's document to its parent** — even
  with `sandbox="allow-same-origin"`, `contentDocument` goes null once the real document
  loads (and the frame fires `load` more than once). Chromium happily allows it, so the
  delegated-click-listener pattern from web articles silently does nothing here. Anything
  that must observe events inside the reader frame has to run *inside* it (injected
  script + `postMessage`, with `sandbox="allow-scripts"` for an opaque origin — see the
  reader links bullet in ARCHITECTURE.md). Verify iframe behavior in a WebKit2GTK
  harness, not a Chromium browser.

## IMAP sync

- **Do not implement the 200-message initial-sync cap as `UID FETCH 1:*` followed by a local
  trim.** A 1,497-message Purelymail inbox proved that this downloads/parses the whole mailbox
  and leaves the UI looking empty until the work completes. Use dense message sequence numbers
  from `STATUS MESSAGES` to `FETCH max(1, exists-199):*`, requesting `UID` in the response.
- Folder `total_count` / `unread_count` are authoritative server `STATUS MESSAGES` / `UNSEEN`
  values, not counts of locally cached rows. A successful local `\\Seen` change adjusts unread
  by one; the next STATUS reconciles it. Never recompute these columns from the capped cache.

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

## Tauri / wire format

- **Every wire-crossing struct needs `#[serde(rename_all = "camelCase")]`** — including
  ad-hoc event payload structs in sync code, which is exactly where it was forgotten once
  (frontend read `accountId`, wire had `account_id`; events silently did nothing).
  When adding an event, grep the frontend type in `src/lib/types.ts` and match it.
- Tauri converts JS camelCase invoke args to Rust snake_case params automatically;
  frontend calls `invoke("add_imap_account", { input })` ↔ Rust `input: ImapAccountInput`.
