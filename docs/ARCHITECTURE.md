# Cosmic Mail — Architecture Contract

Native Linux mail client for [omarchy](https://omarchy.org): a Rust backend and a native GPU
UI built with [Rune](https://github.com/anomalyco/rune), in one crate. Supports plain IMAP
accounts and Gmail (OAuth2/XOAUTH2 over IMAP — one sync engine for both).

**This document is the contract between the backend (`src/`, UI-agnostic) and the Rune UI
(`src/ui/`). Both sides must conform to it exactly.** The UI calls `Backend` methods
("commands") and receives `AppEvent`s ("events"); nothing else crosses the boundary. Wire
types are plain Rust structs; the ones persisted or shared as JSON keep
`#[serde(rename_all = "camelCase")]`.

## Module layout

```
src/
  main.rs           # calls app::run
  lib.rs            # module list
  app.rs            # startup: single instance, tokio runtime, db, backend, tray, Rune App
  backend.rs        # Backend: every command below (thin; delegates to modules), Db type
  events.rs         # AppEvent + Events sink (backend -> UI)
  desktop.rs        # --background, single-instance socket, xdg-open (http/s only), hyprctl focus
  error.rs          # AppError (thiserror); Display is the string the UI toasts
  store.rs          # rusqlite: schema, migrations, queries
  accounts.rs       # Account model, accounts.json persistence, keyring secrets
  notifications.rs  # notify-rust wrapper (mako-aware); click -> AppEvent::Activate
  sync/
    mod.rs          # SyncManager: per-account background task on the tokio runtime, IDLE loop
    imap.rs         # async-imap + tokio-rustls connector, LOGIN + XOAUTH2
  auth/
    oauth.rs        # Gmail OAuth2 (PKCE + loopback redirect), token refresh
  ui/               # the Rune UI
    mod.rs          # Ui state, Inbox (backend results -> UI thread), modal state types
    store.rs        # state transitions (selection, paging, optimistic actions, events)
    view/           # element tree: mod (root, panes, keys, palette commands), chrome
                    # (rail, header, folders, footer, toasts), list, reader, modals
    mail_doc.rs     # HTML mail -> Blitz layout -> display list (see "Reader remote-content policy")
    mail.rs         # per-width layout cache, worker-thread layout for the open message
    html.rs         # HTML mail -> styled text lines (image list; fallback rendering)
    images.rs       # inline data: images, consent-gated remote images, opening links
    theme.rs        # rune-theme-foundation tokens -> Palette; Appearance (system/light/dark)
    compose.rs format.rs palette.rs pane.rs   # pure helpers (unit-tested)
    demo.rs         # demo mailbox + named scenes for headless renders and tests
    tests.rs        # UI behaviour tests on rune::Runtime
tests/
  e2e_greenmail.rs  # real UI + real backend against the GreenMail fixture
```

## Wire types (shown in TS notation for brevity; Rust structs in `wire.rs` and the named modules)

```ts
type AccountKind = "imap" | "gmail";

interface Account {           // accounts.rs::Account (public projection, NO secrets)
  id: string;                 // uuid v4
  email: string;
  displayName: string;
  kind: AccountKind;
}

type FolderRole = "inbox" | "sent" | "drafts" | "trash" | "archive" | "spam" | "normal";

interface Folder {
  id: number;                 // db rowid
  accountId: string;
  name: string;               // full IMAP mailbox path, e.g. "Work/Receipts"
  role: FolderRole;           // from SPECIAL-USE attrs, fallback name heuristics
  unreadCount: number;          // authoritative server UNSEEN count from IMAP STATUS
  totalCount: number;           // authoritative server MESSAGES count, not local cache size
}

interface MessageSummary {
  id: number;                 // db rowid (stable local id)
  accountId: string;
  folderId: number;
  uid: number;                // IMAP UID within folder
  subject: string;
  fromName: string;
  fromAddr: string;
  date: string;               // RFC 3339
  snippet: string;            // first ~160 chars of text body, single line
  seen: boolean;
  flagged: boolean;
  hasAttachments: boolean;
}

interface AttachmentInfo {    // store.rs::AttachmentRow projection
  id: number;                 // attachments.id rowid
  filename: string;           // decoded (RFC 2047/2231), sanitized display name
  mimeType: string;           // e.g. "application/pdf"
  sizeBytes: number;          // decoded byte length
  isInline: boolean;          // inline (cid) part vs. a listed attachment
}

interface MessageBody {
  id: number;
  html: string | null;        // laid out by Blitz, never executed or fetched from (see policy);
                              // inline cid: images are inlined as data: URIs at cache time (see policy)
  text: string | null;
  toAddrs: string[];
  ccAddrs: string[];
  attachments: AttachmentInfo[];  // parsed from the body; empty until the body is cached
}

type ShipmentCarrier = "ups" | "fedex" | "usps" | "dhl" | "royal_mail" | "amazon";

interface Shipment {          // shipments.rs::Carrier + store.rs::ShipmentRow projection
  id: number;                 // shipments.id rowid
  carrier: ShipmentCarrier;   // stable lowercase code; the UI maps it to a display label/glyph
  trackingNumber: string | null;
  trackingUrl: string | null; // captured from the email, or a synthesized carrier tracking-page
                              // URL from trackingNumber (never synthesized for Amazon — no
                              // generic public tracking template exists)
  orderId: string | null;     // Amazon order id (\d{3}-\d{7}-\d{7}) when the email carried one
  detectedAt: string;         // RFC 3339, when this row was (re)detected
}

type SyncState = "idle" | "syncing" | "error";

type DiscoverySource = "autoconfig" | "ispdb" | "mx" | "srv" | "guess";

interface DiscoveredConfig {      // autoconfig.rs — settings discovery result
  kind: AccountKind;              // "gmail" ⇒ the UI steers the user to the Gmail OAuth tab
  imapHost: string; imapPort: number;   // implicit-TLS entries only (we don't do IMAP STARTTLS yet)
  smtpHost: string; smtpPort: number;
  username: string;               // %EMAILADDRESS% / %EMAILLOCALPART% already resolved
  source: DiscoverySource;
  confident: boolean;             // false ⇒ heuristic guess, UI must show "unverified"
}

interface ImapAccountInput {
  email: string;
  displayName: string;
  imapHost: string; imapPort: number;   // implicit TLS (993)
  smtpHost: string; smtpPort: number;   // STARTTLS (587) or implicit (465)
  username: string;                     // often == email
  password: string;                     // goes straight to keyring, never to db/json
}                                       // Debug prints the password as <redacted>

interface SendMessageInput {
  accountId: string;                    // sending identity; SMTP details stay backend-only
  toAddrs: string[]; ccAddrs: string[]; bccAddrs: string[];
  subject: string;
  bodyText: string;                     // initial compose implementation is plain text
  replyToMessageId: number | null;      // local message rowid, never an arbitrary RFC header
}

interface Settings {                    // settings.rs::Settings — global preferences
  alwaysDownloadRemoteImages: boolean;  // off by default; see "Reader remote-content policy"
  windowTitleBar: boolean;              // on by default; the platform title bar and borders (Linux
                                        // only: Windows and macOS always draw the app's titlebar).
                                        // Read at startup (decorations are fixed at window
                                        // creation), so a change applies on the next start.
  notificationSound: boolean;           // on by default; new-mail notifications play a sound
                                        // (macOS system alert sound, Windows "Mail", Linux
                                        // `message-new-email` sound-name hint), once per batch.
}
```

## Commands (`Backend` methods, exact names)

| command | args | returns |
|---|---|---|
| `list_accounts` | — | `Account[]` |
| `add_imap_account` | `input: ImapAccountInput` | `Account` (validates by connecting first) |
| `start_gmail_oauth` | — | `Account` (opens browser, blocks until redirect or 5 min timeout) |
| `reauth_gmail_account` | `accountId: string` | `Account` (re-runs the same interactive consent for an **existing** Gmail account, in place — cache/folders/settings untouched; errors without storing anything if the completed consent belongs to a different address than the account's) |
| `remove_account` | `accountId: string` | `void` |
| `add_handed_over_account` | `input: ImapAccountInput` | `Account` (Postmaster's hand-off over the instance socket: refused if an account already has `input.email`, compared case-insensitively; otherwise exactly `add_imap_account`, then emits `AccountsChanged`) |
| `remove_account_by_email` | `email: string` | `void` (Postmaster's removal: the account with that address, compared case-insensitively, exactly as `remove_account`, then emits `AccountsChanged`; errors if there's none) |
| `list_folders` | `accountId: string` | `Folder[]` |
| `list_messages` | `folderId: number, offset: number, limit: number` | `MessageSummary[]` (date DESC) |
| `list_unified_messages` | `offset: number, limit: number` | `MessageSummary[]` (date DESC across all folders with role 'inbox', all accounts) |
| `search_messages` | `query: string, accountId: string \| null, offset: number, limit: number` | `MessageSummary[]` (relevance-ranked; local FTS5 over cached envelopes/bodies. `accountId` null = all accounts, else scoped to that account; all folder roles; empty/whitespace query ⇒ `[]`) |
| `get_message_body` | `messageId: number` | `MessageBody` (fetches from server if not cached) |
| `list_shipments_for_message` | `messageId: number` | `Shipment[]` (empty until the body has been cached, or if none were detected; see "Shipment detection" below) |
| `save_attachment` | `attachmentId: number` | `string` (absolute path of the saved file; refetches raw RFC822 from the server — see below — since it is never cached, re-parses, decodes the part, and writes it to the downloads directory with a sanitized, collision-suffixed name) |
| `mark_read` | `messageId: number, seen: boolean` | `void` (updates server flag + db) |
| `mark_flagged` | `messageId: number, flagged: boolean` | `void` (server `\Flagged` flag + db; emits `mail:messages-updated`) |
| `move_message` | `messageId: number, targetFolderId: number` | `void` (server move to another folder of the **same** account; removes the local row; emits `mail:messages-updated` for both folders) |
| `archive_message` | `messageId: number` | `void` (moves to the account's `archive`-role folder; errors if none) |
| `delete_message` | `messageId: number` | `void` (permanent delete when the source folder role is `trash`, otherwise moves to the `trash`-role folder; errors if no trash folder) |
| `send_message` | `input: SendMessageInput` | `void` (submits through the selected account's SMTP server) |
| `sync_folder` | `folderId: number` | `void` (triggers refresh; progress via events) |
| `sync_account` | `accountId: string` | `void` |
| `test_notification` | — | `void` (sends a sample notification, with sound per `Settings.notificationSound`) |
| `discover_account_config` | `email: string` | `DiscoveredConfig` (never errors on "not found" — falls back to a `guess` with `confident: false`; errors only on invalid email) |
| `get_settings` | — | `Settings` (never errors; a missing/malformed settings file yields defaults) |
| `update_settings` | `settings: Settings` | `Settings` (persists to `settings.json`, returns the stored value) |

Arguments are the method's parameters (snake_case in Rust). Methods that touch the network are
`async` and run on the backend's tokio runtime; the UI calls every method through
`Ui::spawn`, which runs it off the UI thread and lands the result back on it. Errors:
`AppResult<T>` (`Result<T, AppError>`) — the `Display` string is human-readable and the UI
shows it in a toast. Backend-only helpers used by the shell (not the UI): `start_all_syncs`
(startup) and `sync_all_accounts` (tray "Sync now").

## Events (backend -> UI, `events::AppEvent` through the `Events` sink)

| event | variant | payload |
|---|---|---|
| `mail:new-messages` | `NewMessages` | `{ accountId, folderId, messages: MessageSummary[] }` |
| `mail:messages-updated` | `MessagesUpdated` | `{ folderId }` (flags changed / deletions — the UI refetches the page) |
| `mail:sync-state` | `SyncState` | `{ accountId, state: SyncState, error: string \| null, needsReauth: boolean }` (`needsReauth` is `true` only when the failure is classified `AuthExpired` — see the Gmail section — meaning retrying cannot help and the UI should offer Reconnect) |
| — | `Activate` | none: show and focus the window (notification click). The shell handles it directly with `AppHandle::open` so it works while the window is closed. |
| `accounts:changed` | `AccountsChanged` | none: an account was added or removed from outside the UI (Postmaster); the UI reloads the account list, and if it was showing a removed account it falls back to the unified inbox as Remove in Settings does. |

Delivery: `app.rs` installs a sink that forwards every event except `Activate` to the UI's
`Inbox`, which runs it on the UI thread via `AppHandle::update` (this works while the window is
hidden, so state stays current). Without a handle (tests, headless runs) the `Inbox` queues
jobs for `Ui::poll` to drain.

## SQLite schema (store.rs; db at `$XDG_DATA_HOME/cosmic-mail/mail.db`)

```sql
CREATE TABLE IF NOT EXISTS folders (
  id INTEGER PRIMARY KEY,
  account_id TEXT NOT NULL,
  name TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'normal',
  uidvalidity INTEGER NOT NULL DEFAULT 0,
  last_seen_uid INTEGER NOT NULL DEFAULT 0,   -- high-water mark for notification dedupe
  unread_count INTEGER NOT NULL DEFAULT 0, -- server UNSEEN count from latest STATUS
  total_count INTEGER NOT NULL DEFAULT 0,  -- server MESSAGES count (cache may hold fewer)
  UNIQUE(account_id, name)
);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY,
  folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
  uid INTEGER NOT NULL,
  message_id TEXT,                            -- RFC 5322 Message-ID
  subject TEXT NOT NULL DEFAULT '',
  from_name TEXT NOT NULL DEFAULT '',
  from_addr TEXT NOT NULL DEFAULT '',
  to_addrs TEXT NOT NULL DEFAULT '[]',        -- JSON array
  cc_addrs TEXT NOT NULL DEFAULT '[]',
  date TEXT NOT NULL,                         -- RFC 3339
  snippet TEXT NOT NULL DEFAULT '',
  seen INTEGER NOT NULL DEFAULT 0,
  flagged INTEGER NOT NULL DEFAULT 0,
  has_attachments INTEGER NOT NULL DEFAULT 0,
  rfc822_size INTEGER NOT NULL DEFAULT 0,       -- server-reported bytes; 0 when not known yet
  body_text TEXT, body_html TEXT,             -- NULL until body fetched
  body_cached INTEGER NOT NULL DEFAULT 0,      -- distinguishes an unfetched body from an empty one
  UNIQUE(folder_id, uid)
);
CREATE INDEX IF NOT EXISTS idx_messages_folder_date ON messages(folder_id, date DESC);

-- Attachment metadata, populated when a message body is parsed (foreground miss
-- or background prefetch). Raw part bytes are NOT stored; `save_attachment`
-- refetches and re-parses. `part_index` is the mail-parser MessagePartId (stable
-- position in the flat parse order) so the exact part can be re-extracted.
-- Needs no FTS triggers. Rows are replaced wholesale each time a body is cached.
CREATE TABLE IF NOT EXISTS attachments (
  id INTEGER PRIMARY KEY,
  message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  part_index INTEGER NOT NULL,          -- stable index in deterministic parse order
  filename TEXT NOT NULL DEFAULT '',
  mime_type TEXT NOT NULL DEFAULT 'application/octet-stream',
  size_bytes INTEGER NOT NULL DEFAULT 0,
  is_inline INTEGER NOT NULL DEFAULT 0,
  content_id TEXT,
  UNIQUE(message_id, part_index)
);
-- `messages.has_attachments` starts as a BODYSTRUCTURE heuristic (envelope-only
-- rows) and is corrected to the real count of non-inline attachments once a body
-- is parsed, in the same transaction that replaces attachment rows.

-- Shipments detected by local heuristic parsing of a cached message body
-- (shipments.rs::extract_shipments), populated at the same body-parse hook as
-- `attachments` (foreground cache miss in `get_message_body`, or background
-- prefetch). `carrier` is a stable lowercase code (see the `ShipmentCarrier`
-- wire type above), not a display label. Nullable columns reflect what the
-- heuristic actually found — e.g. Amazon shipment emails often carry only a
-- login-gated tracking link and an order id, no raw tracking number. Rows are
-- replaced wholesale per message on every parse (mirrors `replace_attachments`),
-- so re-parsing a message cannot accumulate duplicate rows.
CREATE TABLE IF NOT EXISTS shipments (
  id INTEGER PRIMARY KEY,
  message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  carrier TEXT NOT NULL,
  tracking_number TEXT,
  tracking_url TEXT,
  order_id TEXT,
  detected_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_shipments_message ON shipments(message_id);

-- Local full-text search index (SQLite FTS5, external-content over `messages`).
-- Searches the local cache only — never the server. Created once, guarded on
-- existence; existing rows are backfilled with the FTS5 `rebuild` command.
-- `body_text` is NULL until a body is cached and indexes as empty until then.
CREATE VIRTUAL TABLE messages_fts USING fts5(
  subject, from_name, from_addr, snippet, body_text,
  content='messages', content_rowid='id'
);
-- AFTER INSERT / AFTER DELETE / AFTER UPDATE OF (the five indexed columns)
-- triggers keep the index in sync using the external-content delete/insert form
-- (`INSERT INTO messages_fts(messages_fts, rowid, …) VALUES('delete', …)`).
```

At startup — synchronously in application setup, before any sync task spawns (so it heals even
when sync cannot run, e.g. a missing OAuth token) and before the UI requests its first
message pages (so no update events are needed) — `store::heal_cached_snippets` recomputes the
snippet of every row with `body_cached = 1` using the current `snippet_for_body` logic and
updates only the rows whose snippet actually changes (the FTS `AFTER UPDATE` trigger re-indexes
them; unchanged rows are skipped so the trigger doesn't fire needlessly). Cached bodies are never
re-fetched, so this self-healing pass is how previously-cached rows pick up snippet-cleanup
improvements; it runs on every launch — the cached-body population is small and the recompute is
pure string work, which beats one-shot migration/version bookkeeping.

`search_messages` builds its MATCH expression by splitting the raw query on whitespace,
escaping each token (doubling embedded `"`), quoting it, and appending `*` for prefix
matching — never passing untrusted text to MATCH — then `ORDER BY rank` (bm25).

Account configs (non-secret) live in `$XDG_CONFIG_HOME/cosmic-mail/accounts.json`.
Global preferences (non-secret) live in `$XDG_CONFIG_HOME/cosmic-mail/settings.json`
(`settings.rs`); the read path never errors — a missing or malformed file yields defaults.

## Secrets (keyring, Secret Service via keyring v4 zbus store)

- service: `dev.pluscosmic.mail`
- user key: `imap-password:<account-id>` for IMAP; `oauth-refresh-token:<account-id>` for Gmail.
- Never write secrets to sqlite, accounts.json, or logs.

## Omarchy integration (verified on this machine)

- Notifications are rendered by **mako**; plain freedesktop D-Bus notifications are all that's
  needed. Send via `notify-rust` with:
  - `appname("Cosmic Mail")` — users theme/script per app-name in mako config
    (user already has an `[app-name="Betterbird Mail"]` block; ours enables the same pattern).
  - `.hint(Hint::DesktopEntry("dev.pluscosmic.mail".into()))` — icon + association.
  - a `default` action; on activation, focus the main window (and later, select the message).
    The action emits `AppEvent::Activate`; the shell shows and focuses the window through
    Rune's `AppHandle::open` first, then under Hyprland follows it with a fixed-argument
    `hyprctl dispatch focuswindow class:^(cosmic-mail)$` fallback because Wayland
    focus-stealing prevention can ignore a background focus request. Never invoke
    this through a shell or interpolate notification/message content into the command.
  - summary: `New mail — {fromName}`, body: `{subject}`. Batch: if >3 new messages in one
    sync pass for a folder, send ONE notification: `{n} new messages in {account email}`.
  - Only notify for folders with role `inbox`, only for UIDs above `last_seen_uid`, never
    during an account's initial sync.
  - Sound: when `Settings.notificationSound` is on, the batch's first notification carries
    the `message-new-email` sound-name hint (the rest, and all of them when it is off, carry
    `suppress-sound`). mako plays no sound itself; a mako `[app-name="Cosmic Mail"]` block
    with `on-notify=exec …` can.
- macOS: notifications go through `notify-rust`'s `NSUserNotificationCenter` backend, posted
  as `dev.pluscosmic.mail` (`notify_rust::set_application`, once, before the first one) so
  they carry the app's name and icon and are controlled under its System Settings entry; this
  needs the installed `Cosmic Mail.app` (Launch Services), else they post as another app.
  Sound is `NSUserNotificationDefaultSoundName`. A click emits `AppEvent::Activate`, as on
  Linux: each notification's own thread waits for it (`wait_for_response`; the delegate
  callback arrives on the main run loop Rune's loop pumps), and `AppHandle::open` shows the
  window, reopening it if closed, and activates the app. A notification left in Notification
  Center keeps its thread waiting until clicked or cleared, so at most 32 wait at once; past
  that a notification posts fire-and-forget (a click still activates the app, but cannot
  reopen a closed window).
- Theme: the UI's colours, fonts and type come from `rune-theme-foundation`
  (PlusCosmic/ui-foundation, generated from `@pluscosmic/ui`'s `tokens.css`). `ui::theme::Palette`
  names the foundation's tokens by the roles the views use (`bg`, `fg`, `muted` = `fg-soft`,
  `panel` = `surface`, `panel_2` = `bg-soft`, `fill`/`fill_ink` for primary buttons, `error` =
  `danger`, ...); no colour is hard-coded outside the foundation. The colour scheme and the
  appearance (system/light/dark) are UI preferences in `ui.json` (`colorScheme`, `appearance`),
  not backend settings, and the palette is rebuilt each frame from them and the desktop's
  light/dark preference, so a change re-renders at once.

## UI pane layout

- The account rail (60 pt) and optional 210 pt folder column remain fixed-width. The message
  list and reader share all remaining width, separated by a pointer-draggable,
  keyboard-focusable vertical separator (`role("separator")`).
- The separator constrains both panes to usable minimum widths (`ui::pane`). The message list
  also has a readable maximum width so maximized windows give surplus space to the reader.
- The preferred message-list proportion persists as `listPaneRatio` in
  `$XDG_CONFIG_HOME/cosmic-mail/ui.json` (Rune `Prefs`). A proportion, rather than a pixel
  width, keeps the preference useful when the window is resized or the folder column appears.
  Constraints re-apply whenever the available width changes; malformed values fall back.
- Left/Right arrows resize the focused separator (Shift uses a larger step), Home/End choose its
  minimum/maximum, and double-click restores the default 42% split.

## Process and window lifecycle

- Cosmic Mail has exactly one process/sync-engine owner per desktop session. The first process
  binds `$XDG_RUNTIME_DIR/cosmic-mail.sock` (`desktop::claim_instance`; a stale socket from a
  crash is reclaimed). A later launch connects, writes one line — `activate [token]` or
  `background` — and exits before opening the database or starting sync. If the socket cannot be
  bound at all (e.g. a path over the 108-byte Unix limit) the process logs a warning and runs
  without single-instance protection.
- The socket also takes requests from [Postmaster](https://github.com/PlusCosmic/postmaster),
  the Purelymail admin app, which **wait for a one-line reply on the same connection**:
  - `add-account <ImapAccountInput as compact JSON>` → `add_handed_over_account`, run on the
    tokio runtime (it validates by connecting); replies `ok <accountId>` or `error <message>`.
    The password is Postmaster's app password for the mailbox, never its main one; it goes to
    the keyring like any other, and the request's `Debug` shows only the address.
  - `remove-account <email>` → `remove_account_by_email`; replies `ok` or `error <message>`.

  Replies are one line (`ok [detail]` / `error <message>`, whitespace collapsed). A verb a
  version doesn't know is dropped without a reply, so the sender sees the connection close and
  can say Cosmic Mail needs updating. A request line is read up to 16 KiB. Because requests can
  carry a password, the socket is `chmod 600` straight after `bind`, which matters when
  `XDG_RUNTIME_DIR` is unset and it falls back to the shared temp folder.
- The promoted daily build installs `cosmic-mail.service`, a systemd user service bound to
  `graphical-session.target`. It launches the binary with `--background`, uses
  `Restart=on-failure` for crash recovery, and stops it with the graphical session. A clean tray
  Quit therefore remains stopped until the service is explicitly started or the next graphical
  session. Development runs do not install or control this service.
- The window is Rune's. On Windows and macOS it draws its own 40 pt titlebar band
  (`App::titlebar`, `view::titlebar`: the mark, the search field, `.window_drag()`) under the
  native controls (macOS traffic lights; Windows caption buttons Rune draws, with snap layouts),
  and `Settings.windowTitleBar` is ignored. On Linux it has the platform title bar and borders unless
  `Settings.windowTitleBar` is off (for a tiling window manager such as omarchy's Hyprland,
  which draws its own), 1280×800, minimum 900×560, Wayland app id / X11 class
  `cosmic-mail`. A normal first launch opens it; a `--background` first launch starts windowless
  (`App::windowless`) while sync, IDLE, and notification listeners run normally.
- Closing the window hides it (`App::keep_running`; on Wayland the toplevel is destroyed and
  recreated on open). This does not stop the process or its sync tasks. Session
  shutdown/systemd stop and tray Quit are actual process exits.
- A second normal launch is an explicit activation request: the owner shows and focuses the
  window, forwarding the launcher's `XDG_ACTIVATION_TOKEN` to `AppHandle::activate` when there is
  one. A second `--background` launch is silent so service restarts cannot steal focus.
  Notification default actions and the tray use the same path, followed by the fixed-argument
  Hyprland focus fallback (`hyprctl dispatch focuswindow class:^(cosmic-mail)$`, only when
  `HYPRLAND_INSTANCE_SIGNATURE` is set).
- One tray icon (Rune's StatusNotifierItem tray, `rune::tray`) lives for the process lifetime with
  **Open Cosmic Mail**, **Sync now**, and **Quit**; a primary click opens the window. Sync now
  restarts the background sync task for every configured account. Quit requests a clean
  application exit. A missing tray host (bare session, CI) is logged and non-fatal.
- Headless Rune runs (`--snap`, `--tree`, `--script`, `--list-scenes`, ...;
  `rune::tooling::is_headless`) render the built-in demo mailbox (`ui::demo`) and never claim
  the socket, open the profile, start sync, or touch the network.

## Gmail (auth/oauth.rs)

- OAuth 2.0 authorization-code + PKCE, loopback redirect (`http://127.0.0.1:<random port>`),
  per RFC 8252. Open the consent URL with `desktop::open_url` (`xdg-open`, http/https only). Scope: `https://mail.google.com/`
  plus `openid email` (to learn the address). Endpoints:
  auth `https://accounts.google.com/o/oauth2/v2/auth`, token `https://oauth2.googleapis.com/token`.
- Client credentials resolve in order (first hit wins):
  1. Runtime env vars `COSMIC_MAIL_GOOGLE_CLIENT_ID` (+ optional
     `COSMIC_MAIL_GOOGLE_CLIENT_SECRET`) — dev override.
  2. `$XDG_CONFIG_HOME/cosmic-mail/google-oauth.json` — `{"clientId": "…",
     "clientSecret": "…"}` (camelCase; `clientSecret` optional) — user self-provisioning.
  3. Compile-time baked defaults via `option_env!("COSMIC_MAIL_BUILD_GOOGLE_CLIENT_ID")`
     (+ `COSMIC_MAIL_BUILD_GOOGLE_CLIENT_SECRET`) — set these env vars when building a
     packaged release so installs work out of the box. Distinct names from the runtime
     vars on purpose: a dev shell must never silently bake its credentials into a binary.
  Shipping/storing these is fine: Google desktop-app client credentials are
  non-confidential by design (RFC 8252 §8.5 — installed apps can't hold secrets; PKCE
  secures the flow). The keyring rule covers real secrets — passwords and refresh tokens.
  If no tier is set, `start_gmail_oauth` returns a clear error explaining how to
  create credentials. NOTE for actual distribution: the `https://mail.google.com/` scope
  is a Google "restricted" scope — beyond ~100 users the OAuth consent screen needs
  Google verification (CASA security assessment); that, not credential plumbing, is the
  real gate on public packaging.
- Store refresh token in keyring; access tokens cached in memory with expiry, refreshed on demand.
- Dead credentials are classified, not just reported: `access_token` tags exactly two failures
  with the `AuthExpired` marker — a missing keyring entry, and a refresh exchange rejected with
  OAuth `invalid_grant` (what Google returns when a "Testing"-status client's 7-day refresh token
  lapses, or the user revoked access). The sync loop downcasts for the marker and emits
  `mail:sync-state` with `needsReauth: true`; every other failure (network, keyring outage,
  server 5xx) keeps `needsReauth: false` and remains a plain retryable sync error.
  `reauth_gmail_account` is the recovery path: same consent flow, then verify the authenticated
  email matches the account (case-insensitive — a mismatch errors without storing anything),
  store the new refresh token under the same account id, seed the token cache, restart sync.
  Removing/re-adding the account (which wipes its local cache) is never required for expired auth.
- IMAP/SMTP auth: SASL `AUTHENTICATE XOAUTH2` with the raw string
  `"user=" + email + "\x01auth=Bearer " + access_token + "\x01\x01"`.
  NOTE: async-imap base64-encodes the authenticator's response itself —
  do NOT pre-encode (double encoding ⇒ Gmail "Invalid SASL argument").
  Hosts: `imap.gmail.com:993`, `smtp.gmail.com:587`.

## Compose and SMTP sending (send.rs)

- The first compose implementation sends plain-text messages. `send_message` requires at least one
  valid To/Cc/Bcc mailbox and builds all address and message headers with lettre; the UI never
  supplies raw headers or SMTP credentials.
- The selected account supplies the From mailbox and SMTP configuration. Password accounts reuse
  their keyring-held IMAP password with SMTP `PLAIN`/`LOGIN`; Gmail obtains an access token through
  `auth/oauth.rs` and uses lettre's `XOAUTH2` mechanism with the raw token (lettre constructs and
  base64-encodes the SASL response).
- SMTP is always encrypted: port 465 uses implicit TLS; every other configured submission port uses
  required STARTTLS. There is no plaintext or opportunistic-TLS fallback.
- For replies, `replyToMessageId` is a local SQLite rowid. The backend verifies that the cached
  message belongs to the sending account, resolves its RFC 5322 `message_id`, and, when present,
  writes it as both `In-Reply-To` and the initial `References` value. The command omits both headers
  if the source message has no cached Message-ID. A full pre-existing References chain is not cached
  yet, so this initial implementation threads against the direct parent.
- SMTP acceptance completes the command. Providers such as Gmail normally copy SMTP submissions to
  Sent themselves; Cosmic Mail does not locally insert or IMAP-APPEND the outgoing message.

## Message actions (backend.rs + sync/imap.rs)

Flag/archive/delete/move follow the same shape as `mark_read`: locate the row → open a
per-command IMAP connection → select the source folder → run the server op → update the DB →
emit events. All are server-authoritative; the local cache is adjusted optimistically in the
UI and reconciled by the emitted `mail:messages-updated` events (which refetch the page
and reload folder counts).

- **Mark read/unread** (`mark_read`) — `UID STORE ±FLAGS (\Seen)`, updates the local row, adjusts the
  source folder's unread count, and emits `mail:messages-updated` for it. On a **Gmail** account
  (`Account.kind == "gmail"`), the same physical message is exposed under multiple folders
  (labels); after the primary update, every other cached copy in that account sharing the same
  RFC 5322 Message-ID has its `seen` state and owning folder's unread count updated locally too
  (`store::mark_seen_for_message_id_siblings`), and `mail:messages-updated` is emitted for each
  additionally-changed folder — no new event/wire type, this just fans the existing event out to
  more folder ids. Gmail's own `\Seen` propagation across labels means the next sync of those
  folders reconciles regardless; this only removes the wait until that next sync.
- **Flag** — `UID STORE ±FLAGS (\Flagged)`, mirroring the `\Seen` path. No unread-count change.
- **Move** (also the mechanism behind archive and non-trash delete) — prefers `UID MOVE`
  (RFC 6851, `MOVE` capability); otherwise falls back to `UID COPY` + `UID STORE +FLAGS
  (\Deleted)` + expunge, preferring `UID EXPUNGE` (RFC 4315 `UIDPLUS`) so only the copied
  message is removed, and dropping to a plain `EXPUNGE` when `UIDPLUS` is absent (which removes
  every `\Deleted` message in the mailbox). The target must belong to the **same account** and
  differ from the source (moving between accounts is unsupported; both are rejected with a clear
  error). The local source row is **removed** — no fabricated local row with a guessed UID; the
  message reappears in the target on its next sync. The target folder's counts are bumped
  best-effort (`total +1`, `unread +1` when the message was unseen) until the next STATUS.
- **Archive** — resolves the account's `archive`-role folder, then moves. Gmail's All Mail
  carries the `\All` SPECIAL-USE attribute, which `list_folders_with_roles` already maps to the
  `archive` role, so Gmail archive resolves to All Mail.
- **Delete** — permanent (`UID STORE +FLAGS (\Deleted)` + expunge) when the source folder's role
  is already `trash`; otherwise a move to the `trash`-role folder. The local row is removed in
  both cases.

`remove_message` deletes the local row (the FTS5 delete trigger and the `attachments`
foreign-key cascade clean up derived rows) and decrements the source folder's `total_count`
(and `unread_count` when the row was unseen), floored at 0.

## Settings discovery (autoconfig.rs)

`discover_account_config` resolves IMAP/SMTP settings from just the address, trying in order
(each HTTP fetch 5s timeout, ~12s total budget; first hit wins):

1. `https://autoconfig.{domain}/mail/config-v1.1.xml?emailaddress={email}` → source `autoconfig`
2. `https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml` → source `autoconfig`
3. `https://autoconfig.thunderbird.net/v1.1/{domain}` → source `ispdb`
4. MX lookup (hickory-resolver) → registrable domain of the MX host → that provider's own
   `autoconfig.{mxdomain}` endpoint (hosts configs for custom domains, e.g. Purelymail),
   then ISPDB again → source `mx`
5. RFC 6186 SRV: `_imaps._tcp.{domain}` + `_submission._tcp.{domain}` → source `srv`
6. Fallback `imap.{domain}:993` / `smtp.{domain}:587`, `confident: false` → source `guess`

Rules: parse config-v1.1.xml with roxmltree; take the first `incomingServer type="imap"` with
`socketType SSL` (skip STARTTLS — unsupported by our connector) and first `outgoingServer
type="smtp"` preferring STARTTLS/587 then SSL/465; resolve `%EMAILADDRESS%`/`%EMAILLOCALPART%`
placeholders in username. Gmail detection: domain is `gmail.com`/`googlemail.com`, discovered
IMAP host is `imap.gmail.com`, or MX host ends in `.google.com`/`.googlemail.com` ⇒
`kind: "gmail"`. Discovery makes no connections to the mail servers themselves —
`add_imap_account` still validates by connecting.

## Sync engine (sync/)

- One tokio task per account (`SyncManager` holds JoinHandles; aborted on account removal).
- Loop: connect → LIST folders (RFC 6154 SPECIAL-USE for roles) → `STATUS` each folder for
  authoritative `MESSAGES`, `UNSEEN`, `UIDNEXT`, and `UIDVALIDITY` → UIDVALIDITY check
  (mismatch ⇒ wipe folder cache) → fetch/upsert envelopes → emit events/notify. Then hold the
  connection in an INBOX idle loop: re-sync INBOX (STATUS counts + incremental fetch + notify) →
  inbox body prefetch → drain-check → IDLE → on wakeup, re-sync INBOX on the same connection and
  prefetch/drain-check/IDLE again — until the
  25-min full-resync deadline ends the cycle and the loop reconnects for a fresh full sweep. The
  deadline is a hard wall-clock bound (async-imap's IDLE timeout is inactivity-based; server
  keepalives reset it). On error: emit sync-state error, retry with backoff (30s → 5 min cap).
- **Dead-connection detection (issue #41):** two independent layers, because each one alone leaves
  a window a laptop resume/wifi-roam/VPN-flap/NAT-timeout can fall into (see `docs/GOTCHAS.md`).
  First, TCP keepalive on the socket itself (`sync::imap::tls_client`, via `socket2::SockRef`
  applied to the `tokio::net::TcpStream` before the TLS handshake): `TCP_KEEPALIVE_IDLE` = 60s,
  `TCP_KEEPALIVE_INTERVAL` = 10s, `TCP_KEEPALIVE_RETRIES` = 3, so a genuinely dead peer surfaces as
  an IO error in roughly 90s instead of sitting silent until the full-resync deadline. Second, the
  IDLE re-issue cadence: a single IDLE command is capped at `IDLE_REISSUE_INTERVAL` (5 min) rather
  than the full `FULL_RESYNC_INTERVAL` (25 min) — those two were previously the same constant.
  Hitting the 5-minute cap alone does **not** end the cycle: the loop re-syncs INBOX,
  drain-checks, and re-issues IDLE on the *same* connection (`IdleTimeoutAction::Reissue`); only
  actually reaching the 25-minute deadline tears the connection down and reconnects fresh
  (`IdleTimeoutAction::EndCycle`). Each `idle_wait` call is capped at
  `min(IDLE_REISSUE_INTERVAL, time_remaining_until_full_resync_deadline)`
  (`sync::idle_wait_budget`), and which of the two a `Timeout` outcome actually was is decided by
  `sync::idle_timeout_action` — both are pure functions over `Duration`s, unit-tested including the
  boundary where the re-issue interval would overrun the deadline. The re-issue path still goes
  through the same drain-check as every other path into IDLE (a `DONE`/`IDLE` round trip is itself
  a window an `EXISTS` can land in — see issue #39 below).
- **Drain-check, and why it exists (issue #39):** an IMAP server announces new mail by
  piggy-backing an untagged `* N EXISTS` onto whatever command response is in flight, and never
  repeats that announcement later. `async-imap` routes any such response that lands outside of
  IDLE's own wait — i.e. on the STATUS/SELECT/FETCH commands a re-sync issues, or even on the IDLE
  command's own response before its `+ idling` continuation — onto
  `Session::unsolicited_responses`, a `bounded(100)` channel that is otherwise never read. Before
  entering IDLE, and again immediately after `Handle::init()` succeeds, the loop drains that
  channel with `try_recv` (`sync::drain_mailbox_changed`) and treats any `Exists`/`Recent` as
  "mailbox changed — re-sync now, don't idle yet," repeating the STATUS/SELECT/FETCH pass until a
  drain comes back clean before calling `session.idle()`. The check is a local, non-blocking read
  with no `.await` between it and entering IDLE, so there is no remaining gap — closing a race that
  a bare "re-sync precedes every IDLE" structure (the previous fix, commit 0f1354e) left open,
  since that re-sync happened without ever reading this channel. Bounded by
  `MAX_CONSECUTIVE_RESYNCS_BEFORE_IDLE` (a small constant) so a mailbox that is genuinely changing
  on every single re-sync cannot spin forever. Note what that bound must *not* do: once a drain
  has returned "changed", the announcement is already out of the channel and the server will
  never repeat it, so idling at the cap would strand that message until the deadline — the exact
  bug being fixed. The cap therefore ends the cycle (reconnect + full sweep, guaranteed to see
  it) rather than idling. Only a clean drain may enter IDLE (`sync::drain_action`).
- Initial sync: use message sequence numbers from `STATUS MESSAGES` to `FETCH` exactly the
  newest 200 existing messages per folder (or all when fewer than 200), including `UID` in the
  response. Do not `UID FETCH 1:*` and trim afterward.
- Incremental sync: `UID FETCH <cached-max-uid+1>:*` and ignore any duplicate boundary item.
  Folder `totalCount`/`unreadCount` stay server-authoritative even when only 200 envelopes are
  cached; successful local flag changes adjust the stored server count until the next STATUS.
- **Body prefetch timing (issue #40):** opportunistically cache at most 5 bodies from the newest
  20 unread and newest 20 overall cached inbox envelopes (`BODY_PREFETCH_WORKING_SET` = 20,
  `BODY_PREFETCH_LIMIT` = 5). Prefetch is sequential and best-effort so an individual body failure
  does not fail the sync cycle. Skip messages larger than 1 MiB, using a separate `RFC822.SIZE`
  lookup when a pre-migration envelope has no stored size. Fetch full messages with
  `BODY.PEEK[]`, never `BODY[]`/`RFC822`, so caching cannot set `\Seen`; only the explicit
  `mark_read` command changes that flag. Parsing and caching do not render HTML or load network
  resources. Foreground cache misses use the same non-marking fetch. Prefetch is *not* part of
  the per-folder re-sync helper (`sync_folder_uids`) — issue #40 hoisted it into `run_once`'s
  idle loop, where it runs at exactly one call site, positioned between the re-sync's notify and
  the drain-check. Both sides of that position are load-bearing. **After the notify**, because
  prefetch is up to five sequential size+body fetches and putting it ahead of the notify would
  delay every new-mail toast by exactly the best-effort background work that must stay out of the
  notification path. **Before the drain-check**, because the drain-check has to be the last thing
  that happens before `session.idle()` (issue #39) — so an `EXISTS` piggy-backed on one of
  prefetch's own FETCH commands lands in `unsolicited_responses` and is caught by the drain-check
  rather than lost. On a single connection this window can be made *safe* but not made to
  disappear; eliminating it requires prefetch on a second connection (issue #42). Prefetch also
  requires INBOX to be the selected mailbox — UIDs are mailbox-scoped, and a matching UID in the
  wrong mailbox would store another folder's body/attachments/shipments against an INBOX message,
  which is why it runs immediately after `sync_folder_uids` has selected INBOX and never straight
  after the full sweep (which leaves whichever folder came last in the LIST response selected).
- The idle loop's step order is therefore: full sweep (all folders) → loop { STATUS+SELECT+FETCH
  INBOX (envelopes + notify) → inbox prefetch → drain-check → IDLE (capped at
  `min(IDLE_REISSUE_INTERVAL, time_remaining_until_deadline)`) }, repeating on a `NewData` wakeup
  or a re-issue-cadence `Timeout`, and ending the cycle to reconnect fresh on a
  full-resync-deadline `Timeout`, a dead connection, or a drain-check that hits
  `MAX_CONSECUTIVE_RESYNCS_BEFORE_IDLE`.
- rustls: `tokio_rustls` with `rustls_native_certs` root store.

## Shipment detection (shipments.rs)

- Pure, dependency-free heuristic parsing over a message's cached `text`/`html` body — no
  network calls, no new crates. Runs at the same body-parse hook as attachment extraction, in
  both places a body is cached: the foreground cache miss in `get_message_body` and the
  background prefetch loop in `sync/mod.rs`. Each call replaces that message's `shipments` rows
  wholesale (`store::replace_shipments`, mirroring `replace_attachments`), so re-parsing cannot
  accumulate duplicates. Bodies already cached before this feature shipped are not retroactively
  backfilled — there is no startup healing pass for shipments (unlike snippets); a shipment
  surfaces once that message's body is (re)fetched.
- Carriers and tracking-number shapes: UPS (`1Z` + 16 alphanumeric, Mod-10 check digit); FedEx
  (12/15/20-digit numeric, no checksum); USPS (20/22-digit numeric IMpb with its own Mod-10
  variant, and the 13-character UPU S10 international format, `XX` + 9 digits + `US`); DHL
  (10-digit numeric, no checksum); Royal Mail (UPU S10, `XX` + 9 digits + `GB`); Amazon (no raw
  tracking number — a login-gated tracking link and/or an order id `\d{3}-\d{7}-\d{7}`).
- Checksums are implemented for every format that defines one (UPS Mod-10 and the UPU S10
  standard shared by Royal Mail and USPS's 13-char format); a format-shaped number whose
  checksum fails is dropped outright, never downgraded to a guess. A 20/22-digit numeric USPS
  IMpb token is treated as deterministic only when its Mod-10 checksum validates *and* it starts
  with `9`, matching every real USPS service-type prefix (92/93/94/95/96/…) — a checksum alone
  passes about 1 in 10 random numbers of that length, and reference numbers of that length (bank
  references, account numbers, GUID-ish ids) do turn up in emails, so the checksum by itself is
  not enough evidence. Formats with no published checksum (FedEx, DHL, and a 20/22-digit number
  that fails the checksum-plus-prefix test above) are only kept when a shipping keyword, or the
  carrier's own name, appears within 120 characters of the match — bare numbers of these lengths
  are common false positives (phone numbers, invoice/order numbers). A 20/22-digit numeric token
  is shape-ambiguous between USPS and FedEx; short of the checksum-plus-prefix case, it requires
  the carrier's own name nearby (a generic "shipped" nearby is not enough to pick between the
  two). Context-gated guesses are suppressed entirely once the message already produced at least
  one checksum-verified shipment — a real shipping email is saturated with generic shipping
  keywords, so a second, weaker, co-occurring number-shaped token (an order id, a transaction or
  reference number, ...) is nearly always noise from the same email rather than a second shipment.
- Tracking links found in the body (`ups.com/track`, `fedex.com` track URLs,
  `tools.usps.com/go/TrackConfirmAction`, `dhl.com` tracking URLs, `royalmail.com/track-your-item`,
  Amazon tracking/progress-tracker links) disambiguate the carrier for free and are preferred
  over a guessed (unchecksummed) carrier when both are present in the same message; they never
  override a checksum-verified or carrier-name-confirmed match.
- `Shipment.trackingUrl` is resolved once, at extraction time: the link captured from the email
  when present, otherwise a synthesized public carrier tracking-page URL built from
  `trackingNumber` (never synthesized for Amazon, which has no generic public tracking template
  — its card is a dead end without a captured link).

## Reader remote-content policy

- HTML message bodies are **laid out, never executed or fetched from**. `ui::mail_doc::layout`
  hands the body to Blitz (`blitz-html`/`blitz-dom`: html5ever parsing, Stylo for the cascade of
  the mail's own `<style>` blocks and inline styles, Taffy for block/flex/table layout, Parley for
  text) at the reader's content width, and flattens the result to a display list — element boxes
  (background, gradient, shadow, per-side borders and corner radii — including inline elements'
  boxes, row backgrounds and collapsed table borders), images by `src`, background images by URL
  with their CSS size/position/repeat, list markers, and one text item per line segment (with its
  letter spacing) with per-run weight/italic/colour/underline/strike-through/link spans, cut to the
  padding box of any `overflow` ancestor they cross (`Item::Clip` runs; wholly clipped items are
  dropped). The reader draws that
  list as Rune elements on a page in the mail's own colours (white unless the mail sets a
  background, in either app theme), so table layouts, buttons and colour bars look as they do in
  other clients. There is no JavaScript engine and no event dispatch; forms and frames draw
  nothing; Blitz is given no network provider, so `<link>` stylesheets, `@import` and web fonts
  are never requested. CSS `url()` backgrounds (and the legacy `background=` attribute) become
  display-list items whose URLs join the body's image list: the reader loads them exactly like
  `<img>`s — `data:` at once, remote ones only with remote-image consent. All text is laid out and drawn in the app's own
  fonts (system fonts only as glyph fallback), since Rune draws only fonts the app embeds.
- Layout runs on a worker thread (16 MiB stack; Stylo recurses deeply) and is redone when the
  reader's width changes (8 px steps) or a body image finishes decoding (its pixel size feeds the
  layout as the `<img>`'s intrinsic size). Until the first layout lands the body area is empty.
- `ui::html::render` (`html2text`, rich mode, layout tables flattened to one column) still turns
  the body into styled text lines: they list the body's images (what `prepare_body_images` loads
  and what the remote-image notice keys on) and are drawn instead when Blitz produces nothing.
- Inline `cid:` images are resolved to `data:` URIs backend-side at body-cache time — never by a
  network fetch. When a body is parsed, each `image/*` part carrying a Content-ID whose decoded size
  is ≤ 512 KiB, and while a running per-message budget of ≤ 2 MiB is not exceeded, has its stored
  `body_html` `cid:<content-id>` references rewritten (case-insensitive, exact string match) to
  `data:<mime>;base64,<payload>` before caching. Over-cap parts keep their `cid:` reference, which
  shows as its alt text. The reader decodes `data:` images locally (Rune `ImageCache`) and always
  shows them; 1–2 px images (spacers, pixels) are skipped.
- Remote (`http(s)`, protocol-relative) images are blocked by default and show as their alt text.
  A visible reader action ("Load remote images") allows them for the selected message. Consent
  is keyed by message rowid, kept only for the current session, and never carries over to another
  message or persists across restarts. Allowed images are fetched by the backend runtime with
  `reqwest` (GET, no cookies, ≤ 5 redirects, 15 s timeout, ≤ 8 MiB each, ≤ 40 per message) and
  decoded locally. Nothing else in a message ever reaches the network.
- The global `Settings.alwaysDownloadRemoteImages` preference (off by default; persisted in
  `settings.json`) allows remote **images only** for every message, and hides the per-message
  consent bar because it is already satisfied. It changes nothing else.
- Links never navigate anything inside the app. A click on a link calls `Ui::open_link`, which
  accepts only absolute `http`/`https` URLs (`desktop::openable_url`) and hands them to
  `xdg-open` (no shell); `javascript:`, `file:`, `mailto:`, `data:` and relative hrefs are
  ignored. Shipment tracking chips and the Gmail OAuth consent URL use the same `open_url`.

## Headless tooling and tests

There is no automation bridge: the UI is Rust, so tests drive it in-process.

- **Unit tests** cover the pure helpers (`ui::format`, `compose`, `palette`, `pane`, `html`, `mail_doc`,
  `theme`) and the backend modules.
- **UI behaviour tests** (`src/ui/tests.rs`) build the real view over the demo mailbox on
  `rune::Runtime` — Rune's headless runtime with the same layout, hit testing, focus and key
  routing as a window, on a virtual clock — and drive it with pointer presses, keys and typing
  (`j`/`k`, `gg`/`G`, `/` + Enter, Ctrl+K, chips, rail, dialogs, optimistic actions, events).
- **Headless renders**: any Rune tooling flag renders the demo mailbox offscreen with the real
  renderer: `cosmic-mail --snap out.png [--scene NAME] [--scale 2]`, `--list-scenes`,
  `--tree tree.json` (element tree), `--script s.json` (scripted input), `--video`. Scenes live
  in `ui::demo::SCENES`. Without a GPU, use Mesa's lavapipe
  (`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`).
- **End to end** (`tests/e2e_greenmail.rs`, `#[ignore]`d unless run with `--ignored`): the
  real `Backend` over a real database, syncing the GreenMail fixture, with the real view on
  `rune::Runtime`. It syncs the inbox, opens messages, checks shipment chips, toggles a flag
  and confirms it persisted, checks the optimistic-delete rollback (the fixture has no Trash),
  searches the cache, and uses the palette.

### Hermetic test fixture (debug-only env hooks)

E2E tests run against a **disposable GreenMail IMAPS server** (`e2e/docker-compose.yml`) seeded
with fixed fixture emails (`e2e/fixtures/mail/`, delivered by `node e2e/seed.mjs`), not a real
mailbox. The test points `XDG_CONFIG_HOME`/`XDG_DATA_HOME` at a throwaway dir holding
`e2e/profile/accounts.json`, so a run never touches real accounts or the mail DB (both resolve
via `dirs::config_dir`/`dirs::data_dir`). Two **`debug_assertions`-only, env-gated** hooks
bridge the backend to the fixture; both are compiled out of release and inert unless set:

- **`COSMIC_MAIL_EXTRA_CA`** (`sync/imap.rs::tls_config`) — path to a PEM whose certs are
  added to the rustls trust store, so the app accepts the fixture's committed self-signed
  `localhost` cert (`e2e/fixtures/tls/ca.pem`). The strict-TLS production path is unchanged.
- **`COSMIC_MAIL_TEST_IMAP_PASSWORD`** (`accounts.rs::get_imap_password`) — returned as the
  IMAP password instead of the keyring, so tests need no Secret Service (e.g. headless CI).

CI (`.github/workflows/e2e.yml`) starts and seeds the fixture, then runs
`cargo test --test e2e_greenmail -- --ignored`; no display server or GPU is needed.

## Conventions

- Rust: edition 2021, toolchain pinned by `rust-toolchain.toml` (Rune requires it), `anyhow`
  internally, `thiserror` at the `Backend` boundary, `tracing` for logs.
- No `unwrap()` outside tests/main. UI state changes only in handlers, `Ui::poll`, and jobs
  landed through the `Inbox`; views are pure functions of `Ui`.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` must pass.
  Do not commit — the supervisor handles git.
