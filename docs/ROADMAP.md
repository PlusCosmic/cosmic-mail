# Roadmap & status

Last updated: 2026-07-12.

## Done and verified

- **Scaffold**: Tauri 2 + Svelte 5 (runes) + SvelteKit static adapter, SPA/no-SSR.
- **Rust core** (~2.7k lines): SQLite WAL store, accounts.json + keyring secrets,
  omarchy theme reader/watcher, mako notifications, async-imap sync engine
  (SPECIAL-USE roles, UIDVALIDITY recovery, INBOX IDLE w/ 25-min re-issue + poll
  fallback, 30s→5min backoff), Gmail OAuth2 PKCE loopback → XOAUTH2.
- **Frontend shell**: 3-pane UI, theme CSS vars w/ kanagawa fallback, runes mail store,
  DOMPurify-sanitized reader iframe, vim keys (j/k/gg/G/Enter/Esc/r), add-account modal,
  toasts, status bar.
- **Autoconfig discovery**: provider autoconfig → .well-known → ISPDB → MX→provider/ISPDB
  → SRV → guess; Gmail-hosted detection; modal autofill UX. 9 unit + 3 live tests.
- **Shell converged on prototype 03-hybrid (2026-07-09)**: account icon-rail w/ unread
  badges, unified inbox (`list_unified_messages` across inbox-role folders), per-account
  folder column, slim header (client-side filter input + sync dot), key-hint footer,
  `1`/`2`… view switching, `u` toggle-read (`r` reserved for reply). Old
  Sidebar/StatusBar removed. Command palette NOT included (see Next up).
- Gmail OAuth client credentials now resolve env vars (dev) →
  `$XDG_CONFIG_HOME/cosmic-mail/google-oauth.json` (user) → compile-time baked defaults
  (`COSMIC_MAIL_BUILD_GOOGLE_CLIENT_ID`, for packaged releases);
  window is undecorated (`decorations: false`) for Hyprland.
- **Live-verified end to end (2026-07-09)**: Gmail OAuth sign-in, token refresh, XOAUTH2,
  full folder + 794-message envelope sync on a real account; omarchy theme live re-tint;
  UI prototypes (3, in `prototypes/`).
- **Password IMAP + sync shakedown (2026-07-10)**: Purelymail LOGIN, folder discovery,
  1,497-message INBOX, lazy body fetch, and `mark_read`/unread round-trips live-verified.
  Initial sync now fetches exactly the newest 200 envelopes using sequence numbers instead of
  fetching the full mailbox then trimming; folder totals/unread counts come from IMAP STATUS and
  remain independent of the capped cache. Empty panes show syncing state instead of claiming the
  mailbox has no messages.
- **Notification activation (2026-07-10)**: sample notification delivery and live default-action
  click-to-focus verified with the Hyprland IPC fallback. Restored/expired notifications are not
  valid action tests because their original listener is gone.
- **Background sync + single-instance lifecycle (2026-07-11)**: promoted builds install a
  `graphical-session.target` systemd user service that starts hidden with `--background` and owns
  IMAP IDLE/notifications for the session. Closing the window now hides it without stopping sync;
  Walker and notification activation show/focus the same process through Tauri's session-D-Bus
  single-instance owner. Live isolated release smoke test verified hidden startup, one PID/window
  across a second invocation, and process survival after a Hyprland close request.
- **Resizable, wide-screen-aware panes (2026-07-12)**: the list/reader separator supports pointer
  dragging and keyboard resizing, enforces usable pane bounds, persists a responsive proportional
  preference, and caps list growth so maximized/full-screen reader space is used effectively.
  Double-click resets to the default split; focused sizing logic has unit coverage.
- **Compose / send (2026-07-12)**: plain-text compose UI with account selection, To/Cc/Bcc,
  keyboard send, new/reply/reply-all entry points, quoted replies, and direct-parent
  In-Reply-To/References threading. SMTP uses required STARTTLS or implicit TLS, password
  PLAIN/LOGIN authentication, and Gmail XOAUTH2 through the existing token refresh path. Recipient,
  Bcc, threading-header safety, and reply metadata have unit coverage. Live delivery was verified
  through both Gmail XOAUTH2 and password-authenticated IMAP/SMTP accounts.
- Shakedown fixes landed: WebKit DMA-BUF/Wayland crash, rustls dual-backend panic,
  XOAUTH2 double-encoding, theme-watcher feedback loop (see GOTCHAS.md).

## Not yet verified (built, needs live confirmation)

- **New-mail notification path** (IDLE wakeup → mako toast → click-to-focus): send a
  mail to a synced account while the app runs, expect notification within seconds.

## Next up (rough priority)

1. **Safe message-body prefetch** — opportunistically cache bodies for a bounded set of recent or
   unread messages so keyboard skimming is immediate. Fetch with IMAP `BODY.PEEK[...]` so caching
   never sets `\Seen`; keep the explicit `mark_read` path separate. Prefetch must not render HTML
   or load remote resources, and the reader needs a remote-image policy so tracking pixels/read
   receipts remain separately controlled when a cached message is opened.
2. **Command palette (Ctrl+K)** — walker/telescope-style fuzzy palette from prototype 02
   (the remaining piece of the shell convergence).
3. **Search** — local SQLite FTS5 over envelopes/bodies first; server-side IMAP SEARCH later
   (header input currently only filters loaded messages client-side).
4. **Attachments** — BODYSTRUCTURE part listing, download/save, inline images policy.
5. **Message actions** — archive/delete/move/flag (reader buttons exist but are disabled;
   UI keys `a`/`d` reserved).

## Known limitations / smaller backlog

- IMAP connector is implicit-TLS (993) only — no STARTTLS; discovery skips STARTTLS-only
  providers accordingly.
- IDLE reconnects per cycle (simple, robust, slightly chatty).
- Initial sync = newest 200 envelopes/folder; no full-history backfill UI.
- External flag changes are not reconciled for cached messages: IDLE wakes and STATUS refreshes
  folder counts, but existing rows keep stale `seen`/`flagged` values. After INBOX IDLE wake and
  manual sync, fetch `(UID FLAGS)` for the cached 200 messages, update changed rows, and emit
  `mail:messages-updated`; consider CONDSTORE/QRESYNC later.
- Attachment detection is a BODYSTRUCTURE heuristic.
- Reader iframe: no remote-content blocking toggle yet (sanitized, scripts stripped, but
  remote images load).
- Compose is plain text only. It does not save drafts, attach files, preserve a source message's
  full References chain, honor Reply-To, or locally append to Sent; provider SMTP behavior applies.
- No account-removal control in the UI (backend command + store method exist).
- `G` doesn't auto-trigger "load more"; registrable-domain uses a tiny suffix list, not PSL.
- No app icon yet (scaffold placeholder); no tray icon (would need libappindicator).
