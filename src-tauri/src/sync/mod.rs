//! Sync engine: one background task per account.
//!
//! Each task connects, lists folders, performs an initial sync (newest 200
//! envelopes per folder), then enters an INBOX IDLE loop that re-syncs on
//! wakeup or every 25 minutes. Bodies are fetched lazily by commands. Errors
//! trigger exponential backoff (30s → 5 min). Notifications fire only for new
//! inbox messages above the folder's `last_seen_uid`, and never during the
//! initial sync.

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
fn emit_state(app: &AppHandle, account_id: &str, state: SyncState, error: Option<String>) {
    let _ = app.emit(
        "mail:sync-state",
        SyncStatePayload {
            account_id,
            state: state.as_str(),
            error,
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
        emit_state(&app, &account.id, SyncState::Syncing, None);
        match run_once(&app, &db, &account, first_run).await {
            Ok(()) => {
                backoff = BACKOFF_MIN;
                first_run = false;
                emit_state(&app, &account.id, SyncState::Idle, None);
                // run_once returns after an IDLE/poll cycle; loop again.
            }
            Err(err) => {
                tracing::warn!(error = %err, "sync cycle failed");
                emit_state(&app, &account.id, SyncState::Error, Some(err.to_string()));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

/// One connect → sync → IDLE cycle. Returns after an IDLE wakeup/timeout so the
/// caller re-runs (reconnecting fresh each cycle for robustness).
async fn run_once(app: &AppHandle, db: &Db, account: &Account, initial: bool) -> Result<()> {
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
            // No inbox: just wait before the next full cycle.
            tokio::time::sleep(POLL_INTERVAL).await;
            let _ = session.logout().await;
            return Ok(());
        }
    };

    imap::select(&mut session, &inbox).await?;
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

    if envelopes.is_empty() {
        return Ok(());
    }

    let mut new_summaries: Vec<crate::wire::MessageSummary> = Vec::new();
    let mut notify_list: Vec<NewMail> = Vec::new();
    let mut highest_uid = last_uid;

    {
        let conn = db.lock().expect("db poisoned");
        for env in &envelopes {
            let snippet = imap::snippet_from_text(&env.subject);
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
                snippet: imap::snippet_from_text(&env.subject),
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

    // Emit new-messages event.
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

    // Notify (batching handled inside).
    if !notify_list.is_empty() {
        notifications::notify_new_mail(app, &account.email, &notify_list);
    }

    Ok(())
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
        // Recover the session and NOOP-poll.
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
    // We treat any of NewData / Timeout / ManualInterrupt as "re-sync now".
    let _ = fut.await;

    let mut session = handle.done().await.context("closing IDLE handle")?;
    let _ = session.logout().await;
    Ok(())
}
