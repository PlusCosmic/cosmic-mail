//! State transitions: a port of the Svelte `MailStore`.
//!
//! Every backend call goes through [`Ui::spawn`]; its result lands on the UI
//! thread, where a guard checks the view it was fetched for is still current
//! before applying it. Flag, read, move, archive and delete are optimistic and
//! roll back (or refetch) on failure.

use rune::Tx;

use super::compose::ComposeSeed;
use super::{html, AddTab, Filter, Modal, SettingsState, Ui, PAGE_LIMIT};
use crate::accounts::AccountKind;
use crate::events::AppEvent;
use crate::settings::Settings;
use crate::sync::SyncState;
use crate::wire::{AttachmentInfo, Folder, ImapAccountInput, MessageSummary, SendMessageInput};

/// Which list a page was fetched for; a result lands only if it still matches.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ListKey {
    Search {
        query: String,
        account: Option<String>,
    },
    Unified,
    Folder(i64),
}

/// The item that takes the selection after `removed` leaves `ids`: the next
/// one, else the previous, else none.
pub fn next_selection_id(ids: &[i64], removed: i64) -> Option<i64> {
    let i = ids.iter().position(|&id| id == removed)?;
    ids.get(i + 1)
        .or_else(|| i.checked_sub(1).and_then(|p| ids.get(p)))
        .copied()
}

impl Ui {
    // ---- Derived ----------------------------------------------------------

    /// Messages after the filter chip and live query — what the list shows
    /// and keyboard navigation walks.
    pub fn visible_messages(&self) -> Vec<&MessageSummary> {
        let q = self.query.text.trim().to_lowercase();
        self.messages
            .iter()
            .filter(|m| match self.filter {
                Filter::All => true,
                // Keep the selected message visible once read, so opening it
                // under Unread doesn't yank it (and the reader) away.
                Filter::Unread => !m.seen || Some(m.id) == self.selected_message,
                Filter::Flagged => m.flagged,
            })
            .filter(|m| {
                q.is_empty()
                    || [&m.from_name, &m.from_addr, &m.subject, &m.snippet]
                        .iter()
                        .any(|f| f.to_lowercase().contains(&q))
            })
            .collect()
    }

    pub fn selected_message(&self) -> Option<&MessageSummary> {
        let id = self.selected_message?;
        self.visible_messages().into_iter().find(|m| m.id == id)
    }

    /// Index of the selection within the visible messages.
    pub fn selected_index(&self) -> Option<usize> {
        let id = self.selected_message?;
        self.visible_messages().iter().position(|m| m.id == id)
    }

    pub fn folder(&self, folder_id: i64) -> Option<&Folder> {
        self.folders
            .values()
            .flat_map(|l| l.iter())
            .find(|f| f.id == folder_id)
    }

    pub fn selected_folder(&self) -> Option<&Folder> {
        self.folder(self.selected_folder?)
    }

    fn account_for_folder(&self, folder_id: i64) -> Option<String> {
        self.folders
            .iter()
            .find(|(_, l)| l.iter().any(|f| f.id == folder_id))
            .map(|(id, _)| id.clone())
    }

    /// Unread across every account's inbox.
    pub fn unified_unread(&self) -> i64 {
        self.folders
            .values()
            .flat_map(|l| l.iter())
            .filter(|f| f.role == "inbox")
            .map(|f| f.unread_count)
            .sum()
    }

    /// Unread in one account's inbox.
    pub fn account_unread(&self, account_id: &str) -> i64 {
        self.folders
            .get(account_id)
            .map(|l| {
                l.iter()
                    .filter(|f| f.role == "inbox")
                    .map(|f| f.unread_count)
                    .sum()
            })
            .unwrap_or(0)
    }

    pub fn sync_state(&self, account_id: &str) -> SyncState {
        self.sync_states
            .get(account_id)
            .copied()
            .unwrap_or(SyncState::Idle)
    }

    /// The account a new message is sent from by default.
    pub fn default_compose_account(&self) -> Option<String> {
        self.selected_account
            .clone()
            .or_else(|| self.selected_message().map(|m| m.account_id.clone()))
            .or_else(|| self.accounts.first().map(|a| a.id.clone()))
    }

    fn list_key(&self) -> Option<ListKey> {
        if self.search_active {
            Some(ListKey::Search {
                query: self.search_query.clone(),
                account: if self.unified {
                    None
                } else {
                    self.selected_account.clone()
                },
            })
        } else if self.unified {
            Some(ListKey::Unified)
        } else {
            self.selected_folder.map(ListKey::Folder)
        }
    }

    // ---- Startup ----------------------------------------------------------

    /// Load settings and accounts (called once the window is up).
    pub fn init(&mut self) {
        self.spawn(
            |b| async move { b.get_settings() },
            |ui, settings, _| ui.settings = settings,
        );
        self.load_accounts();
    }

    pub fn load_accounts(&mut self) {
        // Without a backend (scenes, tests) nothing would ever finish loading.
        if self.backend.is_none() {
            return;
        }
        self.loading_accounts = true;
        self.spawn(
            |b| async move {
                let accounts = b.list_accounts()?;
                let folders: Vec<_> = accounts
                    .iter()
                    .map(|a| {
                        (
                            a.id.clone(),
                            b.list_folders(a.id.clone()).unwrap_or_default(),
                        )
                    })
                    .collect();
                Ok::<_, crate::error::AppError>((accounts, folders))
            },
            |ui, result, _| {
                ui.loading_accounts = false;
                let Ok((accounts, folders)) = result else {
                    ui.accounts.clear();
                    return;
                };
                for (id, list) in folders {
                    ui.folders.insert(id, list);
                }
                ui.folders
                    .retain(|id, _| accounts.iter().any(|a| &a.id == id));
                for a in &accounts {
                    ui.sync_states
                        .entry(a.id.clone())
                        .or_insert(SyncState::Idle);
                }
                ui.accounts = accounts;
                if !ui.accounts.is_empty() && ui.selected_folder.is_none() && ui.unified {
                    ui.select_unified();
                }
            },
        );
    }

    pub fn load_folders(&mut self, account_id: String) {
        self.spawn(
            move |b| async move {
                let folders = b.list_folders(account_id.clone()).unwrap_or_default();
                (account_id, folders)
            },
            |ui, (account_id, folders), _| {
                if ui.accounts.iter().any(|a| a.id == account_id) {
                    ui.folders.insert(account_id, folders);
                }
            },
        );
    }

    // ---- Accounts ---------------------------------------------------------

    /// Submit the add-account dialog's IMAP form.
    pub fn submit_imap_account(&mut self) {
        let Some(Modal::AddAccount(form)) = &mut self.modal else {
            return;
        };
        if form.busy {
            return;
        }
        let email = form.email.text.trim().to_string();
        let parse_port = |t: &str, name: &str| -> Result<u16, String> {
            t.trim()
                .parse::<u16>()
                .map_err(|_| format!("{name} must be a port number"))
        };
        let ports = parse_port(&form.imap_port.text, "IMAP port")
            .and_then(|i| parse_port(&form.smtp_port.text, "SMTP port").map(|s| (i, s)));
        let (imap_port, smtp_port) = match ports {
            Ok(p) => p,
            Err(e) => {
                form.error = Some(e);
                return;
            }
        };
        let required = [
            (&email, "Email"),
            (&form.imap_host.text, "IMAP host"),
            (&form.smtp_host.text, "SMTP host"),
            (&form.password.text, "Password"),
        ];
        if let Some((_, name)) = required.iter().find(|(v, _)| v.trim().is_empty()) {
            form.error = Some(format!("{name} is required"));
            return;
        }
        let username = form.username.text.trim();
        let input = ImapAccountInput {
            email: email.clone(),
            display_name: form.display_name.text.trim().to_string(),
            imap_host: form.imap_host.text.trim().to_string(),
            imap_port,
            smtp_host: form.smtp_host.text.trim().to_string(),
            smtp_port,
            // Username often equals the email if left blank.
            username: if username.is_empty() {
                email
            } else {
                username.to_string()
            },
            password: form.password.text.clone(),
        };
        form.error = None;
        form.busy = true;
        self.spawn(
            move |b| async move { b.add_imap_account(input).await },
            |ui, result, _| ui.account_added(result),
        );
    }

    /// Run the Gmail OAuth flow from the add-account dialog.
    pub fn start_gmail_oauth(&mut self) {
        let Some(Modal::AddAccount(form)) = &mut self.modal else {
            return;
        };
        if form.busy {
            return;
        }
        form.error = None;
        form.busy = true;
        self.spawn(
            |b| async move { b.start_gmail_oauth().await },
            |ui, result, _| ui.account_added(result),
        );
    }

    fn account_added(
        &mut self,
        result: Result<crate::accounts::AccountPublic, crate::error::AppError>,
    ) {
        match result {
            Ok(account) => {
                if matches!(self.modal, Some(Modal::AddAccount(_))) {
                    self.modal = None;
                }
                self.load_accounts();
                self.toast(format!("Added {}", account.email), false);
            }
            Err(err) => match &mut self.modal {
                Some(Modal::AddAccount(form)) => {
                    form.busy = false;
                    form.error = Some(err.to_string());
                }
                _ => self.error("Failed to add account", err),
            },
        }
    }

    /// Autodiscover server settings for the typed address.
    pub fn run_discovery(&mut self) {
        let Some(Modal::AddAccount(form)) = &mut self.modal else {
            return;
        };
        let email = form.email.text.trim().to_string();
        let at = email.find('@');
        let looks_like_email =
            at.is_some_and(|i| i > 0 && i < email.len() - 1) && !email.contains(' ');
        // Don't clobber hand-edited fields, and don't repeat a finished lookup.
        if !looks_like_email
            || form.host_fields_touched()
            || (form.discovered_for.as_deref() == Some(email.as_str()) && form.discovered.is_some())
            || form.detecting
        {
            return;
        }
        form.detecting = true;
        let asked = email.clone();
        self.spawn(
            move |b| async move { b.discover_account_config(asked).await },
            move |ui, result, _| {
                let Some(Modal::AddAccount(form)) = &mut ui.modal else {
                    return;
                };
                form.detecting = false;
                if form.email.text.trim() != email {
                    return; // the address changed while we looked
                }
                match result {
                    Ok(cfg) => {
                        if cfg.kind == crate::autoconfig::DiscoveredKind::Imap
                            && !form.host_fields_touched()
                        {
                            form.imap_host.replace_text(cfg.imap_host.clone());
                            form.imap_port.replace_text(cfg.imap_port.to_string());
                            form.smtp_host.replace_text(cfg.smtp_host.clone());
                            form.smtp_port.replace_text(cfg.smtp_port.to_string());
                            if !cfg.username.is_empty() {
                                form.username.replace_text(cfg.username.clone());
                            }
                            form.autofilled = Some(form.host_fields());
                        }
                        form.discovered = Some(cfg);
                        form.discovered_for = Some(email);
                    }
                    Err(_) => {
                        // Discovery failure is silent — keep manual entry.
                        form.discovered = None;
                        form.discovered_for = None;
                    }
                }
            },
        );
    }

    /// The email field changed: a result for another address is stale.
    pub fn email_edited(&mut self) {
        if let Some(Modal::AddAccount(form)) = &mut self.modal {
            if form
                .discovered_for
                .as_deref()
                .is_some_and(|d| d != form.email.text.trim())
            {
                form.discovered = None;
                form.discovered_for = None;
            }
        }
    }

    /// Re-run Gmail consent for an existing account, in place.
    pub fn reauth_account(&mut self, account_id: String) {
        if let Some(Modal::Settings(st)) = &mut self.modal {
            if st.reauthing.is_some() {
                return;
            }
            st.reauthing = Some(account_id.clone());
        }
        self.spawn(
            move |b| async move {
                let r = b.reauth_gmail_account(account_id.clone()).await;
                (account_id, r)
            },
            |ui, (account_id, result), _| {
                if let Some(Modal::Settings(st)) = &mut ui.modal {
                    st.reauthing = None;
                }
                match result {
                    Ok(account) => {
                        ui.reauth_required.remove(&account_id);
                        ui.toast(format!("Reconnected {}", account.email), false);
                    }
                    Err(err) => ui.error("Failed to reconnect account", err),
                }
            },
        );
    }

    /// The settings dialog's Remove button: the first click arms, the second removes.
    pub fn request_remove_account(&mut self, account_id: String) {
        let Some(Modal::Settings(st)) = &mut self.modal else {
            return;
        };
        if st.pending_removal.as_ref().map(|(id, _)| id) == Some(&account_id) {
            st.pending_removal = None;
            self.remove_account(account_id);
        } else {
            st.pending_removal = Some((account_id, None));
        }
    }

    pub fn remove_account(&mut self, account_id: String) {
        self.spawn(
            move |b| async move {
                let r = b.remove_account(account_id.clone());
                (account_id, r)
            },
            |ui, (account_id, result), _| match result {
                Ok(()) => {
                    ui.folders.remove(&account_id);
                    ui.sync_states.remove(&account_id);
                    ui.reauth_required.remove(&account_id);
                    let showing_it = ui.selected_account.as_deref() == Some(account_id.as_str())
                        || ui.messages.iter().any(|m| m.account_id == account_id);
                    ui.accounts.retain(|a| a.id != account_id);
                    if showing_it {
                        ui.selected_account = None;
                        ui.selected_folder = None;
                        ui.unified = true;
                        ui.clear_reader();
                        ui.messages.clear();
                        if !ui.accounts.is_empty() {
                            ui.select_unified();
                        }
                    }
                    ui.load_accounts();
                    ui.toast("Account removed", false);
                }
                Err(err) => ui.error("Failed to remove account", err),
            },
        );
    }

    // ---- Selection --------------------------------------------------------

    fn clear_reader(&mut self) {
        self.selected_message = None;
        self.body = None;
        self.body_lines.clear();
        self.body_images.clear();
        self.shipments.clear();
        self.loading_body = false;
    }

    fn reset_list(&mut self) {
        self.clear_reader();
        self.offset = 0;
        self.messages.clear();
        self.has_more = false;
    }

    /// Show every account's inbox.
    pub fn select_unified(&mut self) {
        self.unified = true;
        self.selected_account = None;
        self.selected_folder = None;
        self.filter = Filter::All;
        self.query.replace_text("");
        self.search_active = false;
        self.search_query.clear();
        self.reset_list();
        self.fetch_page(false);
    }

    /// Show one folder (per-account mode).
    pub fn select_folder(&mut self, folder_id: i64) {
        let Some(account_id) = self.account_for_folder(folder_id) else {
            return;
        };
        self.unified = false;
        self.selected_account = Some(account_id);
        self.selected_folder = Some(folder_id);
        self.filter = Filter::All;
        self.query.replace_text("");
        self.search_active = false;
        self.search_query.clear();
        self.reset_list();
        self.fetch_page(false);
    }

    /// Open an account's inbox (or its first folder).
    pub fn select_account(&mut self, account_id: &str) {
        let folders = self.folders.get(account_id).cloned().unwrap_or_default();
        let target = folders
            .iter()
            .find(|f| f.role == "inbox")
            .or(folders.first());
        if let Some(folder) = target {
            self.select_folder(folder.id);
        } else {
            // No folders yet: the account, without messages.
            self.unified = false;
            self.selected_account = Some(account_id.to_string());
            self.selected_folder = None;
            self.search_active = false;
            self.search_query.clear();
            self.reset_list();
        }
    }

    /// Search the whole local cache for the query (Enter in the search box).
    pub fn run_search(&mut self) {
        let q = self.query.text.trim().to_string();
        if q.is_empty() {
            self.clear_search();
            return;
        }
        self.search_active = true;
        self.search_query = q;
        // The live filter would further constrain ranked results; the search
        // indicator carries the term instead.
        self.query.replace_text("");
        self.reset_list();
        self.fetch_page(false);
    }

    /// Leave search mode and refetch the underlying view.
    pub fn clear_search(&mut self) {
        if !self.search_active {
            return;
        }
        self.search_active = false;
        self.search_query.clear();
        self.reset_list();
        self.fetch_page(false);
    }

    fn fetch_page(&mut self, append: bool) {
        let Some(key) = self.list_key() else {
            self.loading_messages = false;
            return;
        };
        self.loading_messages = true;
        let offset = self.offset;
        let asked = key.clone();
        self.spawn(
            move |b| async move {
                match asked {
                    ListKey::Search { query, account } => {
                        b.search_messages(query, account, offset, PAGE_LIMIT)
                    }
                    ListKey::Unified => b.list_unified_messages(offset, PAGE_LIMIT),
                    ListKey::Folder(id) => b.list_messages(id, offset, PAGE_LIMIT),
                }
            },
            move |ui, result, tx| {
                if ui.list_key().as_ref() != Some(&key) {
                    return; // the view changed while this page loaded
                }
                ui.loading_messages = false;
                match result {
                    Ok(page) => {
                        ui.has_more = page.len() as i64 == PAGE_LIMIT;
                        let page: Vec<MessageSummary> = page
                            .into_iter()
                            .filter(|m| !ui.pending_removals.contains(&m.id))
                            .collect();
                        if append {
                            let known: std::collections::HashSet<i64> =
                                ui.messages.iter().map(|m| m.id).collect();
                            ui.messages
                                .extend(page.into_iter().filter(|m| !known.contains(&m.id)));
                        } else {
                            ui.messages = page;
                        }
                        ui.continue_jump(tx);
                    }
                    Err(err) => {
                        ui.jump_pages_left = 0;
                        if append {
                            ui.error("Failed to load more messages", err);
                        } else {
                            ui.messages.clear();
                        }
                    }
                }
            },
        );
    }

    pub fn load_more(&mut self) {
        if !self.has_more || self.loading_messages {
            return;
        }
        self.offset += PAGE_LIMIT;
        self.fetch_page(true);
    }

    /// `G`: select the last loaded message, then keep loading pages (capped)
    /// until the true last message is reached.
    pub fn jump_to_bottom(&mut self, tx: &mut Tx) {
        const JUMP_TO_BOTTOM_LOAD_CAP: u32 = 20;
        self.select_last(tx);
        if self.has_more {
            self.jump_pages_left = JUMP_TO_BOTTOM_LOAD_CAP;
            self.load_more();
        }
    }

    fn continue_jump(&mut self, tx: &mut Tx) {
        if self.jump_pages_left == 0 {
            return;
        }
        self.jump_pages_left -= 1;
        if self.has_more && self.jump_pages_left > 0 {
            self.load_more();
        } else {
            self.jump_pages_left = 0;
            self.select_last(tx);
        }
    }

    fn select_last(&mut self, tx: &mut Tx) {
        let n = self.visible_messages().len();
        if n > 0 {
            self.select_index(n - 1, tx);
        }
    }

    /// Refetch the loaded range (after a flag/move/delete elsewhere).
    pub fn refresh_current_page(&mut self) {
        let Some(key) = self.list_key() else { return };
        if matches!(key, ListKey::Search { .. }) {
            return;
        }
        let limit = PAGE_LIMIT.max(self.messages.len() as i64);
        let asked = key.clone();
        self.spawn(
            move |b| async move {
                match asked {
                    ListKey::Unified => b.list_unified_messages(0, limit),
                    ListKey::Folder(id) => b.list_messages(id, 0, limit),
                    ListKey::Search { .. } => Ok(Vec::new()),
                }
            },
            move |ui, result, _| {
                if ui.list_key().as_ref() != Some(&key) {
                    return;
                }
                if let Ok(page) = result {
                    ui.has_more = page.len() as i64 == limit;
                    ui.messages = page
                        .into_iter()
                        .filter(|m| !ui.pending_removals.contains(&m.id))
                        .collect();
                }
            },
        );
    }

    /// Select (and open) the visible message at `index`, clamped.
    pub fn select_index(&mut self, index: usize, tx: &mut Tx) {
        let visible = self.visible_messages();
        if visible.is_empty() {
            return;
        }
        let id = visible[index.min(visible.len() - 1)].id;
        self.select_message(id);
        tx.scroll_into_view(format!("msg-{id}"));
    }

    /// Open a message: fetch its body and shipments, then mark it read.
    pub fn select_message(&mut self, id: i64) {
        let Some(msg) = self.messages.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        if self.selected_message == Some(id) && (self.body.is_some() || self.loading_body) {
            return;
        }
        self.selected_message = Some(id);
        self.body = None;
        self.body_lines.clear();
        self.body_images.clear();
        self.shipments.clear();
        self.loading_body = true;
        self.spawn(
            move |b| async move {
                let body = b.get_message_body(id).await;
                // Shipment detection runs as part of body caching backend-side.
                let shipments = b.list_shipments_for_message(id).unwrap_or_default();
                (body, shipments)
            },
            move |ui, (body, shipments), _| {
                if ui.selected_message != Some(id) {
                    return;
                }
                ui.loading_body = false;
                match body {
                    Ok(body) => {
                        ui.body_lines = body.html.as_deref().map(html::render).unwrap_or_default();
                        ui.body = Some(body);
                        ui.shipments = shipments;
                        ui.prepare_body_images();
                    }
                    Err(err) => ui.error("Failed to load message", err),
                }
            },
        );
        // Opening a message marks it read.
        if !msg.seen {
            self.set_seen(id, true);
        }
    }

    // ---- Message actions --------------------------------------------------

    fn patch_message(&mut self, id: i64, f: impl FnOnce(&mut MessageSummary)) {
        if let Some(m) = self.messages.iter_mut().find(|m| m.id == id) {
            f(m);
        }
    }

    fn bump_folder(&mut self, folder_id: i64, total: i64, unread: i64) {
        for f in self.folders.values_mut().flat_map(|l| l.iter_mut()) {
            if f.id == folder_id {
                f.total_count = (f.total_count + total).max(0);
                f.unread_count = (f.unread_count + unread).max(0);
            }
        }
    }

    pub fn toggle_read(&mut self, id: i64) {
        if let Some(seen) = self.messages.iter().find(|m| m.id == id).map(|m| m.seen) {
            self.set_seen(id, !seen);
        }
    }

    /// Set the read state optimistically, rolling back on failure.
    pub fn set_seen(&mut self, id: i64, seen: bool) {
        let Some(msg) = self.messages.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        if msg.seen == seen {
            return;
        }
        let delta = if seen { -1 } else { 1 };
        self.patch_message(id, |m| m.seen = seen);
        self.bump_folder(msg.folder_id, 0, delta);
        self.spawn(
            move |b| async move { b.mark_read(id, seen).await },
            move |ui, result, _| {
                if let Err(err) = result {
                    ui.patch_message(id, |m| m.seen = !seen);
                    ui.bump_folder(msg.folder_id, 0, -delta);
                    ui.error("Failed to update flag", err);
                }
            },
        );
    }

    /// Toggle the flag optimistically.
    pub fn toggle_flagged(&mut self, id: i64) {
        let Some(flagged) = self
            .messages
            .iter()
            .find(|m| m.id == id)
            .map(|m| !m.flagged)
        else {
            return;
        };
        self.patch_message(id, |m| m.flagged = flagged);
        self.spawn(
            move |b| async move { b.mark_flagged(id, flagged).await },
            move |ui, result, _| {
                if let Err(err) = result {
                    ui.patch_message(id, |m| m.flagged = !flagged);
                    ui.error("Failed to update flag", err);
                }
            },
        );
    }

    pub fn archive_message(&mut self, id: i64) {
        self.remove_optimistically(id, None, "Failed to archive message", |b, id| {
            Box::pin(async move { b.archive_message(id).await })
        });
    }

    /// To trash, or permanently when already in trash.
    pub fn delete_message(&mut self, id: i64) {
        self.remove_optimistically(id, None, "Failed to delete message", |b, id| {
            Box::pin(async move { b.delete_message(id).await })
        });
    }

    pub fn move_message(&mut self, id: i64, target_folder_id: i64) {
        if self
            .messages
            .iter()
            .any(|m| m.id == id && m.folder_id == target_folder_id)
        {
            return;
        }
        self.remove_optimistically(
            id,
            Some(target_folder_id),
            "Failed to move message",
            move |b, id| Box::pin(async move { b.move_message(id, target_folder_id).await }),
        );
    }

    /// Shared optimistic removal for archive/delete/move: pick the next
    /// selection, drop the row, adjust folder counts, open the next message,
    /// then run `action`. On failure, refetch the page and folders and toast.
    fn remove_optimistically(
        &mut self,
        id: i64,
        target_folder_id: Option<i64>,
        error_prefix: &'static str,
        action: impl FnOnce(
                std::sync::Arc<crate::backend::Backend>,
                i64,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::error::AppResult<()>> + Send>,
            > + Send
            + 'static,
    ) {
        let Some(msg) = self.messages.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        let visible: Vec<i64> = self.visible_messages().iter().map(|m| m.id).collect();
        let next = next_selection_id(&visible, id);
        let was_selected = self.selected_message == Some(id);

        self.messages.retain(|m| m.id != id);
        self.pending_removals.insert(id);
        let unread = if msg.seen { 0 } else { 1 };
        self.bump_folder(msg.folder_id, -1, -unread);
        if let Some(target) = target_folder_id {
            self.bump_folder(target, 1, unread);
        }
        if was_selected {
            self.clear_reader();
            if let Some(next) = next {
                self.select_message(next);
                self.reveal = Some(format!("msg-{next}"));
            }
        }

        let account_id = msg.account_id.clone();
        self.spawn(
            move |b| action(b, id),
            move |ui, result, _| {
                ui.pending_removals.remove(&id);
                if let Err(err) = result {
                    ui.error(error_prefix, err);
                    ui.refresh_current_page();
                    ui.load_folders(account_id);
                }
            },
        );
    }

    pub fn send_message(&mut self) {
        let Some(Modal::Compose(form)) = &mut self.modal else {
            return;
        };
        if form.busy {
            return;
        }
        let split = |t: &rune::TextEdit| super::compose::split_mailbox_list(&t.text);
        let input = SendMessageInput {
            account_id: form.account_id.clone(),
            to_addrs: split(&form.to),
            cc_addrs: split(&form.cc),
            bcc_addrs: split(&form.bcc),
            subject: form.subject.text.clone(),
            body_text: form.body.text.clone(),
            reply_to_message_id: form.seed.reply_to_message_id,
        };
        if input.to_addrs.is_empty() && input.cc_addrs.is_empty() && input.bcc_addrs.is_empty() {
            form.error = Some("Add at least one recipient".to_string());
            return;
        }
        form.error = None;
        form.busy = true;
        self.spawn(
            move |b| async move { b.send_message(input).await },
            |ui, result, _| match result {
                Ok(()) => {
                    if matches!(ui.modal, Some(Modal::Compose(_))) {
                        ui.modal = None;
                    }
                    ui.toast("Message sent", false);
                }
                Err(err) => match &mut ui.modal {
                    Some(Modal::Compose(form)) => {
                        form.busy = false;
                        form.error = Some(err.to_string());
                    }
                    _ => ui.error("Failed to send message", err),
                },
            },
        );
    }

    pub fn save_attachment(&mut self, attachment: &AttachmentInfo) {
        let id = attachment.id;
        let name = if attachment.filename.is_empty() {
            "attachment".to_string()
        } else {
            attachment.filename.clone()
        };
        self.spawn(
            move |b| async move { b.save_attachment(id).await },
            move |ui, result, _| match result {
                Ok(path) => ui.toast(format!("Saved to {path}"), false),
                Err(err) => ui.error(&format!("Failed to save {name}"), err),
            },
        );
    }

    /// Persist settings optimistically, rolling back on failure.
    pub fn update_settings(&mut self, next: Settings) {
        let prev = std::mem::replace(&mut self.settings, next.clone());
        self.prepare_body_images();
        self.spawn(
            move |b| async move { b.update_settings(next) },
            move |ui, result, _| match result {
                Ok(saved) => ui.settings = saved,
                Err(err) => {
                    ui.settings = prev;
                    ui.error("Failed to save settings", err);
                }
            },
        );
    }

    pub fn sync_selected_folder(&mut self) {
        let Some(folder_id) = self.selected_folder else {
            return;
        };
        self.spawn(
            move |b| async move { b.sync_folder(folder_id) },
            |ui, result, _| {
                if let Err(err) = result {
                    ui.error("Sync failed", err);
                }
            },
        );
    }

    pub fn sync_account(&mut self, account_id: String) {
        self.spawn(
            move |b| async move { b.sync_account(account_id) },
            |ui, result, _| {
                if let Err(err) = result {
                    ui.error("Sync failed", err);
                }
            },
        );
    }

    // ---- Modals -----------------------------------------------------------

    pub fn open_add_account(&mut self) {
        self.modal = Some(Modal::AddAccount(Box::default()));
    }

    pub fn open_settings(&mut self) {
        self.modal = Some(Modal::Settings(SettingsState::default()));
    }

    pub fn open_compose(&mut self) {
        if let Some(account_id) = self.default_compose_account() {
            self.modal = Some(Modal::Compose(Box::new(super::ComposeForm::new(
                ComposeSeed::new_message(account_id),
            ))));
        }
    }

    /// Reply to the selected message (needs its body loaded).
    pub fn open_reply(&mut self, all: bool) {
        let Some(message) = self.selected_message().cloned() else {
            return;
        };
        let Some(account) = self
            .accounts
            .iter()
            .find(|a| a.id == message.account_id)
            .cloned()
        else {
            return;
        };
        let Some(body) = self.body.as_ref().filter(|b| b.id == message.id) else {
            return;
        };
        let quoted_date = super::format::full_date(&message.date);
        let seed = super::compose::reply_seed(&message, body, &account, all, &quoted_date);
        self.modal = Some(Modal::Compose(Box::new(super::ComposeForm::new(seed))));
    }

    pub fn close_modal(&mut self) {
        let busy = match &self.modal {
            Some(Modal::AddAccount(f)) => f.busy,
            Some(Modal::Compose(f)) => f.busy,
            _ => false,
        };
        if !busy {
            self.modal = None;
        }
    }

    pub fn set_add_tab(&mut self, tab: AddTab) {
        if let Some(Modal::AddAccount(form)) = &mut self.modal {
            form.tab = tab;
        }
    }

    // ---- Backend events ---------------------------------------------------

    /// Apply a backend event.
    pub fn on_event(&mut self, event: AppEvent, _tx: &mut Tx) {
        match event {
            AppEvent::NewMessages {
                account_id,
                folder_id,
                messages,
            } => self.on_new_messages(account_id, folder_id, messages),
            AppEvent::MessagesUpdated { folder_id } => self.on_messages_updated(folder_id),
            AppEvent::SyncState {
                account_id,
                state,
                error,
                needs_reauth,
            } => self.on_sync_state(account_id, state, error, needs_reauth),
            AppEvent::Activate => self.activate_requested = true,
        }
    }

    fn prepend_fresh(&mut self, messages: Vec<MessageSummary>) {
        let known: std::collections::HashSet<i64> = self.messages.iter().map(|m| m.id).collect();
        let mut fresh: Vec<MessageSummary> = messages
            .into_iter()
            .filter(|m| !known.contains(&m.id))
            .collect();
        if !fresh.is_empty() {
            fresh.append(&mut self.messages);
            self.messages = fresh;
        }
    }

    fn on_new_messages(
        &mut self,
        account_id: String,
        folder_id: i64,
        messages: Vec<MessageSummary>,
    ) {
        let new_unread = messages.iter().filter(|m| !m.seen).count() as i64;
        // Search results are a ranked list, not a folder view: leave them be.
        if !self.search_active {
            if self.unified {
                match self.folder(folder_id).map(|f| f.role.clone()) {
                    // First sync of a fresh account: its folders aren't known
                    // yet. Learn them, then refresh so inbox mail shows live.
                    None => self.load_folders_then_refresh(account_id),
                    Some(role) if role == "inbox" => self.prepend_fresh(messages),
                    Some(_) => {}
                }
            } else if self.selected_folder == Some(folder_id) {
                self.prepend_fresh(messages);
            }
        }
        if new_unread > 0 {
            self.bump_folder(folder_id, 0, new_unread);
        }
    }

    fn load_folders_then_refresh(&mut self, account_id: String) {
        self.spawn(
            move |b| async move {
                let folders = b.list_folders(account_id.clone()).unwrap_or_default();
                (account_id, folders)
            },
            |ui, (account_id, folders), _| {
                if ui.accounts.iter().any(|a| a.id == account_id) {
                    ui.folders.insert(account_id, folders);
                }
                if ui.unified && !ui.search_active {
                    ui.refresh_current_page();
                }
            },
        );
    }

    fn on_messages_updated(&mut self, folder_id: i64) {
        if !self.search_active {
            let refresh = if self.unified {
                self.folder(folder_id).is_some_and(|f| f.role == "inbox")
            } else {
                self.selected_folder == Some(folder_id)
            };
            if refresh {
                self.refresh_current_page();
            }
        }
        if let Some(account_id) = self.account_for_folder(folder_id) {
            self.load_folders(account_id);
        }
    }

    fn on_sync_state(
        &mut self,
        account_id: String,
        state: SyncState,
        error: Option<String>,
        needs_reauth: bool,
    ) {
        self.sync_states.insert(account_id.clone(), state);
        if state == SyncState::Error && needs_reauth {
            // Dead credentials fail every retry identically: flag the account
            // and toast once on the transition, not per retry.
            if self.reauth_required.insert(account_id.clone()) {
                let email = self
                    .accounts
                    .iter()
                    .find(|a| a.id == account_id)
                    .map(|a| a.email.clone())
                    .unwrap_or_else(|| "an account".to_string());
                self.toast(
                    format!("Gmail sign-in expired for {email} — reconnect it from Settings"),
                    true,
                );
            }
        } else if state == SyncState::Error {
            if let Some(error) = error {
                self.error("Sync error", error);
            }
        } else {
            // Sync is running again (e.g. after a reconnect).
            self.reauth_required.remove(&account_id);
        }
        if state == SyncState::Idle {
            self.load_folders(account_id);
        }
    }

    /// Whether this account is Gmail (offers Reconnect).
    pub fn is_gmail(&self, account_id: &str) -> bool {
        self.accounts
            .iter()
            .any(|a| a.id == account_id && a.kind == AccountKind::Gmail)
    }

    /// Remote images are allowed for this message (globally or by consent).
    pub fn remote_images_allowed(&self, id: i64) -> bool {
        self.settings.always_download_remote_images || self.remote_allowed.contains(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_selection_prefers_the_following_message() {
        assert_eq!(next_selection_id(&[1, 2, 3], 2), Some(3));
        assert_eq!(next_selection_id(&[1, 2, 3], 1), Some(2));
    }

    #[test]
    fn next_selection_falls_back_to_the_previous_message() {
        assert_eq!(next_selection_id(&[1, 2, 3], 3), Some(2));
    }

    #[test]
    fn next_selection_is_none_for_the_only_or_a_missing_message() {
        assert_eq!(next_selection_id(&[7], 7), None);
        assert_eq!(next_selection_id(&[1, 2], 99), None);
        assert_eq!(next_selection_id(&[], 1), None);
    }

    #[test]
    fn add_account_form_tracks_hand_edited_host_fields() {
        let mut form = crate::ui::AddAccountForm::new();
        assert!(!form.host_fields_touched());
        form.imap_host.replace_text("imap.example.com");
        assert!(form.host_fields_touched());
        form.autofilled = Some(form.host_fields());
        assert!(!form.host_fields_touched());
        form.smtp_port.replace_text("465");
        assert!(form.host_fields_touched());
    }
}
