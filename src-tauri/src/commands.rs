//! Tauri command handlers.
//!
//! These are intentionally thin: they validate inputs, delegate to the
//! `store`/`sync`/`auth` modules, and translate errors to [`AppError`] at the
//! boundary so the frontend sees plain strings.

use tauri::{AppHandle, Manager, State};

use crate::accounts::{self, Account, AccountKind};
use crate::error::{AppError, AppResult};
use crate::omarchy::{self, OmarchyTheme};
use crate::state::AppState;
use crate::store;
use crate::sync::imap as sync_imap;
use crate::wire::{Folder, ImapAccountInput, MessageBody, MessageSummary, SendMessageInput};

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
            if cached.text.is_some() || cached.html.is_some() {
                return Ok(MessageBody {
                    id: message_id,
                    html: cached.html,
                    text: cached.text,
                    to_addrs: cached.to_addrs,
                    cc_addrs: cached.cc_addrs,
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

    // Cache the fetched parts.
    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::msg("db lock poisoned"))?;
        store::set_body(
            &conn,
            message_id,
            body.text.as_deref(),
            body.html.as_deref(),
        )
        .map_err(AppError::from)?;
    }

    Ok(MessageBody {
        id: message_id,
        html: body.html,
        text: body.text,
        to_addrs: body.to_addrs,
        cc_addrs: body.cc_addrs,
    })
}

/// Set/clear the seen flag on the server and in the local cache.
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
    }

    let _ = tauri::Emitter::emit(
        &app,
        "mail:messages-updated",
        serde_json::json!({ "folderId": folder_id }),
    );
    Ok(())
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

// --- helpers -----------------------------------------------------------------

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
