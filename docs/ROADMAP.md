# Roadmap & status

Last updated: 2026-07-15 (known limitations migrated to GitHub issues).

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
  Sidebar/StatusBar removed.
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
- **Safe message-body prefetch (2026-07-12)**: each inbox sync opportunistically caches a bounded
  recent/unread working set with a 5-message cycle cap and 1 MiB per-message limit. All foreground
  and background body fetches use `BODY.PEEK[]`, empty bodies have an explicit cache marker, and
  parsed recipients/snippets are retained. HTML reader frames deny network resources by default;
  remote images require session-only per-message consent without relaxing sanitization.
- **Command palette (2026-07-14)**: walker/telescope-style `Ctrl+K` overlay (prototype 02) with a
  pure, unit-tested fuzzy matcher (`palette.ts`) and a `CommandPalette.svelte` dialog. Ranks
  currently-applicable commands (view/account/folder switching, compose, reply/reply-all, toggle
  read, per-folder/account sync, add account, focus search); arrow/Ctrl-n/p navigation with
  wrap-around, Enter runs, Escape closes, focus restored to the list on close. Frontend-only — no
  new Tauri commands or events.
- **Settings window (2026-07-14)**: a Settings cog in the account rail's bottom action group opens
  a keyboard-accessible `SettingsModal.svelte` (also reachable via the "Open settings" palette
  command). First persisted global preference is `Always download remote images` (off by default;
  `settings.json` via `settings.rs` with `get_settings`/`update_settings` commands). When on, HTML
  messages load HTTP(S) images without per-message consent and the per-message control is hidden;
  DOMPurify sanitization, iframe sandboxing, and every non-image CSP directive are unchanged. Rust
  load/save has unit coverage (defaults on missing/malformed file, round-trip, camelCase on disk).
- **Search (2026-07-14)**: local FTS5 over cached envelopes/bodies. An external-content
  `messages_fts` index (subject/from/snippet/body_text) with insert/delete/update triggers and a
  one-time backfill; `search_messages` command builds a safe prefix MATCH (whitespace-split, quoted,
  escaped — no raw text to MATCH), scopes to one account or all, and ranks by bm25. The header input
  keeps live client-side filtering; Enter runs the backend full-cache search, Escape clears it, and
  an active-search indicator shows the term, count, and a clear affordance. Search suppresses new-mail
  prepends/refreshes into the result list until cleared. Palette "Search all mail" focuses the input.
  Tokenizer/escaping, per-column and prefix matching, account scoping, trigger coverage (rows appear
  after upsert and body-set, disappear on delete), and FTS5 availability have unit coverage.
- **Attachments (2026-07-14)**: message bodies are parsed for attachment metadata at cache time
  (foreground miss and background prefetch) into an `attachments` table; the reader lists non-inline
  attachments as focusable chips (filename + human size) and `save_attachment` refetches the raw
  message (`BODY.PEEK[]`, non-marking), re-parses, decodes the part by its stable index, and writes
  it to the downloads directory with a sanitized, collision-suffixed name. Inline `cid:` images are
  rewritten to `data:` URIs backend-side under strict caps (≤ 512 KiB/part, ≤ 2 MiB/message budget);
  over-cap parts keep their `cid:` reference and render blank under the unchanged reader CSP.
  `has_attachments` is corrected from the real parse once a body is cached. Extraction metadata
  (incl. RFC 2047 filenames), the cid→data rewrite under/over caps, deterministic part indexing,
  filename sanitization, and collision naming have unit coverage.
- **Driven verification session (2026-07-14)**: all five features above exercised in a live dev
  build (synthetic keyboard input + window screenshots + sqlite cross-checks) against real
  accounts. Confirmed: palette open/filter/run (incl. "Open settings" and folder navigation);
  settings modal toggle persisting `settings.json` both ways; global remote-images preference
  rendering remote images with the consent banner hidden when on and restoring the block when off;
  Enter-to-search returning bm25-ranked full-cache results matching the FTS index exactly, with
  indicator/clear; attachment extraction on fresh body fetch, chip save writing a byte-identical
  PDF to `~/Downloads` with `name (1).ext` collision suffixing; and on the Purelymail account:
  flag on/off round-trip, archive → Archive and delete → Trash each confirmed server-side after a
  folder sync (flags preserved across moves), move-picker round-trips back to INBOX, and folder
  totals/unread settling to their original values. Remaining gaps are listed under "Not yet
  verified".
- **Cross-platform system tray (2026-07-14)**: Tauri's built-in `tray-icon` backend owns one
  process-lifetime icon with an attached **Open Cosmic Mail / Sync now / Quit** menu. Open reuses
  the launcher/notification activation path, Sync now restarts every configured account task, and
  Quit cleanly exits instead of hiding the window. The promoted systemd service now restarts only
  on failure so an explicit Quit stays stopped. Linux bundle metadata uses Tauri's detected
  AppIndicator/Ayatana runtime dependency; no direct AppIndicator API is called. Menu ID routing
  has unit coverage, and a generated Debian bundle was verified to declare Ayatana alongside the
  required WebKitGTK/GTK runtimes.
- Shakedown fixes landed: WebKit DMA-BUF/Wayland crash, rustls dual-backend panic,
  XOAUTH2 double-encoding, theme-watcher feedback loop (see GOTCHAS.md).

## Not yet verified (built, needs live confirmation)

- **System tray interactions**: confirm the icon and its three-item menu appear under Waybar from
  both normal and `--background` startup; verify Open focuses the existing window, Sync now
  restarts every account without another process, and Quit leaves the promoted service inactive.
- **New-mail notification path** (IDLE wakeup → mako toast → click-to-focus): send a
  mail to a synced account while the app runs, expect notification within seconds.
- **Inline `cid:` images (live)**: confirm an inline `cid:` image renders in the reader body
  (rewritten to a `data:` URI) and that an over-cap inline image renders blank rather than
  fetching. The rewrite logic has unit coverage; no cached message with inline cid parts was
  available during the 2026-07-14 driven session.
- **Message actions on Gmail**: the full flag/archive/delete/move matrix was live-verified on the
  password-IMAP (Purelymail) account on 2026-07-14 (see Done), but not yet on Gmail. Verify there:
  flag star round-trips; archive lands in All Mail (`\All`) and leaves the inbox; delete moves to
  Trash; **permanent delete from Trash** (untested on either account — it destroys mail, pick a
  disposable message); move to an arbitrary label reappears after sync. Gmail advertises `MOVE`,
  so this also confirms the capability detection; the Purelymail run exercised whichever path its
  capabilities selected — the `UID COPY`+`\Deleted`+`UID EXPUNGE` fallback has no confirmed live
  exercise if both servers advertise `MOVE`.

## Next up (rough priority)

1. **Server-side search** — IMAP SEARCH to reach beyond the local cache (local FTS5 search over
   cached envelopes/bodies already landed; see above). Until then, search covers only the newest
   ~200 envelopes/folder plus cached bodies — mail that was never synced is unreachable.

## Small bugs & UX papercuts → GitHub issues

Small bugs and UX quirks (anything that isn't a roadmap-sized feature) are tracked as
GitHub issues labelled [`papercut`](https://github.com/PlusCosmic/cosmic-mail/issues?q=is%3Aissue+is%3Aopen+label%3Apapercut)
and collected on the [Cosmic Mail Papercuts board](https://github.com/users/PlusCosmic/projects/2).
File new friction there rather than growing this document; this roadmap stays the
source of truth for feature-sized work and status.

The known-limitations list that used to live here was migrated to GitHub issues
[#11–#19](https://github.com/PlusCosmic/cosmic-mail/issues) on 2026-07-15
(papercuts where they fit, `enhancement`/`bug` otherwise).
