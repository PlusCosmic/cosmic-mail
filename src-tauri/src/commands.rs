//! Tauri command handlers.
//!
//! These are intentionally thin: they validate inputs, delegate to the
//! `store`/`sync`/`auth` modules, and translate errors to [`AppError`] at the
//! boundary so the frontend sees plain strings.

use tauri::{AppHandle, Manager, State};

use crate::accounts::{self, Account, AccountKind};
use crate::error::{AppError, AppResult};
use crate::omarchy::{self, OmarchyTheme};
use crate::settings::{self, Settings};
use crate::shipments;
use crate::state::AppState;
use crate::store;
use crate::sync::imap as sync_imap;
use crate::wire::{
    AttachmentInfo, Folder, ImapAccountInput, MessageBody, MessageSummary, SendMessageInput,
    Shipment,
};

/// Read the active omarchy theme.
#[tauri::command]
pub fn get_theme() -> OmarchyTheme {
    omarchy::read_theme()
}

/// List all configured accounts (public projection).
#[tauri::command]
pub fn list_accounts() -> AppResult<Vec<accounts::AccountPublic>> {
    let accounts = accounts::load_accounts().map_err(AppError::from)?;
    Ok(accounts.iter().map(Account::public).collect())
}

/// Add a plain IMAP account, validating credentials by connecting first.
#[tauri::command]
pub async fn add_imap_account(
    app: AppHandle,
    input: ImapAccountInput,
) -> AppResult<accounts::AccountPublic> {
    let id = uuid::Uuid::new_v4().to_string();
    let account = Account {
        id: id.clone(),
        email: input.email.clone(),
        display_name: input.display_name.clone(),
        kind: AccountKind::Imap,
        imap_host: input.imap_host.clone(),
        imap_port: input.imap_port,
        smtp_host: input.smtp_host.clone(),
        smtp_port: input.smtp_port,
        username: input.username.clone(),
    };

    // Store the password in the keyring, then validate by connecting.
    accounts::set_imap_password(&id, &input.password).map_err(AppError::from)?;

    if let Err(err) = sync_imap::connect(&account).await {
        // Roll back the secret we just stored.
        accounts::delete_secrets(&id, AccountKind::Imap);
        return Err(AppError::msg(format!(
            "Could not connect to the IMAP server: {err}"
        )));
    }

    accounts::add_account_to_disk(&account).map_err(AppError::from)?;

    // Kick off background sync.
    let state = app.state::<AppState>();
    state
        .sync
        .start(app.clone(), state.db.clone(), account.clone());

    Ok(account.public())
}

/// Run the interactive Gmail OAuth flow and register the account.
#[tauri::command]
pub async fn start_gmail_oauth(app: AppHandle) -> AppResult<accounts::AccountPublic> {
    let outcome = crate::auth::oauth::run_oauth_flow(&app)
        .await
        .map_err(AppError::from)?;

    // Reuse an existing account for this email if present.
    let existing = accounts::load_accounts()
        .map_err(AppError::from)?
        .into_iter()
        .find(|a| a.email == outcome.email && a.kind == AccountKind::Gmail);

    let account = match existing {
        Some(a) => a,
        None => Account {
            id: uuid::Uuid::new_v4().to_string(),
            email: outcome.email.clone(),
            display_name: outcome.email.clone(),
            kind: AccountKind::Gmail,
            imap_host: "imap.gmail.com".to_string(),
            imap_port: 993,
            smtp_host: "smtp.gmail.com".to_string(),
            smtp_port: 587,
            username: outcome.email.clone(),
        },
    };

    accounts::set_oauth_refresh_token(&account.id, &outcome.refresh_token)
        .map_err(AppError::from)?;
    crate::auth::oauth::cache_token(&account.id, &outcome.access_token, outcome.expires_in);

    // Persist if new.
    if accounts::load_accounts()
        .map_err(AppError::from)?
        .iter()
        .all(|a| a.id != account.id)
    {
        accounts::add_account_to_disk(&account).map_err(AppError::from)?;
    }

    let state = app.state::<AppState>();
    state
        .sync
        .start(app.clone(), state.db.clone(), account.clone());

    Ok(account.public())
}

/// Re-run the interactive Gmail OAuth consent for an existing account.
///
/// Recovers from dead credentials (e.g. the 7-day refresh-token expiry of a
/// "Testing"-status Google OAuth client) in place: the account, its folders,
/// and its cached mail are untouched — only the keyring refresh token is
/// replaced. Errors without storing anything if the completed consent belongs
/// to a different Google account than the one being reconnected.
#[tauri::command]
pub async fn reauth_gmail_account(
    app: AppHandle,
    account_id: String,
) -> AppResult<accounts::AccountPublic> {
    let account = load_account(&account_id)?;
    if account.kind != AccountKind::Gmail {
        return Err(AppError::msg(
            "Only Gmail accounts use OAuth re-authentication",
        ));
    }

    let outcome = crate::auth::oauth::run_oauth_flow(&app)
        .await
        .map_err(AppError::from)?;

    if !outcome.email.eq_ignore_ascii_case(&account.email) {
        return Err(AppError::msg(format!(
            "You signed in as {}, but this account is {}. Sign in with the matching Google account.",
            outcome.email, account.email
        )));
    }

    accounts::set_oauth_refresh_token(&account.id, &outcome.refresh_token)
        .map_err(AppError::from)?;
    crate::auth::oauth::cache_token(&account.id, &outcome.access_token, outcome.expires_in);

    let state = app.state::<AppState>();
    state
        .sync
        .start(app.clone(), state.db.clone(), account.clone());

    Ok(account.public())
}

/// Remove an account, its cached data, secrets, and running sync task.
#[tauri::command]
pub fn remove_account(app: AppHandle, account_id: String) -> AppResult<()> {
    let state = app.state::<AppState>();
    state.sync.stop(&account_id);

    let removed = accounts::remove_account_from_disk(&account_id).map_err(AppError::from)?;
    if let Some(account) = &removed {
        accounts::delete_secrets(&account.id, account.kind);
        if account.kind == AccountKind::Gmail {
            crate::auth::oauth::forget(&account.id);
        }
    }

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::msg("db lock poisoned"))?;
    store::delete_account_data(&conn, &account_id).map_err(AppError::from)?;
    Ok(())
}

/// List a folder's folders.
#[tauri::command]
pub fn list_folders(state: State<'_, AppState>, account_id: String) -> AppResult<Vec<Folder>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::msg("db lock poisoned"))?;
    let rows = store::list_folders(&conn, &account_id).map_err(AppError::from)?;
    Ok(rows.into_iter().map(Folder::from).collect())
}

/// Page messages for a folder, newest first.
#[tauri::command]
pub fn list_messages(
    state: State<'_, AppState>,
    folder_id: i64,
    offset: i64,
    limit: i64,
) -> AppResult<Vec<MessageSummary>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::msg("db lock poisoned"))?;
    let folder = store::get_folder(&conn, folder_id)
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::msg("folder not found"))?;
    let rows = store::page_messages(&conn, folder_id, offset, limit).map_err(AppError::from)?;
    Ok(rows
        .into_iter()
        .map(|r| MessageSummary::from_row(r, folder.account_id.clone()))
        .collect())
}

/// Page messages across all inbox folders for all accounts, newest first.
#[tauri::command]
pub fn list_unified_messages(
    state: State<'_, AppState>,
    offset: i64,
    limit: i64,
) -> AppResult<Vec<MessageSummary>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::msg("db lock poisoned"))?;
    let rows = store::page_unified_messages(&conn, offset, limit).map_err(AppError::from)?;
    Ok(rows
        .into_iter()
        .map(|r| MessageSummary::from_row(r.msg, r.account_id))
        .collect())
}

/// Full-text search over the local cache, relevance-ranked.
///
/// Searches cached envelopes and bodies via SQLite FTS5. When `account_id` is
/// `null` the search spans every account; otherwise it is scoped to that
/// account. This is local-cache-only — server-side IMAP SEARCH is not involved.
#[tauri::command]
pub fn search_messages(
    state: State<'_, AppState>,
    query: String,
    account_id: Option<String>,
    offset: i64,
    limit: i64,
) -> AppResult<Vec<MessageSummary>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::msg("db lock poisoned"))?;
    let rows = store::search_messages(&conn, &query, account_id.as_deref(), offset, limit)
        .map_err(AppError::from)?;
    Ok(rows
        .into_iter()
        .map(|r| MessageSummary::from_row(r.msg, r.account_id))
        .collect())
}

/// Get a message body, fetching from the server (and caching) if not present.
#[tauri::command]
pub async fn get_message_body(app: AppHandle, message_id: i64) -> AppResult<MessageBody> {
    let state = app.state::<AppState>();

    // Fast path: cached body.
    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        if let Some(cached) = store::get_body(&conn, message_id).map_err(AppError::from)? {
            if cached.cached {
                let attachments = store::list_attachments(&conn, message_id)
                    .map_err(AppError::from)?
                    .into_iter()
                    .map(AttachmentInfo::from)
                    .collect();
                return Ok(MessageBody {
                    id: message_id,
                    html: cached.html,
                    text: cached.text,
                    to_addrs: cached.to_addrs,
                    cc_addrs: cached.cc_addrs,
                    attachments,
                });
            }
        }
    }

    // Locate the message and its owning account/folder.
    let (_, uid, account_id, folder_name) = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        store::locate_message(&conn, message_id)
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("message not found"))?
    };

    let account = load_account(&account_id)?;

    let mut session = sync_imap::connect(&account).await.map_err(AppError::from)?;
    sync_imap::select(&mut session, &folder_name)
        .await
        .map_err(AppError::from)?;
    let body = sync_imap::fetch_body(&mut session, uid)
        .await
        .map_err(AppError::from)?;
    let _ = session.logout().await;

    // Cache the fetched parts and attachment metadata.
    let attachments = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        store::set_body(
            &conn,
            message_id,
            body.text.as_deref(),
            body.html.as_deref(),
            &body.to_addrs,
            &body.cc_addrs,
            sync_imap::snippet_for_body(body.text.as_deref(), body.html.as_deref()).as_deref(),
        )
        .map_err(AppError::from)?;
        store::replace_attachments(&conn, message_id, &body.attachments).map_err(AppError::from)?;

        let detected = shipments::extract_shipments(body.text.as_deref(), body.html.as_deref());
        let inserts: Vec<store::ShipmentInsert> =
            detected.into_iter().map(shipment_insert).collect();
        store::replace_shipments(&conn, message_id, &inserts).map_err(AppError::from)?;

        store::list_attachments(&conn, message_id)
            .map_err(AppError::from)?
            .into_iter()
            .map(AttachmentInfo::from)
            .collect()
    };

    Ok(MessageBody {
        id: message_id,
        html: body.html,
        text: body.text,
        to_addrs: body.to_addrs,
        cc_addrs: body.cc_addrs,
        attachments,
    })
}

/// Map a heuristic-detected shipment to its stored form (carrier reduced to
/// its stable DB code). Shared with the background prefetch hook in `sync::mod`.
pub(crate) fn shipment_insert(s: shipments::ExtractedShipment) -> store::ShipmentInsert {
    store::ShipmentInsert {
        carrier: s.carrier.as_str().to_string(),
        tracking_number: s.tracking_number,
        tracking_url: s.tracking_url,
        order_id: s.order_id,
    }
}

/// List shipments detected in a message's cached body (empty until the body
/// has been fetched, or if none were detected).
#[tauri::command]
pub fn list_shipments_for_message(app: AppHandle, message_id: i64) -> AppResult<Vec<Shipment>> {
    let state = app.state::<AppState>();
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::msg("db lock poisoned"))?;
    let rows = store::list_shipments(&conn, message_id).map_err(AppError::from)?;
    Ok(rows.into_iter().map(Shipment::from).collect())
}

/// Save an attachment to the user's downloads directory, returning its path.
///
/// Raw RFC822 is not cached, so this refetches the message (`BODY.PEEK[]`,
/// non-marking), re-parses, extracts the part by its stable index, decodes it,
/// and writes it under a sanitized, collision-suffixed filename.
#[tauri::command]
pub async fn save_attachment(app: AppHandle, attachment_id: i64) -> AppResult<String> {
    let state = app.state::<AppState>();

    let location = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        store::get_attachment(&conn, attachment_id)
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("attachment not found"))?
    };

    let account = load_account(&location.account_id)?;

    let mut session = sync_imap::connect(&account).await.map_err(AppError::from)?;
    sync_imap::select(&mut session, &location.folder_name)
        .await
        .map_err(AppError::from)?;
    let bytes = sync_imap::fetch_attachment_bytes(&mut session, location.uid, location.part_index)
        .await
        .map_err(AppError::from)?;
    let _ = session.logout().await;

    let dir = crate::attachments::downloads_dir().map_err(AppError::from)?;
    let name =
        crate::attachments::safe_filename(&location.filename, &location.mime_type, attachment_id);
    let path = crate::attachments::unique_path(&dir, &name);
    std::fs::write(&path, &bytes)
        .map_err(|e| AppError::msg(format!("Could not save attachment: {e}")))?;

    Ok(path.to_string_lossy().into_owned())
}

/// Set/clear the seen flag on the server and in the local cache.
///
/// On a Gmail account, the same physical message is exposed under multiple
/// folders (labels). After the primary update, this also updates the seen
/// state of any other cached copies sharing the same RFC 5322 Message-ID
/// within the account, so the unread count and paperclip-adjacent state
/// don't go stale in other folders until they're next synced (Gmail
/// propagates `\Seen` across labels server-side; the next sync of those
/// folders reconciles regardless).
#[tauri::command]
pub async fn mark_read(app: AppHandle, message_id: i64, seen: bool) -> AppResult<()> {
    let state = app.state::<AppState>();

    let (folder_id, uid, account_id, folder_name) = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        store::locate_message(&conn, message_id)
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("message not found"))?
    };

    let account = load_account(&account_id)?;

    let mut session = sync_imap::connect(&account).await.map_err(AppError::from)?;
    sync_imap::select(&mut session, &folder_name)
        .await
        .map_err(AppError::from)?;
    sync_imap::set_seen_flag(&mut session, uid, seen)
        .await
        .map_err(AppError::from)?;
    let _ = session.logout().await;

    let mut sibling_folders = Vec::new();
    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        let changed = store::mark_seen(&conn, message_id, seen).map_err(AppError::from)?;
        if changed {
            let delta = if seen { -1 } else { 1 };
            store::adjust_folder_unread_count(&conn, folder_id, delta).map_err(AppError::from)?;
        }
        if account.kind == AccountKind::Gmail {
            let header = store::message_id_header(&conn, message_id).map_err(AppError::from)?;
            if let Some(header) = header.filter(|h| !h.is_empty()) {
                sibling_folders = store::mark_seen_for_message_id_siblings(
                    &conn,
                    &account_id,
                    &header,
                    message_id,
                    seen,
                )
                .map_err(AppError::from)?;
            }
        }
    }

    emit_messages_updated(&app, folder_id);
    for sibling_folder_id in sibling_folders {
        emit_messages_updated(&app, sibling_folder_id);
    }
    Ok(())
}

/// Set/clear the `\Flagged` flag on the server and in the local cache.
#[tauri::command]
pub async fn mark_flagged(app: AppHandle, message_id: i64, flagged: bool) -> AppResult<()> {
    let state = app.state::<AppState>();

    let (folder_id, uid, account_id, folder_name) = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        store::locate_message(&conn, message_id)
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("message not found"))?
    };

    let account = load_account(&account_id)?;

    let mut session = sync_imap::connect(&account).await.map_err(AppError::from)?;
    sync_imap::select(&mut session, &folder_name)
        .await
        .map_err(AppError::from)?;
    sync_imap::set_flagged_flag(&mut session, uid, flagged)
        .await
        .map_err(AppError::from)?;
    let _ = session.logout().await;

    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        store::set_flagged(&conn, message_id, flagged).map_err(AppError::from)?;
    }

    emit_messages_updated(&app, folder_id);
    Ok(())
}

/// Move a message to another folder of the same account.
///
/// Validates the target exists, belongs to the same account, and differs from
/// the source, then performs the server move and removes the local row (the
/// message reappears in the target on its next sync — no fabricated local row).
#[tauri::command]
pub async fn move_message(app: AppHandle, message_id: i64, target_folder_id: i64) -> AppResult<()> {
    perform_move(&app, message_id, target_folder_id).await
}

/// Move a message to the account's archive-role folder.
#[tauri::command]
pub async fn archive_message(app: AppHandle, message_id: i64) -> AppResult<()> {
    let state = app.state::<AppState>();
    let target_folder_id = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        let ctx = store::message_action_context(&conn, message_id)
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("message not found"))?;
        store::find_folder_by_role(&conn, &ctx.account_id, store::FolderRole::Archive)
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("This account has no archive folder"))?
            .id
    };
    perform_move(&app, message_id, target_folder_id).await
}

/// Delete a message: permanent when already in trash, otherwise move to trash.
#[tauri::command]
pub async fn delete_message(app: AppHandle, message_id: i64) -> AppResult<()> {
    let state = app.state::<AppState>();

    let ctx = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        store::message_action_context(&conn, message_id)
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("message not found"))?
    };

    // Already in trash: permanently delete on the server, then locally.
    if ctx.folder_role == store::FolderRole::Trash.as_str() {
        let account = load_account(&ctx.account_id)?;
        let mut session = sync_imap::connect(&account).await.map_err(AppError::from)?;
        sync_imap::select(&mut session, &ctx.folder_name)
            .await
            .map_err(AppError::from)?;
        sync_imap::delete_permanently(&mut session, ctx.uid)
            .await
            .map_err(AppError::from)?;
        let _ = session.logout().await;

        {
            let conn = state
                .db
                .lock()
                .map_err(|_| AppError::msg("db lock poisoned"))?;
            store::remove_message(&conn, message_id).map_err(AppError::from)?;
        }
        emit_messages_updated(&app, ctx.folder_id);
        return Ok(());
    }

    // Otherwise move to the account's trash folder.
    let target_folder_id = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        store::find_folder_by_role(&conn, &ctx.account_id, store::FolderRole::Trash)
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("This account has no trash folder"))?
            .id
    };
    perform_move(&app, message_id, target_folder_id).await
}

/// Submit a plain-text message through the selected account's SMTP server.
#[tauri::command]
pub async fn send_message(app: AppHandle, input: SendMessageInput) -> AppResult<()> {
    let account = load_account(&input.account_id)?;
    let reply_message_id = if let Some(local_message_id) = input.reply_to_message_id {
        let state = app.state::<AppState>();
        let metadata = {
            let conn = state
                .db
                .lock()
                .map_err(|_| AppError::msg("db lock poisoned"))?;
            store::get_reply_metadata(&conn, local_message_id)
                .map_err(AppError::from)?
                .ok_or_else(|| AppError::msg("reply message not found"))?
        };
        if metadata.account_id != account.id {
            return Err(AppError::msg("reply message belongs to another account"));
        }
        metadata.message_id
    } else {
        None
    };

    crate::send::send(&account, &input, reply_message_id.as_deref())
        .await
        .map_err(AppError::from)
}

/// Trigger a background re-sync for the account that owns a folder.
#[tauri::command]
pub fn sync_folder(app: AppHandle, folder_id: i64) -> AppResult<()> {
    let state = app.state::<AppState>();
    let account_id = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        store::get_folder(&conn, folder_id)
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("folder not found"))?
            .account_id
    };
    restart_sync(&app, &account_id)
}

/// Trigger a background re-sync for an account.
#[tauri::command]
pub fn sync_account(app: AppHandle, account_id: String) -> AppResult<()> {
    restart_sync(&app, &account_id)
}

/// Restart every configured account's background sync task.
///
/// This is backend-only shared behavior for the tray menu, not a frontend
/// command, so it does not extend the Tauri wire surface.
pub(crate) fn sync_all_accounts(app: &AppHandle) -> AppResult<usize> {
    let accounts = accounts::load_accounts().map_err(AppError::from)?;
    let count = accounts.len();
    let state = app.state::<AppState>();
    for account in accounts {
        state.sync.start(app.clone(), state.db.clone(), account);
    }
    Ok(count)
}

/// Send a sample notification.
#[tauri::command]
pub fn test_notification(app: AppHandle) -> AppResult<()> {
    crate::notifications::test(&app);
    Ok(())
}

/// Discover IMAP/SMTP settings for an email address (Thunderbird-style).
///
/// Errors only on an invalid address; a "not found" result falls through to a
/// heuristic guess with `confident: false`.
#[tauri::command]
pub async fn discover_account_config(
    email: String,
) -> AppResult<crate::autoconfig::DiscoveredConfig> {
    crate::autoconfig::discover(&email)
        .await
        .map_err(|_| AppError::msg("Please enter a valid email address"))
}

/// Read global application settings.
///
/// Never errors: a missing or malformed settings file yields defaults so the
/// UI always has a value to render.
#[tauri::command]
pub fn get_settings() -> Settings {
    settings::load_settings()
}

/// Persist global application settings and return the stored value.
#[tauri::command]
pub fn update_settings(settings: Settings) -> AppResult<Settings> {
    settings::save_settings(&settings).map_err(AppError::from)?;
    Ok(settings)
}

// --- helpers -----------------------------------------------------------------

/// Emit `mail:messages-updated { folderId }` (flags/deletions ⇒ refetch).
fn emit_messages_updated(app: &AppHandle, folder_id: i64) {
    let _ = tauri::Emitter::emit(
        app,
        "mail:messages-updated",
        serde_json::json!({ "folderId": folder_id }),
    );
}

/// Move a message to `target_folder_id`, shared by move/archive/delete-to-trash.
///
/// Validates the target (exists, same account, not the source folder) before any
/// network work, performs the server move, removes the local row, bumps the
/// target folder's counts, and emits `mail:messages-updated` for both folders.
async fn perform_move(app: &AppHandle, message_id: i64, target_folder_id: i64) -> AppResult<()> {
    let state = app.state::<AppState>();

    let (ctx, target_name) = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        let ctx = store::message_action_context(&conn, message_id)
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("message not found"))?;
        let target = store::get_folder(&conn, target_folder_id)
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::msg("Target folder not found"))?;
        if target.account_id != ctx.account_id {
            return Err(AppError::msg(
                "Moving messages between accounts is not supported",
            ));
        }
        if target.id == ctx.folder_id {
            return Err(AppError::msg("The message is already in that folder"));
        }
        (ctx, target.name)
    };

    let account = load_account(&ctx.account_id)?;

    let mut session = sync_imap::connect(&account).await.map_err(AppError::from)?;
    sync_imap::select(&mut session, &ctx.folder_name)
        .await
        .map_err(AppError::from)?;
    sync_imap::move_message(&mut session, ctx.uid, &target_name)
        .await
        .map_err(AppError::from)?;
    let _ = session.logout().await;

    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        store::remove_message(&conn, message_id).map_err(AppError::from)?;
        store::increment_folder_counts(&conn, target_folder_id, !ctx.seen)
            .map_err(AppError::from)?;
    }

    emit_messages_updated(app, ctx.folder_id);
    emit_messages_updated(app, target_folder_id);
    Ok(())
}

fn load_account(account_id: &str) -> AppResult<Account> {
    accounts::load_accounts()
        .map_err(AppError::from)?
        .into_iter()
        .find(|a| a.id == account_id)
        .ok_or_else(|| AppError::msg("account not found"))
}

fn restart_sync(app: &AppHandle, account_id: &str) -> AppResult<()> {
    let account = load_account(account_id)?;
    let state = app.state::<AppState>();
    state.sync.start(app.clone(), state.db.clone(), account);
    Ok(())
}
