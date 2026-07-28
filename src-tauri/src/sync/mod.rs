//! Sync engine: one background task per account.
//!
//! Each task connects, lists folders, performs an initial sync (newest 200
//! envelopes per folder), then holds the connection in an INBOX idle loop:
//! re-sync INBOX → drain-check → IDLE → wakeup → re-sync INBOX → … until the
//! 25-minute full-resync deadline, when the cycle ends and the caller
//! reconnects for a full folder sweep. Re-syncing on the live connection keeps
//! new-mail notification latency at one STATUS+FETCH round trip.
//!
//! A single IDLE command is not held open for the full 25 minutes, though
//! (issue #41): it is capped at the shorter [`IDLE_REISSUE_INTERVAL`] (~5
//! min), and hitting *that* cap alone loops back for another re-sync +
//! drain-check + IDLE on the *same* connection rather than ending the cycle —
//! only hitting the full [`FULL_RESYNC_INTERVAL`] deadline does that. See
//! [`idle_wait_budget`] (which timeout to hand the next `idle_wait` call) and
//! [`idle_timeout_action`] (what a `Timeout` outcome means once it comes
//! back) for the pure decision logic, and their call sites in `run_once` for
//! the ordering. This is on top of, and independent of, TCP keepalive on the
//! socket itself (`sync::imap::tls_client`): keepalive detects a transport
//! that is silently *dead*; the re-issue cadence detects one that is
//! merely idle for a while and gives new-mail latency a shorter worst case
//! even when nothing is transport-wise wrong.
//!
//! The drain-check closes a race that a re-sync-before-every-IDLE alone does
//! not: an IMAP server announces new mail by piggy-backing an untagged
//! `* N EXISTS` onto whatever command response happens to be in flight, and
//! never repeats that announcement later, so it must be caught the instant it
//! arrives or it is gone until the next full resync. `async-imap` routes any
//! such response that lands outside of IDLE's own wait — i.e. on the STATUS
//! and SELECT/FETCH calls a re-sync issues, or even on the IDLE command's own
//! response before its "+ idling" continuation — onto
//! `Session::unsolicited_responses`, a channel nothing else reads. Before
//! entering IDLE we drain that channel with `try_recv` (see
//! [`drain_mailbox_changed`]) and, if anything landed there, re-sync again
//! instead of idling — repeating until a drain comes back clean, bounded by
//! [`MAX_CONSECUTIVE_RESYNCS_BEFORE_IDLE`] so a genuinely hyperactive mailbox
//! can't starve IDLE forever. Because draining is a local, non-blocking read
//! with no `.await` between it and the `session.idle()` call that follows,
//! there is no remaining window for a server response to land unobserved
//! between "we checked" and "we committed to IDLE" (see [`drain_action`]
//! and its call site in `run_once` for the exact ordering, and issue #39 for
//! why a re-sync-precedes-every-IDLE structure alone, tried once already in
//! commit 0f1354e, does not close this race by itself).
//!
//! A small inbox working set is prefetched (without changing `\Seen`; other
//! bodies remain lazy). Issue #40 hoisted that prefetch out of
//! `sync_folder_uids` and into `run_once`'s idle loop, where it sits between
//! the re-sync's notify and the drain-check — after the notify so up to five
//! sequential body downloads can never delay a new-mail toast, and before the
//! drain-check because that check has to be the last thing before
//! `session.idle()`. An `EXISTS` landing on prefetch's own FETCH commands is
//! therefore caught by the drain-check rather than lost. On a single
//! connection that window can be made *safe* but not made to disappear;
//! removing it entirely needs prefetch on a second connection (issue #42).
//! Errors trigger exponential backoff (30s → 5 min). Notifications fire only
//! for new inbox messages above the folder's `last_seen_uid`, and never
//! during the initial sweep.

pub mod imap;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use async_imap::extensions::idle::IdleResponse;
use async_imap::types::UnsolicitedResponse;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::accounts::Account;
use crate::commands::shipment_insert;
use crate::notifications::{self, NewMail};
use crate::shipments;
use crate::state::Db;
use crate::store::{self, FolderRole, MessageUpsert};

const INITIAL_SYNC_LIMIT: u32 = 200;
const BODY_PREFETCH_WORKING_SET: u32 = 20;
const BODY_PREFETCH_LIMIT: u32 = 5;
const BODY_PREFETCH_MAX_BYTES: u32 = 1024 * 1024;
const FULL_RESYNC_INTERVAL: Duration = Duration::from_secs(25 * 60);
/// How long a single IDLE command is allowed to wait before we `DONE` and
/// re-issue it on the same connection (issue #41). This used to be conflated
/// with `FULL_RESYNC_INTERVAL`, so a connection could sit un-exercised for up
/// to 25 minutes; RFC 2177 explicitly anticipates clients re-issuing IDLE
/// periodically, and doing so regularly gives a dead/dropped connection a
/// chance to surface (via the STATUS/SELECT/FETCH re-sync that follows every
/// `DONE`) long before the keepalive settings in `sync::imap` would time it
/// out anyway. Re-issuing is cheap — one `DONE`/`IDLE` round trip — so this
/// can be short without meaningfully increasing IMAP traffic.
const IDLE_REISSUE_INTERVAL: Duration = Duration::from_secs(5 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(60);
const BACKOFF_MIN: Duration = Duration::from_secs(30);
const BACKOFF_MAX: Duration = Duration::from_secs(5 * 60);
/// Cap on re-syncing INBOX again immediately, back to back, without ever
/// entering IDLE, when each drain keeps finding the mailbox changed (see
/// [`drain_action`]). A real mailbox settles within a resync or two;
/// this bound only exists so a pathological case — mail landing on literally
/// every single re-sync's commands, e.g. a hammering test fixture — still
/// idles eventually rather than spinning on STATUS/SELECT/FETCH forever. Kept
/// small: it costs one full IMAP round trip per unit, and giving up on IDLE
/// after a handful of misses is fine because the *next* wakeup or the
/// deadline will catch up regardless.
const MAX_CONSECUTIVE_RESYNCS_BEFORE_IDLE: u32 = 5;

/// Sync state reported to the frontend.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncState {
    Idle,
    Syncing,
    Error,
}

impl SyncState {
    fn as_str(self) -> &'static str {
        match self {
            SyncState::Idle => "idle",
            SyncState::Syncing => "syncing",
            SyncState::Error => "error",
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SyncStatePayload<'a> {
    account_id: &'a str,
    state: &'a str,
    error: Option<String>,
    /// True only when the failure is a classified `AuthExpired` (dead Gmail
    /// credentials) — retrying cannot help; the UI should offer Reconnect.
    needs_reauth: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MessagesUpdatedPayload {
    folder_id: i64,
}

/// Owns per-account sync task handles so they can be aborted on removal.
#[derive(Default)]
pub struct SyncManager {
    handles: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
}

impl SyncManager {
    /// Create an empty manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn (or respawn) the sync task for an account.
    pub fn start(&self, app: AppHandle, db: Db, account: Account) {
        let mut handles = self.handles.lock().expect("sync handles poisoned");
        if let Some(existing) = handles.remove(&account.id) {
            existing.abort();
        }
        let id = account.id.clone();
        let handle = tauri::async_runtime::spawn(async move {
            account_loop(app, db, account).await;
        });
        handles.insert(id, handle);
    }

    /// Abort and forget the sync task for an account.
    pub fn stop(&self, account_id: &str) {
        let mut handles = self.handles.lock().expect("sync handles poisoned");
        if let Some(handle) = handles.remove(account_id) {
            handle.abort();
        }
    }
}

/// Emit `mail:sync-state`.
fn emit_state(
    app: &AppHandle,
    account_id: &str,
    state: SyncState,
    error: Option<String>,
    needs_reauth: bool,
) {
    let _ = app.emit(
        "mail:sync-state",
        SyncStatePayload {
            account_id,
            state: state.as_str(),
            error,
            needs_reauth,
        },
    );
}

/// Top-level per-account loop with error backoff.
async fn account_loop(app: AppHandle, db: Db, account: Account) {
    let span = tracing::info_span!("account_sync", account = %account.email);
    let _guard = span.enter();
    let mut backoff = BACKOFF_MIN;
    let mut first_run = true;

    loop {
        match run_once(&app, &db, &account, first_run).await {
            Ok(()) => {
                backoff = BACKOFF_MIN;
                first_run = false;
                // run_once already reported Idle once real work finished and it
                // settled into the IDLE wait (or poll sleep); the next iteration's
                // run_once reports Syncing again as soon as it reconnects.
            }
            Err(err) => {
                tracing::warn!(error = %err, "sync cycle failed");
                let needs_reauth = crate::auth::oauth::is_auth_expired(&err);
                emit_state(
                    &app,
                    &account.id,
                    SyncState::Error,
                    Some(err.to_string()),
                    needs_reauth,
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

/// One connect → full folder sweep → INBOX idle-loop cycle. The idle loop
/// alternates "re-sync INBOX" and "IDLE" on the same connection, re-issuing
/// IDLE every [`IDLE_REISSUE_INTERVAL`] without ending the cycle, until the
/// full-resync deadline (or a dead connection) ends the cycle, so the caller
/// reconnects fresh and sweeps every folder again.
async fn run_once(app: &AppHandle, db: &Db, account: &Account, initial: bool) -> Result<()> {
    // Real work is about to start: connect, discover folders, fetch/upsert
    // envelopes, prefetch bodies. Report Syncing for exactly this span, not for
    // the IDLE waits that follow (see Idle emissions below).
    emit_state(app, &account.id, SyncState::Syncing, None, false);

    let mut session = imap::connect(account).await?;

    // Discover folders and reconcile with the DB.
    let remote = imap::list_folders_with_roles(&mut session).await?;
    let mut inbox: Option<String> = None;
    for folder in &remote {
        let status = imap::status(&mut session, &folder.name).await?;
        let (folder_id, _wiped) = {
            let conn = db.lock().expect("db poisoned");
            let result = store::upsert_folder(
                &conn,
                &account.id,
                &folder.name,
                folder.role,
                status.uidvalidity,
            )?;
            store::set_folder_counts(&conn, result.0, status.exists, status.unseen)?;
            result
        };
        if folder.role == FolderRole::Inbox {
            inbox = Some(folder.name.clone());
        }
        sync_folder_uids(
            app,
            db,
            account,
            folder_id,
            &folder.name,
            folder.role,
            initial,
            status.exists,
            &mut session,
        )
        .await?;
    }

    let Some(inbox_name) = inbox else {
        // No inbox: nothing left to do this cycle; settle to Idle before
        // waiting for the next one.
        emit_state(app, &account.id, SyncState::Idle, None, false);
        tokio::time::sleep(POLL_INTERVAL).await;
        let _ = session.logout().await;
        return Ok(());
    };

    // No prefetch here, deliberately: the sweep leaves whichever folder came
    // last in the server's LIST response selected, not necessarily INBOX, and
    // prefetch issues UID FETCHes against the selected mailbox. The idle
    // loop's first iteration begins by selecting and re-syncing INBOX, and
    // prefetches from there, so the sweep's inbox candidates are picked up
    // one step later with the right mailbox selected.

    // Idle loop: re-sync INBOX on the live connection, then IDLE until the
    // server announces changes, the re-issue cadence elapses, or the
    // full-resync deadline passes (issue #41 — see `idle_wait_budget` /
    // `idle_timeout_action` below for which of the latter two actually
    // happened and what each means for the loop). The first re-sync runs
    // straight after the sweep and catches mail that arrived while other
    // folders were syncing — arrivals whose untagged EXISTS would otherwise be
    // swallowed by the pre-IDLE SELECT and sit unnoticed until the deadline.
    // Later iterations are IDLE wakeups (or re-issues): one STATUS+FETCH round
    // trip from wakeup to notification, no reconnect. Body prefetch (issue
    // #40) is deliberately *not* part of this per-iteration re-sync any more — see
    // the `prefetch_message_bodies` call after `IdleOutcome::NewData` below
    // for where it moved and why.
    let deadline = std::time::Instant::now() + FULL_RESYNC_INTERVAL;
    let mut consecutive_resyncs: u32 = 0;
    loop {
        // STATUS first (and write the counts) so the refresh events emitted by
        // sync_folder_uids find server-authoritative folder badges in the DB.
        let status = imap::status(&mut session, &inbox_name).await?;
        let inbox_id = {
            let conn = db.lock().expect("db poisoned");
            let (folder_id, _wiped) = store::upsert_folder(
                &conn,
                &account.id,
                &inbox_name,
                FolderRole::Inbox,
                status.uidvalidity,
            )?;
            store::set_folder_counts(&conn, folder_id, status.exists, status.unseen)?;
            folder_id
        };
        // Never `initial` here: the sweep above already established the
        // `last_seen_uid` baseline, so anything newer deserves a notification
        // even during the account's first connection.
        sync_folder_uids(
            app,
            db,
            account,
            inbox_id,
            &inbox_name,
            FolderRole::Inbox,
            false,
            status.exists,
            &mut session,
        )
        .await?;

        // Body prefetch (issue #40) runs *here*: after the re-sync above has
        // already emitted and notified, and before the drain-check below.
        //
        // Both halves of that placement are load-bearing. It must come after
        // the notify, because prefetch is up to five sequential size+body
        // fetches of up to 1 MB each — putting it ahead of the notify would
        // delay every new-mail toast by exactly the best-effort background
        // work this whole change set exists to keep out of the notification
        // path. And it must come before the drain-check rather than after,
        // because the drain-check has to be the last thing that happens
        // before `session.idle()` (issue #39). An EXISTS piggy-backed on one
        // of prefetch's own FETCH commands is therefore not lost: it lands in
        // `unsolicited_responses` and the drain-check immediately below sees
        // it and re-syncs instead of idling.
        //
        // That the window exists at all is the cost of a single connection;
        // the drain-check makes it *safe*, not absent. Eliminating it needs
        // prefetch on a second connection — see issue #42, deliberately not
        // done here.
        //
        // INBOX is guaranteed to be the selected mailbox at this point
        // because `sync_folder_uids` selected it just above. Do not move this
        // call somewhere that isn't true: prefetch issues UID FETCHes, UIDs
        // are mailbox-scoped, and a matching UID in the wrong mailbox would
        // store another folder's body/attachments/shipments against an INBOX
        // message.
        prefetch_message_bodies(db, inbox_id, &mut session).await;

        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remaining_until_deadline = deadline.saturating_duration_since(now);

        // Atomicity requirement (issue #39): the decision to IDLE must be made
        // from a check that cannot go stale before we actually call
        // `session.idle()`. `drain_mailbox_changed` is a local, synchronous
        // `try_recv` loop — no `.await`, no network I/O — so nothing can land
        // in the channel between this check and the `idle_wait` call below.
        // Anything piggy-backed on the STATUS/SELECT/FETCH calls just above
        // is already sitting in the channel by now (there is no prefetch call
        // in this window any more — see issue #40 below — so this is now a
        // short, cheap sequence rather than the long one prefetch used to
        // make it); this is what closes the race that commit 0f1354e left
        // open (it re-synced before every IDLE, same as here, but never
        // drained this channel, so a server response landing on any of those
        // commands was still silently dropped instead of triggering another
        // re-sync).
        let changed = drain_mailbox_changed(&session.unsolicited_responses);
        match drain_action(changed, consecutive_resyncs) {
            DrainAction::Resync => {
                consecutive_resyncs += 1;
                continue;
            }
            // The drain consumed an announcement the server will not repeat,
            // and the cap says stop re-syncing — so idling here would strand
            // it. Reconnect and sweep instead.
            DrainAction::EndCycle => break,
            DrainAction::Idle => consecutive_resyncs = 0,
        }

        // Work is done; settle to Idle before entering IDLE (or the polling
        // fallback inside idle_wait). Cap the wait at the shorter of the IDLE
        // re-issue cadence and whatever is left until the full-resync
        // deadline (issue #41: `idle_wait_budget`) — never the full remaining
        // time, or the connection could sit un-exercised for up to 25
        // minutes, exactly the blind window this fix exists to shrink.
        let wait_budget = idle_wait_budget(remaining_until_deadline, IDLE_REISSUE_INTERVAL);
        emit_state(app, &account.id, SyncState::Idle, None, false);
        match idle_wait(session, wait_budget).await? {
            IdleOutcome::NewData(live) => {
                session = live;
                emit_state(app, &account.id, SyncState::Syncing, None, false);
                // Deliberately no prefetch here: the wakeup means new mail is
                // waiting, so the only thing that should happen next is the
                // loop's re-sync and its notification. Prefetch runs after
                // that, at the top-of-loop call site above.
            }
            IdleOutcome::Timeout(live) => {
                session = live;
                // A `Timeout` here is ambiguous by itself: it fires both when
                // the (short) IDLE re-issue cadence elapses on a perfectly
                // healthy connection, and when `wait_budget` was capped by
                // the full-resync deadline instead. Only the latter should
                // tear the connection down; the former must loop back and
                // re-issue IDLE on the *same* connection, or splitting the
                // two intervals would just reconnect 5x more often than the
                // old 25-minute cadence instead of fixing anything. See
                // `idle_timeout_action` for the (tested) decision.
                match idle_timeout_action(remaining_until_deadline, wait_budget) {
                    IdleTimeoutAction::Reissue => continue,
                    IdleTimeoutAction::EndCycle => break,
                }
            }
            IdleOutcome::SessionGone => return Ok(()),
        }
    }

    let _ = session.logout().await;
    Ok(())
}

/// Fetch new UIDs for a folder, upsert them, emit events, and (for inbox,
/// non-initial) notify.
///
/// Deliberately does *not* prefetch bodies (issue #40): this is called for
/// every folder on every full sweep, but prefetch is inbox-only and its
/// timing relative to IDLE matters a great deal (see the `prefetch_message_bodies`
/// call sites in `run_once`), so it is the caller's job to invoke it at the
/// right moment rather than have it tag along here.
#[allow(clippy::too_many_arguments)]
async fn sync_folder_uids(
    app: &AppHandle,
    db: &Db,
    account: &Account,
    folder_id: i64,
    folder_name: &str,
    role: FolderRole,
    initial: bool,
    server_exists: u32,
    session: &mut imap::ImapSession,
) -> Result<()> {
    imap::select(session, folder_name).await?;

    // Determine the UID range to fetch.
    let (last_uid, last_seen_uid) = {
        let conn = db.lock().expect("db poisoned");
        let cached_max = store::max_uid(&conn, folder_id)?;
        let folder = store::get_folder(&conn, folder_id)?;
        let seen = folder.map(|f| f.last_seen_uid as u32).unwrap_or(0);
        (cached_max, seen)
    };

    let mut envelopes = if last_uid == 0 {
        imap::fetch_recent_envelopes(session, server_exists, INITIAL_SYNC_LIMIT).await?
    } else {
        let uid_set = format!("{}:*", last_uid + 1);
        imap::fetch_envelopes(session, &uid_set).await?
    };
    // Drop any envelope we already have (the `<last+1>:*` form can return the
    // last UID on some servers) and keep only genuinely new ones.
    envelopes.retain(|e| e.uid > last_uid);

    let mut new_summaries: Vec<crate::wire::MessageSummary> = Vec::new();
    let mut notify_list: Vec<NewMail> = Vec::new();
    let mut highest_uid = last_uid;

    {
        let conn = db.lock().expect("db poisoned");
        for env in &envelopes {
            // No body has been fetched yet at envelope-sync time, so there is
            // nothing to summarize; leave the snippet empty rather than
            // seeding it from the subject (which just duplicates the subject
            // line in the UI until the body is prefetched or opened).
            let snippet = String::new();
            let upsert = MessageUpsert {
                folder_id,
                uid: env.uid,
                message_id: env.message_id.clone(),
                subject: env.subject.clone(),
                from_name: env.from_name.clone(),
                from_addr: env.from_addr.clone(),
                to_addrs: env.to_addrs.clone(),
                cc_addrs: env.cc_addrs.clone(),
                date: env.date.clone(),
                snippet,
                seen: env.seen,
                flagged: env.flagged,
                has_attachments: env.has_attachments,
                rfc822_size: env.rfc822_size,
            };
            let id = store::upsert_message(&conn, &upsert)?;
            highest_uid = highest_uid.max(env.uid);

            let summary = crate::wire::MessageSummary {
                id,
                account_id: account.id.clone(),
                folder_id,
                uid: env.uid,
                subject: env.subject.clone(),
                from_name: env.from_name.clone(),
                from_addr: env.from_addr.clone(),
                date: env.date.clone(),
                snippet: String::new(),
                seen: env.seen,
                flagged: env.flagged,
                has_attachments: env.has_attachments,
            };
            new_summaries.push(summary);

            if !initial && role == FolderRole::Inbox && env.uid > last_seen_uid && !env.seen {
                notify_list.push(NewMail {
                    from_name: env.from_name.clone(),
                    subject: env.subject.clone(),
                });
            }
        }
        if role == FolderRole::Inbox {
            store::set_last_seen_uid(&conn, folder_id, highest_uid)?;
        }
    }

    if !envelopes.is_empty() {
        // Emit and notify immediately; body prefetch (now handled by the
        // caller, never here — see the doc comment above) must never sit
        // between new mail landing in the DB and this notification.
        let _ = app.emit(
            "mail:new-messages",
            serde_json::json!({
                "accountId": account.id,
                "folderId": folder_id,
                "messages": new_summaries,
            }),
        );
        let _ = app.emit(
            "mail:messages-updated",
            MessagesUpdatedPayload { folder_id },
        );

        if !notify_list.is_empty() {
            notifications::notify_new_mail(app, &account.email, &notify_list);
        }
    }

    Ok(())
}

async fn prefetch_message_bodies(db: &Db, folder_id: i64, session: &mut imap::ImapSession) {
    let candidates = {
        let conn = db.lock().expect("db poisoned");
        store::body_prefetch_candidates(
            &conn,
            folder_id,
            BODY_PREFETCH_WORKING_SET,
            BODY_PREFETCH_LIMIT,
            BODY_PREFETCH_MAX_BYTES,
        )
    };
    let candidates = match candidates {
        Ok(candidates) => candidates,
        Err(err) => {
            tracing::warn!(folder_id, error = %err, "could not select body prefetch candidates");
            return;
        }
    };

    for candidate in candidates {
        let size = if candidate.rfc822_size == 0 {
            match imap::fetch_message_size(session, candidate.uid).await {
                Ok(size) => {
                    let conn = db.lock().expect("db poisoned");
                    if let Err(err) = store::set_message_size(&conn, candidate.id, size) {
                        tracing::warn!(message_id = candidate.id, error = %err, "could not cache message size");
                    }
                    size
                }
                Err(err) => {
                    tracing::warn!(message_id = candidate.id, error = %err, "body prefetch size lookup failed");
                    continue;
                }
            }
        } else {
            candidate.rfc822_size
        };
        if size > BODY_PREFETCH_MAX_BYTES {
            continue;
        }

        let body = match imap::fetch_body(session, candidate.uid).await {
            Ok(body) => body,
            Err(err) => {
                tracing::warn!(message_id = candidate.id, error = %err, "body prefetch failed");
                continue;
            }
        };
        let snippet = imap::snippet_for_body(body.text.as_deref(), body.html.as_deref());
        let conn = db.lock().expect("db poisoned");
        if let Err(err) = store::set_body(
            &conn,
            candidate.id,
            body.text.as_deref(),
            body.html.as_deref(),
            &body.to_addrs,
            &body.cc_addrs,
            snippet.as_deref(),
        ) {
            tracing::warn!(message_id = candidate.id, error = %err, "could not cache prefetched body");
            continue;
        }
        if let Err(err) = store::replace_attachments(&conn, candidate.id, &body.attachments) {
            tracing::warn!(message_id = candidate.id, error = %err, "could not cache prefetched attachments");
        }

        let detected = shipments::extract_shipments(body.text.as_deref(), body.html.as_deref());
        let inserts: Vec<store::ShipmentInsert> =
            detected.into_iter().map(shipment_insert).collect();
        if let Err(err) = store::replace_shipments(&conn, candidate.id, &inserts) {
            tracing::warn!(message_id = candidate.id, error = %err, "could not cache prefetched shipments");
        }
    }
}

/// Whether an [`UnsolicitedResponse`] means the mailbox changed and a
/// re-sync is owed before the next IDLE, as opposed to a kind we can safely
/// ignore for this purpose (e.g. `Status` for a *different* mailbox pushed by
/// a server extension, or `Expunge`, which shrinks the mailbox and is already
/// handled by the ordinary UID-based re-sync catching up next time around).
///
/// `Exists` and `Recent` are exactly the RFC 3501 §7.3.1/§7.3.2 signals a
/// server uses to announce new mail arriving in the selected mailbox — the
/// two kinds this whole fix exists to stop dropping.
fn mailbox_changed(resp: &UnsolicitedResponse) -> bool {
    matches!(
        resp,
        UnsolicitedResponse::Exists(_) | UnsolicitedResponse::Recent(_)
    )
}

/// Drain every response currently queued on an `async-imap` session's
/// `unsolicited_responses` channel and report whether any of them means the
/// mailbox changed (see [`mailbox_changed`]).
///
/// Must loop on `try_recv`, not `recv().await`: the channel is filled by
/// `try_send().ok()` on the other end and is never closed by the server, so
/// awaiting would block forever once it runs dry rather than returning "no
/// more right now". Draining to completion (not stopping at the first hit)
/// is deliberate too, independent of the return value: the channel is
/// `bounded(100)` and filled with `try_send().ok()`, so anything left behind
/// counts against that bound and brings the channel closer to silently
/// dropping *future* announcements once it fills up — see the module docs
/// for why a dropped announcement is unrecoverable (the server never repeats
/// it).
fn drain_mailbox_changed(unsolicited: &async_channel::Receiver<UnsolicitedResponse>) -> bool {
    let mut changed = false;
    // `Err` here is Empty (nothing queued right now) or Closed (session gone)
    // — either way there is nothing left to read this pass, so the loop ends.
    while let Ok(resp) = unsolicited.try_recv() {
        changed |= mailbox_changed(&resp);
    }
    changed
}

/// What the idle loop should do once the drain-check has run.
#[derive(Debug, PartialEq, Eq)]
enum DrainAction {
    /// Nothing pending; enter IDLE.
    Idle,
    /// The mailbox changed; re-sync INBOX again before idling.
    Resync,
    /// The mailbox changed but the re-sync cap is exhausted; end the cycle so
    /// the caller reconnects and sweeps rather than idling on a known-stale
    /// view.
    EndCycle,
}

/// Decide what to do after a drain-check, given whether it found the mailbox
/// changed and how many times in a row this cycle has already re-synced
/// without ever actually idling.
///
/// This is the pure core of the atomicity requirement described in the module
/// docs: as long as a `changed` drain is never followed by an IDLE call, no
/// server response is left unread while we sleep.
///
/// The `consecutive_resyncs` bound is the pathological-loop guard, but note
/// what it must *not* do: once the drain has returned `true`, the announcement
/// it saw has already been taken out of the channel and the server will never
/// repeat it. Idling at that point would strand the very message the drain
/// just told us about until the full-resync deadline — the exact bug this
/// whole change set exists to fix. So the cap ends the cycle instead, which
/// reconnects and sweeps every folder and is therefore guaranteed to see it.
/// Only a clean drain may enter IDLE.
fn drain_action(mailbox_changed_since_last_check: bool, consecutive_resyncs: u32) -> DrainAction {
    if !mailbox_changed_since_last_check {
        return DrainAction::Idle;
    }
    if consecutive_resyncs < MAX_CONSECUTIVE_RESYNCS_BEFORE_IDLE {
        DrainAction::Resync
    } else {
        DrainAction::EndCycle
    }
}

/// Duration to pass as the next `idle_wait` timeout (issue #41): whichever is
/// shorter, the IDLE re-issue cadence or however long is actually left until
/// the full-resync deadline. Capping at the deadline (rather than always
/// waiting a full `idle_reissue_interval`) is what guarantees the idle loop
/// never overruns `FULL_RESYNC_INTERVAL` — without it, a wait that started
/// with e.g. 4 minutes left on the deadline but was handed the full 5-minute
/// cadence would blow past the deadline before `idle_wait` ever returns.
fn idle_wait_budget(
    remaining_until_deadline: Duration,
    idle_reissue_interval: Duration,
) -> Duration {
    remaining_until_deadline.min(idle_reissue_interval)
}

/// What the idle loop should do after an [`IdleOutcome::Timeout`]: loop back
/// and re-issue IDLE on the same connection, or end the cycle so the caller
/// reconnects and does a fresh full sweep.
///
/// This is the crux of issue #41's second half. Before this fix,
/// `IdleOutcome::Timeout` meant exactly one thing — the 25-minute
/// `FULL_RESYNC_INTERVAL` deadline had been reached — because that was the
/// only timeout `idle_wait` was ever given. Splitting the IDLE re-issue
/// cadence out from the full-resync deadline means `idle_wait` can now also
/// time out simply because the (much shorter) re-issue interval elapsed on a
/// perfectly healthy connection, and that case must *not* tear the
/// connection down — doing so would reconnect roughly 5x more often than
/// before and make the exact problem #41 reports (dead time invisible to the
/// user) *worse*, not better, by spending more of it reconnecting instead of
/// idling.
#[derive(Debug, PartialEq, Eq)]
enum IdleTimeoutAction {
    /// The re-issue cadence elapsed but the full-resync deadline has not been
    /// reached; loop back, re-sync INBOX, drain-check, and re-enter IDLE on
    /// the same connection.
    Reissue,
    /// The wait was capped by the full-resync deadline rather than the
    /// re-issue cadence, and that deadline is now reached; end the cycle so
    /// the caller reconnects and re-sweeps every folder.
    EndCycle,
}

/// Decide [`IdleTimeoutAction`] from the same two durations that produced the
/// `wait_budget` handed to `idle_wait` ([`idle_wait_budget`]). Because
/// `idle_wait` is given exactly `wait_budget` as its timeout, a `Timeout`
/// outcome after that wait corresponds deterministically to whichever value
/// `idle_wait_budget` picked: if the budget was *not* capped by the deadline
/// (`wait_budget < remaining_until_deadline`), the timeout must have been the
/// re-issue cadence firing early, so there is time left and the loop should
/// re-issue. Otherwise — including the exact boundary where the re-issue
/// interval would overrun the deadline, i.e. `remaining_until_deadline <=
/// idle_reissue_interval` — the budget was clamped to the deadline, so timing
/// out means the deadline itself has (just) arrived.
fn idle_timeout_action(
    remaining_until_deadline: Duration,
    wait_budget: Duration,
) -> IdleTimeoutAction {
    if wait_budget >= remaining_until_deadline {
        IdleTimeoutAction::EndCycle
    } else {
        IdleTimeoutAction::Reissue
    }
}

/// Outcome of one IDLE wait on the selected inbox.
enum IdleOutcome {
    /// The server announced changes; the recovered session is live for an
    /// immediate re-sync on the same connection.
    NewData(imap::ImapSession),
    /// The wait timed out — either the IDLE re-issue cadence elapsed on a
    /// healthy connection, the full-resync deadline was reached, or an
    /// IDLE-less server's poll-fallback sleep elapsed. The recovered session
    /// is live either way; the caller decides which of those it was via
    /// [`idle_timeout_action`] and either loops back onto the same connection
    /// or ends the cycle accordingly.
    Timeout(imap::ImapSession),
    /// The wakeup was routine but the connection died while closing IDLE; the
    /// caller should end the cycle and reconnect.
    SessionGone,
}

/// Enter IDLE on the currently-selected mailbox and wait for a server wakeup
/// or `timeout`, recovering the session so the caller can keep using the
/// connection.
///
/// If the server does not support IDLE, `init()` fails and we fall back to a
/// single poll sleep, per the contract.
async fn idle_wait(session: imap::ImapSession, timeout: Duration) -> Result<IdleOutcome> {
    // Cloning the receiver half of an `async_channel` clones a handle onto the
    // same underlying queue, not a snapshot — this still observes every push
    // `Handle` makes into the same channel once `session.idle()` below
    // consumes `session` and we lose direct field access to it.
    let unsolicited = session.unsolicited_responses.clone();
    let mut handle = session.idle();

    if let Err(err) = handle.init().await {
        tracing::info!(error = %err, "IDLE unavailable; falling back to polling");
        // IDLE was never actually established here, so a failure to close it
        // reflects a genuinely broken connection, not routine IDLE cleanup;
        // keep surfacing it as a real failure.
        let mut session = handle
            .done()
            .await
            .context("closing IDLE handle after failed init")?;
        tokio::time::sleep(POLL_INTERVAL.min(timeout)).await;
        let _ = session.noop().await;
        return Ok(IdleOutcome::Timeout(session));
    }

    // `init()`'s own response-reading loop runs until the server's "+ idling"
    // continuation, and anything it sees before that — e.g. an EXISTS
    // piggy-backed on the IDLE command's own response — is routed to
    // `unsolicited_responses` rather than surfaced to us (see
    // `extensions::idle::Handle::init` in the vendored async-imap source: it
    // calls `handle_unilateral` for everything except the continuation and
    // its own tagged completion). That is the "handle.init() can swallow an
    // announcement too" case from issue #39. Drain now, before committing to
    // an actual wait, so it is not lost: if something is already there, end
    // IDLE immediately and report it exactly like a real wakeup rather than
    // waiting out `timeout` on a mailbox we already know has moved on.
    if drain_mailbox_changed(&unsolicited) {
        // Closing here is subject to exactly the same routine-disconnect
        // caveat as the post-wait `done()` below, so it gets the same
        // treatment: the drain already told us the mailbox moved on, so a
        // connection that dies while we send DONE is not new information.
        // Reporting it as a sync failure would be actively harmful here —
        // `account_loop` would emit an error state and sleep out the 30s
        // backoff before reconnecting, delaying the very message this branch
        // exists to surface. `SessionGone` instead ends the cycle cleanly so
        // the caller reconnects immediately and re-syncs.
        return match handle.done().await {
            Ok(session) => Ok(IdleOutcome::NewData(session)),
            Err(err) if is_benign_after_routine_wakeup(&err) => {
                tracing::debug!(
                    error = %err,
                    "IDLE session close failed after pre-wait drain; reconnecting next cycle"
                );
                Ok(IdleOutcome::SessionGone)
            }
            Err(err) => Err(err).context("closing IDLE handle after pre-wait drain"),
        };
    }

    let (fut, _stop) = handle.wait_with_timeout(timeout);
    // async-imap's timeout is an *inactivity* timeout — any server response,
    // including "* OK Still here" keepalives, resets it — so `timeout` is also
    // enforced as a hard wall-clock bound here. Otherwise a server with chatty
    // keepalives (Dovecot pings every ~2 min) would postpone the full-resync
    // deadline indefinitely. NewData / Timeout / ManualInterrupt are all
    // routine wakeups; only an `Err` — a genuine IO/protocol failure while
    // idling — is worth surfacing as a sync failure.
    let response = match tokio::time::timeout(timeout, fut).await {
        Ok(woke) => woke.context("IDLE wait failed")?,
        Err(_elapsed) => IdleResponse::Timeout,
    };

    // Send DONE to end IDLE cleanly and recover the session. The wakeup itself
    // already succeeded, and every cycle reconnects fresh regardless of how it
    // ends (see module docs), so a failure here — e.g. the server or an
    // intervening NAT/firewall silently dropped the connection during a long,
    // uneventful IDLE — must not be reported as a sync failure. A genuinely
    // dead connection is still caught for real by the next cycle's
    // `imap::connect`.
    match handle.done().await {
        Ok(session) => {
            // A `NewData` wakeup already means "the mailbox changed" via the
            // one response that ended `wait_with_timeout` directly (see the
            // module docs: that response bypasses the channel entirely). This
            // drain is purely hygiene for whatever *else* may have landed in
            // the channel during the wait or DONE's own response read (e.g.
            // via the `Done`-response arm inside `wait_with_timeout`, which
            // does route through `handle_unilateral`) — left undrained it
            // just brings the bounded(100) channel closer to silently
            // dropping a future announcement. The outcome below is decided by
            // `response`, not by this drain, either way.
            drain_mailbox_changed(&unsolicited);
            Ok(match response {
                IdleResponse::NewData(_) => IdleOutcome::NewData(session),
                IdleResponse::Timeout | IdleResponse::ManualInterrupt => {
                    IdleOutcome::Timeout(session)
                }
            })
        }
        Err(err) if is_benign_after_routine_wakeup(&err) => {
            tracing::debug!(
                error = %err,
                "IDLE session close failed after routine wakeup; reconnecting next cycle"
            );
            Ok(IdleOutcome::SessionGone)
        }
        Err(err) => Err(err).context("closing IDLE handle"),
    }
}

/// Whether a failure to close an IDLE handle, occurring right after a
/// routine wakeup (Timeout / NewData / ManualInterrupt already succeeded),
/// should be swallowed rather than reported as a sync failure.
///
/// Every IDLE cycle reconnects fresh regardless of how it ends (see module
/// docs), so once the wakeup itself is known-good, a `DONE` that fails
/// because the connection or stream is gone is not new information — it is
/// the routine downside of holding a socket idle for up to ~25 minutes
/// (server timeout, NAT/firewall drop, etc.). A genuinely dead connection is
/// still caught for real by the next cycle's `imap::connect`. The one case
/// kept fatal is a malformed/unexpected server response to `DONE` itself
/// (`Bad`/`Parse`), since that points at a protocol problem rather than a
/// vanished connection.
fn is_benign_after_routine_wakeup(err: &async_imap::error::Error) -> bool {
    use async_imap::error::Error;
    matches!(err, Error::Io(_) | Error::ConnectionLost | Error::No(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_state_as_str_matches_wire_contract() {
        // The frontend matches on these exact lowercase strings
        // (src/lib/types.ts SyncState) — this is the binding wire contract.
        assert_eq!(SyncState::Idle.as_str(), "idle");
        assert_eq!(SyncState::Syncing.as_str(), "syncing");
        assert_eq!(SyncState::Error.as_str(), "error");
    }

    #[test]
    fn sync_state_payload_serializes_camel_case() {
        let payload = SyncStatePayload {
            account_id: "acct-1",
            state: SyncState::Syncing.as_str(),
            error: None,
            needs_reauth: false,
        };
        let json = serde_json::to_value(&payload).expect("serialize payload");
        assert_eq!(
            json,
            serde_json::json!({
                "accountId": "acct-1",
                "state": "syncing",
                "error": null,
                "needsReauth": false,
            })
        );
    }

    #[test]
    fn sync_state_payload_carries_error_message() {
        let payload = SyncStatePayload {
            account_id: "acct-1",
            state: SyncState::Error.as_str(),
            error: Some("connection refused".to_string()),
            needs_reauth: false,
        };
        let json = serde_json::to_value(&payload).expect("serialize payload");
        assert_eq!(json["state"], "error");
        assert_eq!(json["error"], "connection refused");
        assert_eq!(json["needsReauth"], false);
    }

    #[test]
    fn auth_expired_marker_is_detectable_through_context_chain() {
        // The marker is attached as *context* mid-chain (see oauth::access_token)
        // and the sync loop downcasts through however many contexts the connect
        // path stacked on top; it must be found regardless of position.
        let err = anyhow::anyhow!("Server returned error response: invalid_grant")
            .context("refreshing access token")
            .context(crate::auth::oauth::AuthExpired)
            .context("obtaining Gmail access token")
            .context("connecting IMAP");
        assert!(crate::auth::oauth::is_auth_expired(&err));

        let plain = anyhow::anyhow!("connection refused").context("connecting IMAP");
        assert!(!crate::auth::oauth::is_auth_expired(&plain));
    }

    #[test]
    fn idle_close_failure_is_benign_for_io_and_connection_loss() {
        // The exact "closing IDLE handle" papercut: after a routine ~25-min
        // IDLE re-issue, the server/NAT/firewall has already dropped the
        // connection, so DONE fails with an IO error or ConnectionLost. Both
        // must be swallowed rather than reported as a sync failure.
        let io_err = async_imap::error::Error::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "broken pipe",
        ));
        assert!(is_benign_after_routine_wakeup(&io_err));
        assert!(is_benign_after_routine_wakeup(
            &async_imap::error::Error::ConnectionLost
        ));
        assert!(is_benign_after_routine_wakeup(
            &async_imap::error::Error::No("server said no".to_string())
        ));
    }

    #[test]
    fn idle_close_failure_stays_fatal_for_protocol_errors() {
        // A BAD or unparsable response to our own DONE points at a genuine
        // protocol problem, not a vanished connection; keep it surfacing.
        let bad = async_imap::error::Error::Bad("DONE not understood".to_string());
        assert!(!is_benign_after_routine_wakeup(&bad));

        let parse = async_imap::error::Error::Parse(async_imap::error::ParseError::Invalid(
            b"garbage".to_vec(),
        ));
        assert!(!is_benign_after_routine_wakeup(&parse));
    }

    // -- issue #39: drain unsolicited EXISTS/RECENT before every IDLE --

    #[test]
    fn mailbox_changed_flags_exists_and_recent() {
        // These are the two RFC 3501 signals a server uses to announce new
        // mail in the selected mailbox — exactly what this fix must stop
        // dropping.
        assert!(mailbox_changed(&UnsolicitedResponse::Exists(42)));
        assert!(mailbox_changed(&UnsolicitedResponse::Recent(3)));
    }

    #[test]
    fn mailbox_changed_ignores_other_response_kinds() {
        // Expunge shrinks the mailbox (handled by the ordinary UID re-sync
        // catching up), and Status is scoped to a mailbox name, not
        // necessarily the selected one — neither means "re-sync INBOX now".
        assert!(!mailbox_changed(&UnsolicitedResponse::Expunge(1)));
        assert!(!mailbox_changed(&UnsolicitedResponse::Status {
            mailbox: "Other".to_string(),
            attributes: Vec::new(),
        }));
    }

    #[test]
    fn drain_mailbox_changed_reports_true_when_exists_queued() {
        let (tx, rx) = async_channel::bounded(8);
        tx.try_send(UnsolicitedResponse::Exists(5)).unwrap();
        assert!(drain_mailbox_changed(&rx));
    }

    #[test]
    fn drain_mailbox_changed_reports_false_on_empty_channel() {
        let (_tx, rx) = async_channel::bounded::<UnsolicitedResponse>(8);
        assert!(!drain_mailbox_changed(&rx));
        // Calling again on an already-empty channel must not block or panic
        // (this is exactly why `try_recv`, not `recv().await`, is required —
        // the channel is never closed by the server side).
        assert!(!drain_mailbox_changed(&rx));
    }

    #[test]
    fn drain_mailbox_changed_ignores_non_mailbox_kinds() {
        let (tx, rx) = async_channel::bounded(8);
        tx.try_send(UnsolicitedResponse::Expunge(1)).unwrap();
        tx.try_send(UnsolicitedResponse::Status {
            mailbox: "Other".to_string(),
            attributes: Vec::new(),
        })
        .unwrap();
        assert!(!drain_mailbox_changed(&rx));
    }

    #[test]
    fn drain_mailbox_changed_drains_fully_not_just_first_hit() {
        // A partial drain would leave items counting against the bounded(100)
        // channel, eventually causing it to silently start dropping future
        // announcements. One call must empty it completely regardless of how
        // many items (or what kind) are queued.
        let (tx, rx) = async_channel::bounded(8);
        tx.try_send(UnsolicitedResponse::Expunge(1)).unwrap();
        tx.try_send(UnsolicitedResponse::Exists(9)).unwrap();
        tx.try_send(UnsolicitedResponse::Recent(2)).unwrap();

        assert!(drain_mailbox_changed(&rx));
        // Nothing left: a follow-up drain finds the channel empty.
        assert!(!drain_mailbox_changed(&rx));
    }

    #[test]
    fn drain_action_idles_only_on_a_clean_drain() {
        assert_eq!(drain_action(false, 0), DrainAction::Idle);
        assert_eq!(
            drain_action(false, MAX_CONSECUTIVE_RESYNCS_BEFORE_IDLE + 10),
            DrainAction::Idle
        );
    }

    #[test]
    fn drain_action_resyncs_while_under_the_bound() {
        assert_eq!(drain_action(true, 0), DrainAction::Resync);
        assert_eq!(
            drain_action(true, MAX_CONSECUTIVE_RESYNCS_BEFORE_IDLE - 1),
            DrainAction::Resync
        );
    }

    #[test]
    fn drain_action_ends_the_cycle_rather_than_idling_on_a_stranded_announcement() {
        // At the cap the drain has already consumed an announcement the server
        // will never repeat. Idling would strand that message until the
        // full-resync deadline — precisely the bug being fixed — so the cap
        // must reconnect-and-sweep, never idle.
        assert_eq!(
            drain_action(true, MAX_CONSECUTIVE_RESYNCS_BEFORE_IDLE),
            DrainAction::EndCycle
        );
        assert_eq!(
            drain_action(true, MAX_CONSECUTIVE_RESYNCS_BEFORE_IDLE + 10),
            DrainAction::EndCycle
        );
    }

    #[test]
    fn idle_wait_budget_prefers_the_reissue_interval_when_deadline_is_far_off() {
        // Plenty of time left until the full-resync deadline: the wait should
        // be capped at the (shorter) re-issue cadence, not the whole
        // remaining budget.
        let remaining = Duration::from_secs(20 * 60);
        let reissue = Duration::from_secs(5 * 60);
        assert_eq!(idle_wait_budget(remaining, reissue), reissue);
    }

    #[test]
    fn idle_wait_budget_clamps_to_the_deadline_when_it_is_the_closer_bound() {
        // Less time left until the deadline than a full re-issue interval: the
        // wait must be clamped so it cannot overrun the deadline.
        let remaining = Duration::from_secs(90);
        let reissue = Duration::from_secs(5 * 60);
        assert_eq!(idle_wait_budget(remaining, reissue), remaining);
    }

    #[test]
    fn idle_wait_budget_boundary_when_remaining_exactly_equals_the_reissue_interval() {
        let interval = Duration::from_secs(5 * 60);
        assert_eq!(idle_wait_budget(interval, interval), interval);
    }

    #[test]
    fn idle_timeout_action_reissues_when_the_budget_was_not_clamped_by_the_deadline() {
        // wait_budget < remaining_until_deadline means idle_wait_budget picked
        // the re-issue interval, not the deadline, so the connection is fine
        // and there is time left: loop back onto the same connection.
        let remaining = Duration::from_secs(20 * 60);
        let budget = Duration::from_secs(5 * 60);
        assert_eq!(
            idle_timeout_action(remaining, budget),
            IdleTimeoutAction::Reissue
        );
    }

    #[test]
    fn idle_timeout_action_ends_the_cycle_when_the_budget_was_clamped_by_the_deadline() {
        let remaining = Duration::from_secs(90);
        let budget = Duration::from_secs(90);
        assert_eq!(
            idle_timeout_action(remaining, budget),
            IdleTimeoutAction::EndCycle
        );
    }

    #[test]
    fn idle_timeout_action_boundary_where_the_reissue_interval_would_overrun_the_deadline() {
        // Exactly the boundary called out in issue #41: remaining time equals
        // the re-issue interval, so idle_wait_budget clamps (arbitrarily,
        // since both sides agree) to that shared value, and the timeout must
        // be treated as the deadline arriving, not a routine re-issue — an
        // extra iteration here would (marginally) overrun the full-resync
        // deadline.
        let interval = Duration::from_secs(5 * 60);
        let budget = idle_wait_budget(interval, interval);
        assert_eq!(
            idle_timeout_action(interval, budget),
            IdleTimeoutAction::EndCycle
        );
    }
}
