# Cosmic Mail — Architecture Contract

Native Linux mail client for [omarchy](https://omarchy.org), built with Tauri 2 + Svelte 5.
Supports plain IMAP accounts and Gmail (OAuth2/XOAUTH2 over IMAP — one sync engine for both).

**This document is the contract between the Rust backend and the Svelte frontend. Both sides
must conform to it exactly.** All serde structs use `#[serde(rename_all = "camelCase")]`.
Tauri command args arrive camelCase on the Rust side too (Tauri converts JS camelCase args to
snake_case parameter names automatically — declare Rust params in snake_case as usual).

## Module layout

```
src-tauri/src/
  lib.rs            # tauri::Builder wiring, state setup, background task spawn
  main.rs
  desktop.rs        # process/window lifecycle, launcher + notification activation
  tray.rs           # persistent system tray and Open / Sync now / Quit menu
  error.rs          # AppError (thiserror) -> serialized as String to frontend
  state.rs          # AppState: db pool, account registry, sync handles
  commands.rs       # all #[tauri::command] fns (thin; delegate to modules)
  store.rs          # rusqlite: schema, migrations, queries
  accounts.rs       # Account model, accounts.json persistence, keyring secrets
  omarchy.rs        # theme reader + file watcher -> "omarchy:theme-changed" event
  notifications.rs  # notify-rust wrapper (mako-aware)
  automation.rs     # DEBUG BUILDS ONLY: loopback E2E bridge (see section below)
  sync/
    mod.rs          # SyncManager: per-account background task, IDLE loop
    imap.rs         # async-imap + tokio-rustls connector, LOGIN + XOAUTH2
  auth/
    oauth.rs        # Gmail OAuth2 (PKCE + loopback redirect), token refresh
src/                # SvelteKit (adapter-static, SPA)
  lib/api.ts        # typed invoke() wrappers — mirrors commands below
  lib/types.ts      # TS mirrors of the wire types below
  lib/theme.ts      # applies OmarchyTheme as CSS custom properties
  lib/palette.ts    # command-palette fuzzy ranking (pure, unit-tested; no backend surface)
  lib/stores/       # Svelte 5 runes-based state
  lib/components/   # *.svelte (incl. CommandPalette.svelte — the Ctrl+K overlay)
  routes/+page.svelte  # app shell
prototypes/         # static HTML/CSS design mockups (no build step)
```

## Wire types (TS names; Rust equivalents in parens)

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
  html: string | null;        // sanitization happens frontend-side before render;
                              // inline cid: images are inlined as data: URIs at cache time (see policy)
  text: string | null;
  toAddrs: string[];
  ccAddrs: string[];
  attachments: AttachmentInfo[];  // parsed from the body; empty until the body is cached
}

type ShipmentCarrier = "ups" | "fedex" | "usps" | "dhl" | "royal_mail" | "amazon";

interface Shipment {          // shipments.rs::Carrier + store.rs::ShipmentRow projection
  id: number;                 // shipments.id rowid
  carrier: ShipmentCarrier;   // stable lowercase code; frontend maps to a display label/glyph
  trackingNumber: string | null;
  trackingUrl: string | null; // captured from the email, or a synthesized carrier tracking-page
                              // URL from trackingNumber (never synthesized for Amazon — no
                              // generic public tracking template exists)
  orderId: string | null;     // Amazon order id (\d{3}-\d{7}-\d{7}) when the email carried one
  detectedAt: string;         // RFC 3339, when this row was (re)detected
}

interface OmarchyTheme {
  name: string;               // e.g. "kanagawa" (~/.config/omarchy/current/theme.name)
  accent: string; foreground: string; background: string; cursor: string;
  selectionForeground: string; selectionBackground: string;
  palette: string[];          // color0..color15 as "#rrggbb"
}

type SyncState = "idle" | "syncing" | "error";

type DiscoverySource = "autoconfig" | "ispdb" | "mx" | "srv" | "guess";

interface DiscoveredConfig {      // autoconfig.rs — settings discovery result
  kind: AccountKind;              // "gmail" ⇒ frontend steers user to the Gmail OAuth tab
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
}

interface SendMessageInput {
  accountId: string;                    // sending identity; SMTP details stay backend-only
  toAddrs: string[]; ccAddrs: string[]; bccAddrs: string[];
  subject: string;
  bodyText: string;                     // initial compose implementation is plain text
  replyToMessageId: number | null;      // local message rowid, never an arbitrary RFC header
}

interface Settings {                    // settings.rs::Settings — global preferences
  alwaysDownloadRemoteImages: boolean;  // off by default; see "Reader remote-content policy"
}
```

## Tauri commands (exact names)

| command | args | returns |
|---|---|---|
| `get_theme` | — | `OmarchyTheme` |
| `list_accounts` | — | `Account[]` |
| `add_imap_account` | `input: ImapAccountInput` | `Account` (validates by connecting first) |
| `start_gmail_oauth` | — | `Account` (opens browser, blocks until redirect or 5 min timeout) |
| `reauth_gmail_account` | `accountId: string` | `Account` (re-runs the same interactive consent for an **existing** Gmail account, in place — cache/folders/settings untouched; errors without storing anything if the completed consent belongs to a different address than the account's) |
| `remove_account` | `accountId: string` | `void` |
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
| `test_notification` | — | `void` (sends a sample mako notification) |
| `discover_account_config` | `email: string` | `DiscoveredConfig` (never errors on "not found" — falls back to a `guess` with `confident: false`; errors only on invalid email) |
| `get_settings` | — | `Settings` (never errors; a missing/malformed settings file yields defaults) |
| `update_settings` | `settings: Settings` | `Settings` (persists to `settings.json`, returns the stored value) |

Errors: commands return `Result<T, String>` — human-readable message, frontend shows it in a toast.

## Events (backend -> frontend, via `AppHandle::emit`)

| event | payload |
|---|---|
| `omarchy:theme-changed` | `OmarchyTheme` |
| `mail:new-messages` | `{ accountId: string, folderId: number, messages: MessageSummary[] }` |
| `mail:messages-updated` | `{ folderId: number }` (flags changed / deletions — frontend refetches page) |
| `mail:sync-state` | `{ accountId: string, state: SyncState, error: string | null, needsReauth: boolean }` (`needsReauth` is `true` only when the failure is classified `AuthExpired` — see the Gmail section — meaning retrying cannot help and the UI should offer Reconnect) |

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
when sync cannot run, e.g. a missing OAuth token) and before the webview requests its first
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
    Use Tauri's portable show/unminimize/focus path first; under Hyprland, follow it with a
    fixed-argument `hyprctl dispatch focuswindow class:^(cosmic-mail)$` fallback because
    Wayland focus-stealing prevention can ignore a background `set_focus()` call. Never invoke
    this through a shell or interpolate notification/message content into the command.
  - summary: `New mail — {fromName}`, body: `{subject}`. Batch: if >3 new messages in one
    sync pass for a folder, send ONE notification: `{n} new messages in {account email}`.
  - Only notify for folders with role `inbox`, only for UIDs above `last_seen_uid`, never
    during an account's initial sync.
- Theme: read `~/.config/omarchy/current/theme/colors.toml` (keys: accent, foreground,
  background, cursor, selection_foreground, selection_background, color0..color15) and
  `~/.config/omarchy/current/theme.name`. `~/.config/omarchy/current/theme` is a symlink
  swapped by `omarchy-theme-set`; watch the **parent directory** `~/.config/omarchy/current`
  with the `notify` crate (watch for any event, then debounce ~300ms, re-read, emit
  `omarchy:theme-changed`). Fall back to built-in kanagawa values if files are missing.
- Frontend maps OmarchyTheme onto CSS custom properties (see prototypes): `--bg`, `--fg`,
  `--accent`, `--cursor`, `--sel-bg`, `--sel-fg`, `--c0`..`--c15`. All UI colors derive from
  these; no hardcoded colors outside the fallback.

## Frontend pane layout

- The account rail and optional 190px folder column remain fixed-width. The message list and
  reader share all remaining shell width, separated by a pointer-draggable, keyboard-focusable
  vertical separator.
- The separator constrains both panes to usable minimum widths. The message list also has a
  readable maximum width so maximized and full-screen windows give surplus space to the reader
  instead of stretching message rows indefinitely.
- Persist the preferred message-list proportion in browser local storage under
  `cosmic-mail:list-pane-ratio`. A proportion, rather than a pixel width, keeps the preference
  useful when the window is resized or the folder column appears. Re-apply current constraints
  whenever the available width changes; malformed or unavailable storage falls back safely.
- Left/Right arrows resize the focused separator (Shift uses a larger step), Home/End choose its
  minimum/maximum, and double-click restores the default 42% split. The separator exposes the
  current and permitted list widths through ARIA separator values.

## Process and window lifecycle

- Cosmic Mail has exactly one process/sync-engine owner per desktop session. Tauri's
  single-instance plugin owns `dev.pluscosmic.mail.SingleInstance` on the session D-Bus; a
  second process forwards its argv to that owner and exits before application setup can start a
  second set of sync tasks.
- The promoted daily build installs `cosmic-mail.service`, a systemd user service bound to
  `graphical-session.target`. It launches the current promoted binary with `--background`, uses
  `Restart=on-failure` for crash recovery, and stops it with the graphical session. A clean tray
  Quit therefore remains stopped until the service is explicitly started or the next graphical
  session. Development runs do not install or control this service.
- The configured main webview is initially hidden. A normal first launch shows/focuses it; a
  `--background` first launch leaves it hidden while sync, IDLE, theme watching, and notification
  action listeners run normally.
- Closing the main window prevents destruction and hides it. This does not stop the process or
  its sync tasks. Session shutdown/systemd stop remains an actual process exit.
- A second normal launcher invocation is an explicit activation request: the owner shows,
  unminimizes, and focuses the existing main window. A second `--background` invocation is silent
  so service restarts cannot steal focus. Notification default actions use the same activation
  path, including the fixed-argument Hyprland focus fallback described above.
- Desktop builds create one Tauri tray icon for the process lifetime. Its attached menu is kept
  for the tray lifetime and contains **Open Cosmic Mail**, **Sync now**, and **Quit**. Open uses
  the same activation path as launcher and notification actions. Sync now restarts the background
  sync task for every configured account. Quit requests a clean application exit; it does not
  merely hide the window.
- Linux tray interaction is menu-driven. The backend does not depend on tray pointer events,
  tooltips, or tray-rectangle queries, and it does not remove or replace the attached menu. Linux
  bundles declare the detected compatible Ayatana/AppIndicator GTK3 runtime through Tauri's
  built-in packaging path; Cosmic Mail uses only Tauri's tray API rather than calling that library
  directly.

## Gmail (auth/oauth.rs)

- OAuth 2.0 authorization-code + PKCE, loopback redirect (`http://127.0.0.1:<random port>`),
  per RFC 8252. Open the consent URL with `tauri-plugin-opener`. Scope: `https://mail.google.com/`
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
  valid To/Cc/Bcc mailbox and builds all address and message headers with lettre; the frontend never
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

## Message actions (commands.rs + sync/imap.rs)

Flag/archive/delete/move follow the same shape as `mark_read`: locate the row → open a
per-command IMAP connection → select the source folder → run the server op → update the DB →
emit events. All are server-authoritative; the local cache is adjusted optimistically on the
frontend and reconciled by the emitted `mail:messages-updated` events (which refetch the page
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
  connection in an INBOX idle loop: re-sync INBOX (STATUS counts + incremental fetch + notify)
  → IDLE → on wakeup, re-sync INBOX on the same connection and IDLE again — until the 25-min
  full-resync deadline ends the cycle and the loop reconnects for a fresh full sweep. A re-sync
  precedes *every* IDLE, so mail arriving while the sweep was busy (whose untagged EXISTS the
  SELECT swallows and IDLE will not re-announce) is caught immediately, and the deadline is a
  hard wall-clock bound (async-imap's IDLE timeout is inactivity-based; server keepalives reset
  it). On error: emit sync-state error, retry with backoff (30s → 5 min cap).
- Initial sync: use message sequence numbers from `STATUS MESSAGES` to `FETCH` exactly the
  newest 200 existing messages per folder (or all when fewer than 200), including `UID` in the
  response. Do not `UID FETCH 1:*` and trim afterward.
- Incremental sync: `UID FETCH <cached-max-uid+1>:*` and ignore any duplicate boundary item.
  Folder `totalCount`/`unreadCount` stay server-authoritative even when only 200 envelopes are
  cached; successful local flag changes adjust the stored server count until the next STATUS.
- After each inbox envelope sync, opportunistically cache at most 5 bodies from the newest 20
  unread and newest 20 overall cached envelopes. Prefetch is sequential and best-effort so an
  individual body failure does not fail the sync cycle. Skip messages larger than 1 MiB, using
  a separate `RFC822.SIZE` lookup when a pre-migration envelope has no stored size. Fetch full
  messages with `BODY.PEEK[]`, never `BODY[]`/`RFC822`, so caching cannot set `\Seen`; only the
  explicit `mark_read` command changes that flag. Parsing and caching do not render HTML or load
  network resources. Foreground cache misses use the same non-marking fetch.
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

- HTML message bodies are sanitized with DOMPurify and rendered in a sandboxed `srcdoc` iframe.
  Sender scripts, style elements, frames, objects, embeds, forms, and `srcset` remain forbidden.
- Every generated iframe document places a Content Security Policy before sender content. The
  default policy denies all network resources while allowing trusted inline reader styles and
  embedded `data:`/`cid:` images. This also blocks tracking pixels referenced by inline CSS.
- Inline `cid:` images are resolved to `data:` URIs backend-side at body-cache time — never by a
  network fetch. When a body is parsed, each `image/*` part carrying a Content-ID whose decoded size
  is ≤ 512 KiB, and while a running per-message budget of ≤ 2 MiB is not exceeded, has its stored
  `body_html` `cid:<content-id>` references rewritten (case-insensitive, exact string match) to
  `data:<mime>;base64,<payload>` before caching. Over-cap parts keep their `cid:` reference, which
  renders blank under the CSP (`cid:` has no resolver in the sandboxed frame) — safe by construction.
  This happens without rendering HTML or loading any resource.
- A visible reader action may allow HTTP(S) image resources for the selected message.
  Consent is keyed by message rowid, kept only for the current frontend session, and never carries
  over to another message or persists across application restarts. Sanitization and sandboxing are
  unchanged when remote content is allowed.
- The global `Settings.alwaysDownloadRemoteImages` preference (off by default; persisted in
  `settings.json`) relaxes the block for **images only**: when on, every HTML message renders with the
  same `img-src … http: https:` CSP the per-message opt-in uses, and the per-message consent control is
  hidden because it is already satisfied. This changes nothing else — DOMPurify sanitization, iframe
  sandboxing, and every non-image CSP directive (scripts, media, objects, frames, forms) stay exactly
  as they are with the setting off. When the preference is off, per-message session consent semantics
  above are unchanged. Turning the preference off applies to subsequently rendered messages.
- Links never navigate the sandbox or reach the app: the iframe runs with `sandbox="allow-scripts"`
  only (no `allow-popups`, no `allow-same-origin`), giving it an opaque origin — no parent DOM
  access, no Tauri IPC, and a new-window request still can't reach the system browser directly.
  Click handling lives inside the frame rather than being observed from the parent, because
  WebKitGTK keeps a sandboxed `srcdoc` iframe's `contentDocument` null once the real document
  loads. `messageFrameDocument` injects `LINK_FORWARDER_SCRIPT` into the frame's `<head>`, ahead
  of sanitized sender HTML in `<body>` (DOMPurify already strips `<script>`, so sender content
  can never pre-empt or terminate it — the head `<script>` tag is fully closed in the template
  before sender markup is inserted into `<body>`). The forwarder listens for `click`/`auxclick` on
  `a[href]`, calls `preventDefault()` unconditionally, and `postMessage`s
  `{type: OPEN_LINK_MESSAGE_TYPE, href}` to the parent window. The frame CSP gains
  `script-src 'unsafe-inline'` to permit only this inline forwarder script; `default-src 'none'`
  still blocks the network, so the frame cannot fetch or exfiltrate anything else.
  `Reader.svelte` listens for `message` events in a `$effect`, validates
  `event.source === iframeEl.contentWindow` and the message shape, then resolves the href with
  `resolveOpenableLinkUrl(href, "about:srcdoc")` (http/https only, in `message-html.ts`) before
  forwarding to `@tauri-apps/plugin-opener`'s `openUrl`. The `opener:allow-open-url` capability
  permission is scoped to `http://*` / `https://*` only — narrower than the plugin's
  `opener:default` set (which also grants `mailto:`/`tel:` and `reveal_item_in_dir`, unneeded
  here). Gmail OAuth's own consent-URL open (`auth/oauth.rs`) calls the Rust `OpenerExt::open_url`
  API directly and is unaffected by this capability, since ACL scoping only gates the JS-invoked
  command.

## Automation bridge (debug builds only — `automation.rs`)

Scripted end-to-end tests drive the real webview through an in-app bridge. Arch/CachyOS
`webkit2gtk-4.1` ships no `WebKitWebDriver`, so the standard `tauri-driver` route is
unavailable; the bridge replaces it. **It exists only in `debug_assertions` builds** —
`lib.rs` gates both `mod automation;` and the `automation::spawn` call on `cfg`, so it is
compiled out of every release/promoted binary. It never appears in the invoke handler and
adds no wire types or events.

- **Transport.** A raw loopback HTTP/1.1 listener on `127.0.0.1:4127` (override with
  `COSMIC_MAIL_AUTOMATION_PORT`), on tokio (no new crates). Binds loopback only; one
  request per `Connection: close`. Bind failure is logged and non-fatal. Never handles
  secrets.
- **`GET /health`** → `200 {"ok":true}` once the main webview exists; `503` otherwise. A
  client's readiness wait gates on this, so it means "UI drivable", not just "socket up".
- **`POST /eval`**, body = a JS **function body** → runs it in the main webview via
  `WebviewWindow::eval_with_callback` (WebKitGTK `run_javascript` → `js_value().to_json`)
  and returns the value. The snippet is wrapped in a try/catch IIFE, so:
  - success → `{"ok":true,"value":<the JSON value; undefined coerced to null>}`
  - a throw → `{"ok":false,"error":"<message>"}`
  Only JSON-serializable return values survive (strings, numbers, booleans, arrays, plain
  objects) — return element *text*/counts, not DOM nodes.
- **No promise awaiting.** WebKitGTK evaluates each snippet synchronously and does not
  await a returned Promise. Waiting for async UI (a click that renders a pane) is the
  client's job, done by polling `/eval` until a condition holds — never by returning a
  Promise from the snippet.
- **Client + tests** live in `e2e/` (`client.mjs` `Bridge`, `*.test.mjs` on `node --test`,
  `npm run e2e`). See `e2e/README.md` for the run recipe and the single-instance/port
  constraints.

### Hermetic test fixture (debug-only env hooks)

Tests run against a **disposable GreenMail IMAPS server** (`e2e/docker-compose.yml`) seeded
with fixed fixture emails (`e2e/fixtures/mail/`), not a real mailbox. The app is launched
against an **isolated XDG profile** — `XDG_CONFIG_HOME`/`XDG_DATA_HOME` pointed at a
throwaway dir with `e2e/profile/accounts.json` — so a run never touches real accounts or the
mail DB (both resolve via `dirs::config_dir`/`dirs::data_dir`). Two **`debug_assertions`-only,
env-gated** hooks bridge the app to the fixture; both are compiled out of release and inert
unless their env var is set:

- **`COSMIC_MAIL_EXTRA_CA`** (`sync/imap.rs::tls_config`) — path to a PEM whose certs are
  added to the rustls trust store, so the app accepts the fixture's committed self-signed
  `localhost` cert (`e2e/fixtures/tls/ca.pem`). The strict-TLS production path is unchanged.
- **`COSMIC_MAIL_TEST_IMAP_PASSWORD`** (`accounts.rs::get_imap_password`) — returned as the
  IMAP password instead of the keyring, so tests need no Secret Service (e.g. headless CI).

Also: **tray setup is non-fatal** (`lib.rs`) — a missing StatusNotifier host (headless CI
under Xvfb) logs a warning instead of aborting startup before the webview/bridge come up. CI
(`.github/workflows/e2e.yml`) builds a debug binary (`tauri build --debug --no-bundle`) and
runs it under `dbus-run-session -- xvfb-run` via `e2e/ci-run.sh`.

## Conventions

- Rust: edition 2021, `anyhow` internally, `thiserror` at command boundary, `tracing` for logs.
- No `unwrap()` outside tests/main. Frontend: strict TS, Svelte 5 runes ($state/$derived/$effect).
- `npm run check` and `cargo check` must pass. Do not commit — the supervisor handles git.
