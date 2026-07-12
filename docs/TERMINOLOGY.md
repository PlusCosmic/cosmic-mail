# Shared terminology

Use these terms in issues, feature descriptions, documentation, and code discussions. The
definitions describe the current product and implementation; they do not replace the API contract
in [ARCHITECTURE.md](ARCHITECTURE.md).

## Interface

| term | definition |
|---|---|
| **app shell** | The main Cosmic Mail window and its persistent layout: account rail, header, content area, and footer. |
| **account rail** | The narrow leftmost navigation strip. It switches between the unified inbox and accounts and holds compose and add-account actions. `Rail.svelte` implements it. Use **rail** only when the context is already clear. |
| **folder column** | The optional column beside the account rail that lists folders for the selected account. It is hidden in the unified inbox. `FolderColumn.svelte` implements it. |
| **header** | The bar above the content area containing the current view title, account/unread summary, loaded-message filter field, and overall sync status. |
| **message list** | The pane containing filter chips and message rows for the current unified inbox or folder. `MessageList.svelte` implements it. Do not call it the inbox because it can show any folder. |
| **message row** | One selectable summary in the message list, showing sender, date, subject, snippet, read state, and relevant account or flag metadata. |
| **reader** | The pane that displays the selected message's headers, actions, and body. `Reader.svelte` implements it. Prefer **reader** over preview pane. |
| **pane separator** | The draggable, keyboard-focusable vertical control between the message list and reader. It changes the message-list/reader width split. |
| **footer** | The bottom bar containing keyboard hints, per-account sync indicators, and the selected-message position. |
| **unified inbox** | The date-ordered collection of messages from every folder whose role is `inbox`, across all accounts. It is not a stored folder of its own. |
| **unified view** | The UI mode that shows the unified inbox and hides the folder column. `mail.unified` represents this mode. |
| **account view** | The UI mode for one account. Selecting an account opens its inbox (or first available folder) and shows the folder column; selecting another folder stays in this mode. The store also calls this per-account or per-folder mode. |
| **selected message** | The message row currently targeted by keyboard navigation or pointer selection and displayed in the reader. Opening it fetches its body if needed and marks it read. |
| **loaded-message filter** | The header text field that filters messages already loaded into the frontend by sender, subject, or snippet. It is not full local or server search. Use **filter** for the current feature and reserve **search** for the planned indexed/server-backed feature. |
| **filter chips** | The `All`, `Unread`, and `Flagged` controls at the top of the message list. They filter the currently loaded messages client-side. |
| **compose dialog** | The modal surface for creating a new message or reply and sending it through a selected account. Compose is currently plain text. |
| **add-account dialog** | The modal surface for adding a password-authenticated IMAP account or starting Gmail sign-in. |
| **toast** | A temporary in-app informational or error message. A toast is not a desktop notification. |
| **desktop notification** | A new-mail notification sent over the freedesktop notification interface and rendered by Mako. Activating a live notification focuses the existing app window. |

## Mail model

| term | definition |
|---|---|
| **account** | One configured email identity and its incoming/outgoing server settings. Its public projection contains an ID, email, display name, and account kind; secrets remain in the keyring. |
| **IMAP account** | An account configured with IMAP/SMTP settings and a password. In authentication-specific discussion, **password account** distinguishes it from Gmail OAuth. |
| **Gmail account** | An account authenticated with Google OAuth2 and XOAUTH2. It still uses the shared IMAP sync engine and SMTP sending path, not the Gmail REST API. |
| **folder** | Cosmic Mail's UI, wire, and database term for an IMAP mailbox. A folder belongs to one account and has a server path, role, unread count, and total count. Use **mailbox** only when discussing IMAP protocol behavior. |
| **folder role** | The normalized purpose of a folder: inbox, sent, drafts, trash, archive, spam, or normal. It comes from IMAP SPECIAL-USE attributes with name-based fallback heuristics. |
| **message** | An email represented locally by a summary and, once fetched, a body. A cached copy belongs to one folder and account. |
| **message summary** | The lightweight `MessageSummary` used by message rows: sender, date, subject, snippet, flags, attachment indication, and identifiers. It does not contain the renderable body. |
| **envelope** | Sync/cache shorthand for message metadata fetched to build a message summary. In this project, an envelope sync can also collect fields such as flags, size, and a snippet; it does not imply that the full body is cached. |
| **snippet** | The single-line, roughly 160-character text extract shown in a message row. |
| **message body** | The lazily fetched `MessageBody`: HTML and/or plain text plus parsed To and Cc recipients. HTML is sanitized before the reader renders it. |
| **read state** | Whether a message has the IMAP `\\Seen` flag. The wire field is `seen`; the UI describes messages as read or unread. |
| **flagged state** | Whether a message has the IMAP `\\Flagged` flag. This is independent of read state. |
| **local message ID** | The stable SQLite row ID in `MessageSummary.id`. Frontend commands use this ID to identify a cached message. |
| **IMAP UID** | A server-assigned message identifier that is unique only within a folder and its current UIDVALIDITY epoch. The wire field is `MessageSummary.uid`. |
| **Message-ID** | The RFC 5322 header used for email threading. It is distinct from both the local message ID and IMAP UID. |

## Data and synchronization

| term | definition |
|---|---|
| **local cache** | The SQLite store of folders, message summaries, and fetched bodies. It is not necessarily a complete copy of the server; initial sync currently starts with the newest 200 envelopes per folder and later incremental syncs can add to them. |
| **body cache** | The portion of the local cache containing a message's fetched and parsed body. An explicitly cached empty body is distinct from a body that has not been fetched. |
| **body fetch** | Retrieval of a full message with `BODY.PEEK[]` when its body is not cached. It must not mark the message read. |
| **body prefetch** | Bounded, best-effort background body fetching for recent or unread inbox messages so likely selections open immediately. It follows the same non-marking and size-limit rules as foreground body fetches. |
| **sync engine** | The Rust subsystem that runs one background task per account, maintains the local cache, emits frontend events, and drives new-mail notifications. |
| **sync cycle** | One account refresh pass: connect, discover folders, obtain server status, reconcile cache validity, fetch new metadata, emit updates, and then return to waiting on the inbox. |
| **initial sync** | The first sync of a folder or a sync after cache invalidation. It fetches up to the newest 200 existing envelopes and suppresses new-mail notifications. |
| **incremental sync** | A later sync that fetches messages above the highest cached IMAP UID instead of repeating the initial range. |
| **IMAP IDLE** | The server-waiting phase used for near-real-time inbox change detection. A wakeup triggers another sync; it is not itself the cache update. |
| **authoritative count** | A folder's server-reported `MESSAGES` total or `UNSEEN` unread count from IMAP STATUS. These counts can exceed the number of locally cached message summaries. |
| **remote content** | HTTP(S) images referenced by an HTML message. The reader blocks them by default and current consent applies only to one message for the frontend session. Embedded `data:` and `cid:` images are not remote content. |
| **account discovery** | Resolution of likely IMAP/SMTP settings from an email address using provider autoconfig, ISPDB, MX, SRV, or a final unverified guess. Discovery does not authenticate with the mail server. |

## Application architecture

| term | definition |
|---|---|
| **frontend** | The Svelte application under `src/`, responsible for the shell, interaction state, message rendering, and typed calls into Tauri. |
| **backend** | The Rust application core under `src-tauri/`, responsible for accounts, secrets, storage, network protocols, sync, notifications, and desktop lifecycle. |
| **wire type** | A camelCase-serialized data shape exchanged between the Rust backend and TypeScript frontend. `ARCHITECTURE.md` is its binding contract. |
| **command** | A typed frontend-to-backend Tauri invocation, such as `list_messages` or `mark_read`. |
| **event** | A backend-to-frontend Tauri emission used for theme, message, and sync-state updates. |
| **sync state** | An account's current `idle`, `syncing`, or `error` status exposed to the frontend. `idle` means no sync pass is active; background IDLE waiting may still be running. |
| **process owner** | The single Cosmic Mail process for a desktop session that owns sync tasks, notification listeners, and the main window. Later launches activate this process instead of creating another engine. |
| **background service** | The systemd user service installed by a promoted build. It starts the process owner hidden and keeps it running for the graphical session. |
| **activation** | A launcher invocation or live notification action that reveals, unminimizes, and focuses the existing main window. |
| **Omarchy theme** | The active Omarchy color data read by the backend and mapped to frontend CSS custom properties. Theme changes re-tint the running app and reader styling. |
