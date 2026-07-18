//! Sync engine: one background task per account.
//!
//! Each task connects, lists folders, performs an initial sync (newest 200
//! envelopes per folder), then enters an INBOX IDLE loop that re-syncs on
//! wakeup or every 25 minutes. A small inbox working set is prefetched without
//! changing `\Seen`; other bodies remain lazy. Errors trigger exponential
//! backoff (30s → 5 min). Notifications fire only for new inbox messages above
//! the folder's `last_seen_uid`, and never during the initial sync.

pub mod imap;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::accounts::Account;
use crate::notifications::{self, NewMail};
use crate::state::Db;
use crate::store::{self, FolderRole, MessageUpsert};

const INITIAL_SYNC_LIMIT: u32 = 200;
const BODY_PREFETCH_WORKING_SET: u32 = 20;
const BODY_PREFETCH_LIMIT: u32 = 5;
const BODY_PREFETCH_MAX_BYTES: u32 = 1024 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(25 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(60);
const BACKOFF_MIN: Duration = Duration::from_secs(30);
const BACKOFF_MAX: Duration = Duration::from_secs(5 * 60);

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

/// One connect → sync → IDLE cycle. Returns after an IDLE wakeup/timeout so the
/// caller re-runs (reconnecting fresh each cycle for robustness).
async fn run_once(app: &AppHandle, db: &Db, account: &Account, initial: bool) -> Result<()> {
    // Real work is about to start: connect, discover folders, fetch/upsert
    // envelopes, prefetch bodies. Report Syncing for exactly this span, not for
    // the IDLE wait that follows (see Idle emissions below).
    emit_state(app, &account.id, SyncState::Syncing, None, false);

    let mut session = imap::connect(account).await?;

    // Discover folders and reconcile with the DB.
    let remote = imap::list_folders_with_roles(&mut session).await?;
    let mut inbox_name: Option<String> = None;
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
            inbox_name = Some(folder.name.clone());
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

    // IDLE on INBOX (fallback to polling if unavailable).
    let inbox = match inbox_name {
        Some(name) => name,
        None => {
            // No inbox: nothing left to do this cycle; settle to Idle before
            // waiting for the next one.
            emit_state(app, &account.id, SyncState::Idle, None, false);
            tokio::time::sleep(POLL_INTERVAL).await;
            let _ = session.logout().await;
            return Ok(());
        }
    };

    imap::select(&mut session, &inbox).await?;
    // Work is done; settle to Idle before entering IDLE (or the polling
    // fallback inside idle_wait), which can last up to 25 minutes.
    emit_state(app, &account.id, SyncState::Idle, None, false);
    idle_wait(session).await?;
    Ok(())
}

/// Fetch new UIDs for a folder, upsert them, emit events, and (for inbox,
/// non-initial) notify.
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
        // Emit and notify before prefetch so body downloads do not delay new-mail delivery.
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

    if role == FolderRole::Inbox {
        prefetch_message_bodies(db, folder_id, session).await;
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
    }
}

/// Enter IDLE on the currently-selected mailbox and wait for a server wakeup or
/// the 25-minute timeout, then return so the caller reconnects and re-syncs.
///
/// If the server does not support IDLE, `init()` fails and we fall back to a
/// single poll sleep, per the contract.
async fn idle_wait(session: imap::ImapSession) -> Result<()> {
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
        tokio::time::sleep(POLL_INTERVAL).await;
        let _ = session.noop().await;
        let _ = session.logout().await;
        return Ok(());
    }

    let (fut, _stop) = handle.wait_with_timeout(IDLE_TIMEOUT);
    // NewData / Timeout / ManualInterrupt are all routine wakeups (the
    // ~25-minute timeout is a *deliberate* re-issue per RFC 2177, not a
    // failure). Only an `Err` here — a genuine IO/protocol failure while
    // idling — is worth surfacing as a sync failure.
    fut.await.context("IDLE wait failed")?;

    // Best-effort: send DONE to end IDLE cleanly. The wakeup itself already
    // succeeded, and every cycle reconnects fresh regardless of how this
    // cycle ends (see module docs), so a failure here — e.g. the server or an
    // intervening NAT/firewall silently dropped the connection during a
    // long, uneventful IDLE, which routinely coincides with the ~25-minute
    // re-issue interval — must not be reported as a sync failure. A
    // genuinely dead connection is still caught for real by the next cycle's
    // `imap::connect`.
    match handle.done().await {
        Ok(mut session) => {
            let _ = session.logout().await;
        }
        Err(err) if is_benign_after_routine_wakeup(&err) => {
            tracing::debug!(
                error = %err,
                "IDLE session close failed after routine wakeup; reconnecting next cycle"
            );
        }
        Err(err) => return Err(err).context("closing IDLE handle"),
    }
    Ok(())
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
}
