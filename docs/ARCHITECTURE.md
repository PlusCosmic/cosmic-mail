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
  error.rs          # AppError (thiserror) -> serialized as String to frontend
  state.rs          # AppState: db pool, account registry, sync handles
  commands.rs       # all #[tauri::command] fns (thin; delegate to modules)
  store.rs          # rusqlite: schema, migrations, queries
  accounts.rs       # Account model, accounts.json persistence, keyring secrets
  omarchy.rs        # theme reader + file watcher -> "omarchy:theme-changed" event
  notifications.rs  # notify-rust wrapper (mako-aware)
  sync/
    mod.rs          # SyncManager: per-account background task, IDLE loop
    imap.rs         # async-imap + tokio-rustls connector, LOGIN + XOAUTH2
  auth/
    oauth.rs        # Gmail OAuth2 (PKCE + loopback redirect), token refresh
src/                # SvelteKit (adapter-static, SPA)
  lib/api.ts        # typed invoke() wrappers — mirrors commands below
  lib/types.ts      # TS mirrors of the wire types below
  lib/theme.ts      # applies OmarchyTheme as CSS custom properties
  lib/stores/       # Svelte 5 runes-based state
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

interface MessageBody {
  id: number;
  html: string | null;        // sanitization happens frontend-side before render
  text: string | null;
  toAddrs: string[];
  ccAddrs: string[];
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
```

## Tauri commands (exact names)

| command | args | returns |
|---|---|---|
| `get_theme` | — | `OmarchyTheme` |
| `list_accounts` | — | `Account[]` |
| `add_imap_account` | `input: ImapAccountInput` | `Account` (validates by connecting first) |
| `start_gmail_oauth` | — | `Account` (opens browser, blocks until redirect or 5 min timeout) |
| `remove_account` | `accountId: string` | `void` |
| `list_folders` | `accountId: string` | `Folder[]` |
| `list_messages` | `folderId: number, offset: number, limit: number` | `MessageSummary[]` (date DESC) |
| `list_unified_messages` | `offset: number, limit: number` | `MessageSummary[]` (date DESC across all folders with role 'inbox', all accounts) |
| `get_message_body` | `messageId: number` | `MessageBody` (fetches from server if not cached) |
| `mark_read` | `messageId: number, seen: boolean` | `void` (updates server flag + db) |
| `send_message` | `input: SendMessageInput` | `void` (submits through the selected account's SMTP server) |
| `sync_folder` | `folderId: number` | `void` (triggers refresh; progress via events) |
| `sync_account` | `accountId: string` | `void` |
| `test_notification` | — | `void` (sends a sample mako notification) |
| `discover_account_config` | `email: string` | `DiscoveredConfig` (never errors on "not found" — falls back to a `guess` with `confident: false`; errors only on invalid email) |

Errors: commands return `Result<T, String>` — human-readable message, frontend shows it in a toast.

## Events (backend -> frontend, via `AppHandle::emit`)

| event | payload |
|---|---|
| `omarchy:theme-changed` | `OmarchyTheme` |
| `mail:new-messages` | `{ accountId: string, folderId: number, messages: MessageSummary[] }` |
| `mail:messages-updated` | `{ folderId: number }` (flags changed / deletions — frontend refetches page) |
| `mail:sync-state` | `{ accountId: string, state: SyncState, error: string | null }` |

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
  body_text TEXT, body_html TEXT,             -- NULL until body fetched
  UNIQUE(folder_id, uid)
);
CREATE INDEX IF NOT EXISTS idx_messages_folder_date ON messages(folder_id, date DESC);
```

Account configs (non-secret) live in `$XDG_CONFIG_HOME/cosmic-mail/accounts.json`.

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
  `graphical-session.target`. It launches the current promoted binary with `--background`, keeps
  it alive with `Restart=always`, and stops it with the graphical session. Development runs do
  not install or control this service.
- The configured main webview is initially hidden. A normal first launch shows/focuses it; a
  `--background` first launch leaves it hidden while sync, IDLE, theme watching, and notification
  action listeners run normally.
- Closing the main window prevents destruction and hides it. This does not stop the process or
  its sync tasks. Session shutdown/systemd stop remains an actual process exit.
- A second normal launcher invocation is an explicit activation request: the owner shows,
  unminimizes, and focuses the existing main window. A second `--background` invocation is silent
  so service restarts cannot steal focus. Notification default actions use the same activation
  path, including the fixed-argument Hyprland focus fallback described above.

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
  (mismatch ⇒ wipe folder cache) → fetch/upsert envelopes → emit events/notify. Then IDLE on
  INBOX; on IDLE wakeup or 25-min timeout, re-sync. On error: emit sync-state error, retry with
  backoff (30s → 5 min cap).
- Initial sync: use message sequence numbers from `STATUS MESSAGES` to `FETCH` exactly the
  newest 200 existing messages per folder (or all when fewer than 200), including `UID` in the
  response. Do not `UID FETCH 1:*` and trim afterward. Bodies remain lazy.
- Incremental sync: `UID FETCH <cached-max-uid+1>:*` and ignore any duplicate boundary item.
  Folder `totalCount`/`unreadCount` stay server-authoritative even when only 200 envelopes are
  cached; successful local flag changes adjust the stored server count until the next STATUS.
- rustls: `tokio_rustls` with `rustls_native_certs` root store.

## Conventions

- Rust: edition 2021, `anyhow` internally, `thiserror` at command boundary, `tracing` for logs.
- No `unwrap()` outside tests/main. Frontend: strict TS, Svelte 5 runes ($state/$derived/$effect).
- `npm run check` and `cargo check` must pass. Do not commit — the supervisor handles git.
