//! State transitions: a port of the Svelte `MailStore`.
//!
//! Every backend call goes through [`Ui::spawn`]; its result lands on the UI
//! thread, where a guard checks the view it was fetched for is still current
//! before applying it. Flag, read, move, archive and delete are optimistic and
//! roll back (or refetch) on failure.

use rune::{TextEdit, Tx};

use super::compose::ComposeSeed;
use super::{
    html, AddTab, Filter, Modal, PendingFlagged, PendingSeen, SettingsState, Ui, UpdateBusy,
    PAGE_LIMIT,
};
use crate::accounts::AccountKind;
use crate::error::AppError;
use crate::events::AppEvent;
use crate::settings::Settings;
use crate::sync::SyncState;
use crate::update::{UpdateInfo, UpdateMethod, UpdateMode};
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

    fn next_list_gen(&mut self) -> u64 {
        self.list_gen = self.list_gen.wrapping_add(1);
        self.list_gen
    }

    fn bump_folders_gen(&mut self, account_id: &str) -> u64 {
        let entry = self.folders_gen.entry(account_id.to_string()).or_insert(0);
        *entry = entry.wrapping_add(1);
        *entry
    }

    /// True while any optimistic read-state or removal action is still in flight
    /// for `account_id`, so authoritative folder-count reloads wait until they settle.
    fn has_pending_folder_ops(&self, account_id: &str) -> bool {
        self.pending_seen
            .values()
            .any(|p| p.account_id == account_id)
            || self.pending_removals.values().any(|a| a == account_id)
    }

    /// Reload an account's folders now, or defer until its pending optimistic
    /// unread/removal operations finish so badges never bounce back.
    fn request_load_folders(&mut self, account_id: String) {
        if self.has_pending_folder_ops(&account_id) {
            self.pending_folder_reload.insert(account_id);
        } else {
            self.load_folders(account_id);
        }
    }

    fn flush_pending_folder_reload(&mut self, account_id: &str) {
        if !self.has_pending_folder_ops(account_id) && self.pending_folder_reload.remove(account_id)
        {
            self.load_folders(account_id.to_string());
        }
    }

    /// Filter out pending removals and overlay in-flight `\Seen`/`\Flagged`
    /// transitions onto rows loaded from the DB (or pushed live).
    pub(crate) fn apply_pending_to_page(&self, page: Vec<MessageSummary>) -> Vec<MessageSummary> {
        page.into_iter()
            .filter(|m| !self.pending_removals.contains_key(&m.id))
            .map(|mut m| {
                if let Some(p) = self.pending_seen.get(&m.id) {
                    m.seen = p.desired;
                }
                if let Some(p) = self.pending_flagged.get(&m.id) {
                    m.flagged = p.desired;
                }
                m
            })
            .collect()
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
                    if ui.has_pending_folder_ops(&id) && ui.folders.contains_key(&id) {
                        ui.pending_folder_reload.insert(id);
                    } else {
                        ui.folders.insert(id, list);
                    }
                }
                ui.folders
                    .retain(|id, _| accounts.iter().any(|a| &a.id == id));
                // An account removed from outside the UI (Postmaster) takes its view with it,
                // as Remove in Settings does.
                ui.sync_states
                    .retain(|id, _| accounts.iter().any(|a| &a.id == id));
                ui.reauth_required
                    .retain(|id| accounts.iter().any(|a| &a.id == id));
                ui.pending_seen
                    .retain(|_, p| accounts.iter().any(|a| a.id == p.account_id));
                ui.pending_removals
                    .retain(|_, acc| accounts.iter().any(|a| &a.id == acc));
                ui.pending_folder_reload
                    .retain(|id| accounts.iter().any(|a| &a.id == id));
                ui.folders_gen
                    .retain(|id, _| accounts.iter().any(|a| &a.id == id));
                let gone = |id: &str| !accounts.iter().any(|a| a.id == id);
                let showing_gone = ui.selected_account.as_deref().is_some_and(gone)
                    || ui.messages.iter().any(|m| gone(&m.account_id));
                if showing_gone {
                    ui.selected_account = None;
                    ui.selected_folder = None;
                    ui.unified = true;
                    ui.clear_reader();
                    ui.messages.clear();
                }
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
        let gen = self.bump_folders_gen(&account_id);
        self.spawn(
            move |b| async move {
                let folders = b.list_folders(account_id.clone()).unwrap_or_default();
                (account_id, folders)
            },
            move |ui, (account_id, folders), _| {
                if ui.folders_gen.get(&account_id).copied() != Some(gen) {
                    return;
                }
                if !ui.accounts.iter().any(|a| a.id == account_id) {
                    return;
                }
                if ui.has_pending_folder_ops(&account_id) && ui.folders.contains_key(&account_id) {
                    ui.pending_folder_reload.insert(account_id);
                    return;
                }
                ui.folders.insert(account_id, folders);
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
            imap_tls: form.imap_tls,
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
                            form.imap_tls = cfg.imap_tls;
                            form.smtp_host.replace_text(cfg.smtp_host.clone());
                            form.smtp_port.replace_text(cfg.smtp_port.to_string());
                            if !cfg.username.is_empty() {
                                form.username.replace_text(cfg.username.clone());
                            }
                            form.autofilled = Some(form.host_snapshot());
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

    /// Switch the add-account IMAP security mode (SSL/TLS vs STARTTLS).
    ///
    /// Swaps the standard default ports (`993` ↔ `143`) when the port still
    /// holds the other mode's default, and leaves custom ports (e.g. `1143`,
    /// `3143`) untouched.
    pub fn set_add_imap_tls(&mut self, tls: crate::accounts::ImapTls) {
        let Some(Modal::AddAccount(form)) = &mut self.modal else {
            return;
        };
        if form.busy || form.imap_tls == tls {
            return;
        }
        let prev_default = form.imap_tls.default_port().to_string();
        form.imap_tls = tls;
        if form.imap_port.text.trim() == prev_default {
            form.imap_port.replace_text(tls.default_port().to_string());
        }
    }

    /// Mirror well-known IMAP ports (`143` / `993`) into the security toggle
    /// while leaving custom ports under manual control.
    pub fn imap_port_edited(&mut self) {
        let Some(Modal::AddAccount(form)) = &mut self.modal else {
            return;
        };
        match form.imap_port.text.trim() {
            "143" => form.imap_tls = crate::accounts::ImapTls::Starttls,
            "993" => form.imap_tls = crate::accounts::ImapTls::Implicit,
            _ => {}
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

    /// The settings dialog's "Edit name": edit an account's display name in place.
    pub fn start_rename(&mut self, account_id: &str) {
        let Some(acc) = self.accounts.iter().find(|a| a.id == account_id) else {
            return;
        };
        let mut edit = TextEdit::new(acc.display_name.clone());
        edit.focused = true;
        if let Some(Modal::Settings(st)) = &mut self.modal {
            st.pending_removal = None;
            st.renaming = Some(Box::new((account_id.to_string(), edit)));
        }
    }

    pub fn cancel_rename(&mut self) {
        if let Some(Modal::Settings(st)) = &mut self.modal {
            st.renaming = None;
        }
    }

    /// Save the edited display name: shown at once, rolled back if it can't be stored.
    pub fn save_rename(&mut self) {
        let Some(Modal::Settings(st)) = &mut self.modal else {
            return;
        };
        let Some((account_id, edit)) = st.renaming.take().map(|r| *r) else {
            return;
        };
        let name = edit.text.trim().to_string();
        let Some(acc) = self.accounts.iter_mut().find(|a| a.id == account_id) else {
            return;
        };
        if acc.display_name == name {
            return;
        }
        let prev = std::mem::replace(&mut acc.display_name, name.clone());
        self.spawn(
            move |b| async move {
                let r = b.rename_account(account_id.clone(), name);
                (account_id, r)
            },
            move |ui, (account_id, result), _| {
                if let Err(err) = result {
                    if let Some(acc) = ui.accounts.iter_mut().find(|a| a.id == account_id) {
                        acc.display_name = prev;
                    }
                    ui.error("Failed to rename account", err);
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
                    ui.pending_seen.retain(|_, p| p.account_id != account_id);
                    ui.pending_removals.retain(|_, acc| acc != &account_id);
                    ui.pending_folder_reload.remove(&account_id);
                    ui.folders_gen.remove(&account_id);
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
        self.reset_mail_layout();
        self.body_images.clear();
        self.shipments.clear();
        self.loading_body = false;
    }

    fn reset_list(&mut self) {
        self.clear_reader();
        self.next_list_gen();
        self.view_gen = self.view_gen.wrapping_add(1);
        self.offset = 0;
        self.messages.clear();
        self.has_more = false;
        self.loading_older = false;
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
        let list_gen = if append {
            self.list_gen
        } else {
            self.next_list_gen()
        };
        let view_gen = self.view_gen;
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
                let stale = if append {
                    ui.view_gen != view_gen
                } else {
                    ui.list_gen != list_gen
                };
                if ui.list_key().as_ref() != Some(&key) || stale {
                    return; // the view changed or a newer load superseded this one
                }
                ui.loading_messages = false;
                match result {
                    Ok(page) => {
                        let empty_append = append && page.is_empty();
                        ui.has_more = page.len() as i64 == PAGE_LIMIT;
                        let page = ui.apply_pending_to_page(page);
                        if append {
                            let known: std::collections::HashSet<i64> =
                                ui.messages.iter().map(|m| m.id).collect();
                            ui.messages
                                .extend(page.into_iter().filter(|m| !known.contains(&m.id)));
                        } else {
                            ui.messages = page;
                        }
                        if empty_append && ui.jump_pages_left == 0 && ui.has_server_history() {
                            // Local cache ended cleanly on a PAGE_LIMIT boundary;
                            // roll straight into a server backfill so one click
                            // always loads older mail.
                            ui.offset = ui.messages.len() as i64;
                            ui.load_older_from_server();
                        } else {
                            ui.continue_jump(tx);
                        }
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

    /// Whether the currently viewed folder (or any inbox in unified mode) still
    /// has older uncached envelopes on the IMAP server.
    pub fn has_server_history(&self) -> bool {
        if self.search_active {
            return false;
        }
        if self.unified {
            self.folders
                .values()
                .flatten()
                .any(|f| f.role == "inbox" && !f.history_complete)
        } else if let Some(fid) = self.selected_folder {
            self.folder(fid).is_some_and(|f| !f.history_complete)
        } else {
            false
        }
    }

    /// Approximate count of older uncached envelopes still on the server for
    /// the active view (0 when unknown or complete).
    pub fn remaining_server_count(&self) -> i64 {
        if self.search_active {
            return 0;
        }
        let rem = |f: &Folder| {
            if f.history_complete {
                0
            } else {
                (f.total_count - f.cached_count).max(0)
            }
        };
        if self.unified {
            self.folders
                .values()
                .flatten()
                .filter(|f| f.role == "inbox")
                .map(rem)
                .sum()
        } else if let Some(fid) = self.selected_folder {
            self.folder(fid).map(rem).unwrap_or(0)
        } else {
            0
        }
    }

    /// Whether either local SQLite pages or older server history remain.
    pub fn can_load_more(&self) -> bool {
        self.has_more || self.has_server_history()
    }

    pub fn load_more(&mut self) {
        if self.loading_messages || self.loading_older {
            return;
        }
        if self.has_more {
            self.offset = self.messages.len() as i64;
            self.fetch_page(true);
        } else if self.has_server_history() {
            self.load_older_from_server();
        }
    }

    /// Fetch the next batch of older envelopes from the IMAP server for the
    /// current folder (or all incomplete inboxes in unified mode).
    pub fn load_older_from_server(&mut self) {
        if self.loading_older || self.loading_messages || !self.has_server_history() {
            return;
        }
        let Some(key) = self.list_key() else {
            return;
        };
        if matches!(key, ListKey::Search { .. }) {
            return;
        }
        let accounts_to_reload: Vec<String> = match &key {
            ListKey::Unified => self
                .folders
                .values()
                .flatten()
                .filter(|f| f.role == "inbox" && !f.history_complete)
                .map(|f| f.account_id.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect(),
            ListKey::Folder(id) => self.account_for_folder(*id).into_iter().collect(),
            ListKey::Search { .. } => Vec::new(),
        };
        let target_limit = (self.messages.len() as i64).max(PAGE_LIMIT) + PAGE_LIMIT;
        let view_gen = self.view_gen;
        let asked = key.clone();
        self.loading_older = true;
        self.spawn(
            move |b| async move {
                let batch = crate::sync::BACKFILL_BATCH_LIMIT;
                match asked {
                    ListKey::Unified => b.load_older_unified_messages(batch).await,
                    ListKey::Folder(id) => b.load_older_messages(id, batch).await,
                    ListKey::Search { .. } => Ok(0),
                }
            },
            move |ui, result, _| {
                for acc in accounts_to_reload {
                    ui.request_load_folders(acc);
                }
                let same_view = ui.view_gen == view_gen && ui.list_key().as_ref() == Some(&key);
                if same_view {
                    ui.loading_older = false;
                }
                match result {
                    Ok(added) => {
                        if same_view && added > 0 {
                            // Refetch `[0..target_limit]` rather than appending at an
                            // offset so Unified Inbox interleaves newly backfilled mail
                            // in exact chronological order, and bumping `list_gen` drops
                            // any competing `MessagesUpdated` refresh.
                            ui.refetch_limit(target_limit);
                        }
                    }
                    Err(err) => {
                        ui.error("Failed to load older messages", err);
                    }
                }
            },
        );
    }

    /// `G`: select the last loaded message, then keep loading *cached* local
    /// pages (capped) until the bottom of locally cached mail is reached. Never
    /// triggers server backfill automatically.
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
        let limit = PAGE_LIMIT.max(self.messages.len() as i64);
        self.refetch_limit(limit);
    }

    fn refetch_limit(&mut self, limit: i64) {
        let Some(key) = self.list_key() else { return };
        if matches!(key, ListKey::Search { .. }) {
            return;
        }
        self.loading_messages = true;
        let list_gen = self.next_list_gen();
        let asked = key.clone();
        self.spawn(
            move |b| async move {
                match asked {
                    ListKey::Unified => b.list_unified_messages(0, limit),
                    ListKey::Folder(id) => b.list_messages(id, 0, limit),
                    ListKey::Search { .. } => Ok(Vec::new()),
                }
            },
            move |ui, result, tx| {
                if ui.list_key().as_ref() != Some(&key) || ui.list_gen != list_gen {
                    return;
                }
                ui.loading_messages = false;
                if let Ok(page) = result {
                    ui.has_more = page.len() as i64 == limit;
                    ui.messages = ui.apply_pending_to_page(page);
                    ui.offset = ui.messages.len() as i64;
                    ui.continue_jump(tx);
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
        self.reset_mail_layout();
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
                        ui.reset_mail_layout();
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

    /// Set the read state optimistically, coalescing rapid toggles per message
    /// and rolling back on failure.
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

        if self.backend.is_none() {
            return;
        }
        // Drop any folder reload already in flight for this account so it
        // cannot land with the pre-click unread count; re-sync once settled.
        self.bump_folders_gen(&msg.account_id);
        self.pending_folder_reload.insert(msg.account_id.clone());

        if let Some(entry) = self.pending_seen.get_mut(&id) {
            entry.desired = seen;
            return;
        }
        self.pending_seen.insert(
            id,
            PendingSeen {
                account_id: msg.account_id,
                folder_id: msg.folder_id,
                confirmed: msg.seen,
                in_flight: seen,
                desired: seen,
            },
        );
        self.dispatch_mark_read(id, seen);
    }

    fn dispatch_mark_read(&mut self, id: i64, seen: bool) {
        self.spawn(
            move |b| async move { b.mark_read(id, seen).await },
            move |ui, result, _| ui.finish_mark_read(id, seen, result),
        );
    }

    pub(crate) fn finish_mark_read(
        &mut self,
        id: i64,
        sent: bool,
        result: crate::error::AppResult<()>,
    ) {
        let Some(entry) = self.pending_seen.get_mut(&id) else {
            return;
        };
        match result {
            Ok(()) => {
                entry.confirmed = sent;
                if entry.desired != sent {
                    let next = entry.desired;
                    entry.in_flight = next;
                    self.dispatch_mark_read(id, next);
                } else {
                    let account_id = entry.account_id.clone();
                    self.pending_seen.remove(&id);
                    self.flush_pending_folder_reload(&account_id);
                }
            }
            Err(err) => {
                let Some(entry) = self.pending_seen.remove(&id) else {
                    return;
                };
                // Skip rolling back a message the user already archived/deleted;
                // the removal path reconciles folder counts when it settles.
                if !self.pending_removals.contains_key(&id) && entry.desired != entry.confirmed {
                    self.patch_message(id, |m| m.seen = entry.confirmed);
                    let rollback_delta = if entry.confirmed { -1 } else { 1 };
                    self.bump_folder(entry.folder_id, 0, rollback_delta);
                    self.error("Failed to update flag", err);
                }
                self.flush_pending_folder_reload(&entry.account_id);
            }
        }
    }

    /// Toggle the flag optimistically, coalescing rapid toggles per message.
    pub fn toggle_flagged(&mut self, id: i64) {
        let Some(was_flagged) = self.messages.iter().find(|m| m.id == id).map(|m| m.flagged) else {
            return;
        };
        let flagged = !was_flagged;
        self.patch_message(id, |m| m.flagged = flagged);

        if self.backend.is_none() {
            return;
        }
        if let Some(entry) = self.pending_flagged.get_mut(&id) {
            entry.desired = flagged;
            return;
        }
        self.pending_flagged.insert(
            id,
            PendingFlagged {
                confirmed: was_flagged,
                in_flight: flagged,
                desired: flagged,
            },
        );
        self.dispatch_mark_flagged(id, flagged);
    }

    fn dispatch_mark_flagged(&mut self, id: i64, flagged: bool) {
        self.spawn(
            move |b| async move { b.mark_flagged(id, flagged).await },
            move |ui, result, _| ui.finish_mark_flagged(id, flagged, result),
        );
    }

    pub(crate) fn finish_mark_flagged(
        &mut self,
        id: i64,
        sent: bool,
        result: crate::error::AppResult<()>,
    ) {
        let Some(entry) = self.pending_flagged.get_mut(&id) else {
            return;
        };
        match result {
            Ok(()) => {
                entry.confirmed = sent;
                if entry.desired != sent {
                    let next = entry.desired;
                    entry.in_flight = next;
                    self.dispatch_mark_flagged(id, next);
                } else {
                    self.pending_flagged.remove(&id);
                }
            }
            Err(err) => {
                let Some(entry) = self.pending_flagged.remove(&id) else {
                    return;
                };
                if !self.pending_removals.contains_key(&id) && entry.desired != entry.confirmed {
                    self.patch_message(id, |m| m.flagged = entry.confirmed);
                    self.error("Failed to update flag", err);
                }
            }
        }
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
        let account_id = msg.account_id.clone();
        self.pending_removals.insert(id, account_id.clone());
        let unread = if msg.seen { 0 } else { 1 };
        self.bump_folder(msg.folder_id, -1, -unread);
        if let Some(target) = target_folder_id {
            self.bump_folder(target, 1, unread);
        }
        if self.backend.is_some() {
            self.bump_folders_gen(&account_id);
            self.pending_folder_reload.insert(account_id.clone());
        }
        if was_selected {
            self.clear_reader();
            if let Some(next) = next {
                self.select_message(next);
                self.reveal = Some(format!("msg-{next}"));
            }
        }

        self.spawn(
            move |b| action(b, id),
            move |ui, result, _| {
                ui.pending_removals.remove(&id);
                match result {
                    Ok(()) => {
                        ui.pending_seen.remove(&id);
                        ui.pending_flagged.remove(&id);
                    }
                    Err(err) => {
                        ui.error(error_prefix, err);
                        ui.refresh_current_page();
                        ui.pending_folder_reload.insert(account_id.clone());
                    }
                }
                ui.flush_pending_folder_reload(&account_id);
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

    /// Show a sample notification, so the user can check it appears (and sounds).
    pub fn test_notification(&self) {
        self.spawn(
            move |b| async move { b.test_notification() },
            move |ui, result, _| {
                if let Err(err) = result {
                    ui.error("Failed to send a test notification", err);
                }
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

    // ---- Updates ----------------------------------------------------------

    /// Settings → Check now.
    pub fn check_for_update(&mut self) {
        if self.update_busy.is_some() || self.backend.is_none() {
            return;
        }
        self.update_busy = Some(UpdateBusy::Checking);
        self.spawn(
            |b| async move { b.check_for_update().await },
            |ui, result, _| {
                ui.update_busy = None;
                match result {
                    Ok(found) => {
                        ui.update = found;
                        ui.update_checked = true;
                    }
                    Err(err) => ui.error("Update check failed", err),
                }
            },
        );
    }

    /// Settings → Install and restart, or Open download page, as the method allows.
    pub fn install_update(&mut self) {
        let Some(info) = self.update.clone() else {
            return;
        };
        if !info.method.installs() {
            if info.method == UpdateMethod::Download {
                self.open_link(&info.release_url);
            }
            return;
        }
        if self.update_busy.is_some() || self.backend.is_none() {
            return;
        }
        self.update_busy = Some(UpdateBusy::Installing);
        self.spawn(
            |b| async move {
                let info = b.install_update().await?;
                b.restart_to_update(false)?;
                Ok::<_, AppError>(info)
            },
            |ui, result, tx| {
                ui.update_busy = None;
                match result {
                    Ok(info) => {
                        ui.update = Some(info);
                        // The update installs as the app exits, then starts the new one.
                        tx.quit();
                    }
                    Err(err) => ui.error("Update failed", err),
                }
            },
        );
    }

    /// Install a prepared update now and come back in the background (the window is closed).
    fn restart_in_background_for_update(&mut self) {
        self.spawn(
            |b| async move { b.restart_to_update(true) },
            |ui, result, tx| match result {
                Ok(()) => tx.quit(),
                Err(err) => ui.error("Update failed", err),
            },
        );
    }

    pub fn set_update_mode(&mut self, mode: UpdateMode) {
        let mut next = self.settings.clone();
        next.updates = mode;
        self.update_settings(next);
    }

    fn on_update_available(&mut self, info: UpdateInfo) {
        let seen = self
            .update
            .as_ref()
            .is_some_and(|u| u.version == info.version && u.ready == info.ready);
        let auto_ready = info.ready && self.settings.updates == UpdateMode::Auto;
        let version = info.version.clone();
        self.update = Some(info);
        let window_closed = self.window.as_ref().is_some_and(|w| !w.is_visible());
        if auto_ready && window_closed {
            self.restart_in_background_for_update();
            return;
        }
        if seen {
            return;
        }
        if auto_ready {
            self.toast(
                format!("Cosmic Mail {version} is ready. It installs when you close the window."),
                false,
            );
        } else {
            self.toast(
                format!("Cosmic Mail {version} is available. See Settings → Updates."),
                false,
            );
        }
    }

    /// The window is closing: an automatic update that is ready installs now.
    pub fn on_window_close(&mut self) {
        let ready = self.update.as_ref().is_some_and(|u| u.ready);
        if ready && self.settings.updates == UpdateMode::Auto {
            self.restart_in_background_for_update();
        }
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
            AppEvent::AccountsChanged => self.load_accounts(),
            AppEvent::UpdateAvailable(info) => self.on_update_available(info),
        }
    }

    fn prepend_fresh(&mut self, messages: Vec<MessageSummary>) {
        let known: std::collections::HashSet<i64> = self.messages.iter().map(|m| m.id).collect();
        let mut fresh: Vec<MessageSummary> = self
            .apply_pending_to_page(messages)
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
        let gen = self.bump_folders_gen(&account_id);
        self.spawn(
            move |b| async move {
                let folders = b.list_folders(account_id.clone()).unwrap_or_default();
                (account_id, folders)
            },
            move |ui, (account_id, folders), _| {
                if ui.folders_gen.get(&account_id).copied() == Some(gen)
                    && ui.accounts.iter().any(|a| a.id == account_id)
                {
                    if ui.has_pending_folder_ops(&account_id)
                        && ui.folders.contains_key(&account_id)
                    {
                        ui.pending_folder_reload.insert(account_id);
                    } else {
                        ui.folders.insert(account_id, folders);
                    }
                }
                if ui.unified && !ui.search_active {
                    ui.refresh_current_page();
                }
            },
        );
    }

    fn on_messages_updated(&mut self, folder_id: i64) {
        if !self.search_active && !self.loading_older {
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
            self.request_load_folders(account_id);
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
            self.request_load_folders(account_id);
        }
    }

    /// Whether this account is Gmail (offers Reconnect).
    /// The account's colour slot (`Palette::slot`): its place in the account list, so
    /// the first accounts get clearly different colours.
    pub fn account_slot(&self, account_id: &str) -> usize {
        self.accounts
            .iter()
            .position(|a| a.id == account_id)
            .unwrap_or(0)
    }

    pub fn set_account_marker(&mut self, marker: crate::settings::AccountMarker) {
        let mut next = self.settings.clone();
        next.account_marker = marker;
        self.update_settings(next);
    }

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
        form.autofilled = Some(form.host_snapshot());
        assert!(!form.host_fields_touched());
        form.smtp_port.replace_text("465");
        assert!(form.host_fields_touched());
        form.autofilled = Some(form.host_snapshot());
        assert!(!form.host_fields_touched());
        form.imap_tls = crate::accounts::ImapTls::Starttls;
        assert!(form.host_fields_touched());
    }

    struct DbFixture {
        ui: Ui,
        db_path: std::path::PathBuf,
        _dir: tempfile::TempDir,
        _rt: tokio::runtime::Runtime,
        folder_id: i64,
        msg_ids: Vec<i64>,
    }

    impl DbFixture {
        fn new(unread_count: usize) -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let db_path = dir.path().join("mail.db");
            let conn = rusqlite::Connection::open(&db_path).expect("open db");
            crate::store::init_schema(&conn).expect("init schema");
            let (folder_id, _) = crate::store::upsert_folder(
                &conn,
                "a1",
                "INBOX",
                crate::store::FolderRole::Inbox,
                1,
            )
            .expect("folder");
            crate::store::set_folder_counts(
                &conn,
                folder_id,
                unread_count as u32,
                unread_count as u32,
            )
            .expect("counts");
            let mut msg_ids = Vec::new();
            for i in 0..unread_count {
                let uid = (i + 1) as u32;
                let id = crate::store::upsert_message(
                    &conn,
                    &crate::store::MessageUpsert {
                        folder_id,
                        uid,
                        message_id: Some(format!("<m{uid}@example.com>")),
                        subject: format!("Message {uid}"),
                        from_name: "Sender".into(),
                        from_addr: "sender@example.com".into(),
                        to_addrs: vec!["me@example.com".into()],
                        cc_addrs: Vec::new(),
                        date: format!("2026-11-27T10:0{i}:00Z"),
                        snippet: "Hello".into(),
                        seen: false,
                        flagged: false,
                        has_attachments: false,
                        rfc822_size: 128,
                    },
                )
                .expect("upsert msg");
                msg_ids.push(id);
            }

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio");
            let inbox = crate::ui::Inbox::default();
            let events = {
                let inbox = inbox.clone();
                crate::events::Events::new(move |ev| inbox.event(ev))
            };
            let backend = crate::backend::Backend::new(conn, events, rt.handle().clone());
            let mut ui = Ui::new(Some(backend.clone()), inbox);
            ui.accounts = vec![crate::accounts::AccountPublic {
                id: "a1".into(),
                email: "me@example.com".into(),
                display_name: "Me".into(),
                kind: AccountKind::Imap,
            }];
            ui.folders.insert(
                "a1".into(),
                backend.list_folders("a1".into()).expect("folders"),
            );
            ui.messages = backend
                .list_unified_messages(0, PAGE_LIMIT)
                .expect("messages");
            ui.loading_accounts = false;
            Self {
                ui,
                db_path,
                _dir: dir,
                _rt: rt,
                folder_id,
                msg_ids,
            }
        }

        fn conn(&self) -> rusqlite::Connection {
            rusqlite::Connection::open(&self.db_path).expect("reopen db")
        }

        fn drain(&mut self) {
            let mut tx = rune::Tx::default();
            for _ in 0..8 {
                self._rt.block_on(async {
                    tokio::task::yield_now().await;
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                });
                let jobs = self.ui.inbox.take();
                if jobs.is_empty() {
                    continue;
                }
                for job in jobs {
                    job(&mut self.ui, &mut tx);
                }
            }
        }

        fn seen(&self, id: i64) -> bool {
            self.ui
                .messages
                .iter()
                .find(|m| m.id == id)
                .map(|m| m.seen)
                .expect("message present")
        }

        fn flagged(&self, id: i64) -> bool {
            self.ui
                .messages
                .iter()
                .find(|m| m.id == id)
                .map(|m| m.flagged)
                .expect("message present")
        }

        /// Simulate an optimistic `set_seen` without touching IMAP over the network.
        fn begin_seen(&mut self, id: i64, seen: bool) {
            let msg = self
                .ui
                .messages
                .iter()
                .find(|m| m.id == id)
                .cloned()
                .expect("message");
            assert_ne!(msg.seen, seen);
            let delta = if seen { -1 } else { 1 };
            self.ui.patch_message(id, |m| m.seen = seen);
            self.ui.bump_folder(msg.folder_id, 0, delta);
            self.ui.bump_folders_gen(&msg.account_id);
            self.ui.pending_folder_reload.insert(msg.account_id.clone());
            if let Some(entry) = self.ui.pending_seen.get_mut(&id) {
                entry.desired = seen;
            } else {
                self.ui.pending_seen.insert(
                    id,
                    PendingSeen {
                        account_id: msg.account_id,
                        folder_id: msg.folder_id,
                        confirmed: msg.seen,
                        in_flight: seen,
                        desired: seen,
                    },
                );
            }
        }
    }

    #[test]
    fn rapid_click_through_keeps_unread_indicators_and_badges_steady() {
        let mut fx = DbFixture::new(3);
        let [a, b, c] = [fx.msg_ids[0], fx.msg_ids[1], fx.msg_ids[2]];

        // User rapidly clicks through all three unread messages.
        fx.begin_seen(a, true);
        fx.begin_seen(b, true);
        fx.begin_seen(c, true);
        assert!(fx.seen(a) && fx.seen(b) && fx.seen(c));
        assert_eq!(fx.ui.unified_unread(), 0);

        // Backend commits only A and emits MessagesUpdated while B and C are still unread in DB.
        {
            let conn = fx.conn();
            crate::store::mark_seen(&conn, a, true).unwrap();
            crate::store::adjust_folder_unread_count(&conn, fx.folder_id, -1).unwrap();
        }
        let mut tx = rune::Tx::default();
        fx.ui.on_event(
            AppEvent::MessagesUpdated {
                folder_id: fx.folder_id,
            },
            &mut tx,
        );
        fx.drain();
        // B and C must not flash back to unread, and the unread badge must stay at 0.
        assert!(fx.seen(a) && fx.seen(b) && fx.seen(c));
        assert_eq!(fx.ui.unified_unread(), 0);

        // A's mark_read finishes; B and C are still pending.
        fx.ui.finish_mark_read(a, true, Ok(()));
        fx.drain();
        assert!(fx.seen(a) && fx.seen(b) && fx.seen(c));
        assert_eq!(fx.ui.unified_unread(), 0);

        // B commits and an IDLE SyncState::Idle arrives while C is still pending.
        {
            let conn = fx.conn();
            crate::store::mark_seen(&conn, b, true).unwrap();
            crate::store::adjust_folder_unread_count(&conn, fx.folder_id, -1).unwrap();
        }
        fx.ui.on_event(
            AppEvent::MessagesUpdated {
                folder_id: fx.folder_id,
            },
            &mut tx,
        );
        fx.ui.on_event(
            AppEvent::SyncState {
                account_id: "a1".into(),
                state: SyncState::Idle,
                error: None,
                needs_reauth: false,
            },
            &mut tx,
        );
        fx.ui.finish_mark_read(b, true, Ok(()));
        fx.drain();
        assert!(fx.seen(a) && fx.seen(b) && fx.seen(c));
        assert_eq!(fx.ui.unified_unread(), 0);

        // C commits and finishes: pending_seen clears and the deferred folder reload reconciles.
        {
            let conn = fx.conn();
            crate::store::mark_seen(&conn, c, true).unwrap();
            crate::store::adjust_folder_unread_count(&conn, fx.folder_id, -1).unwrap();
        }
        fx.ui.on_event(
            AppEvent::MessagesUpdated {
                folder_id: fx.folder_id,
            },
            &mut tx,
        );
        fx.ui.finish_mark_read(c, true, Ok(()));
        fx.drain();
        assert!(fx.ui.pending_seen.is_empty());
        assert!(fx.ui.pending_folder_reload.is_empty());
        assert!(fx.seen(a) && fx.seen(b) && fx.seen(c));
        assert_eq!(fx.ui.unified_unread(), 0);
    }

    #[test]
    fn rapid_toggle_on_same_message_coalesces_and_survives_refetch() {
        let mut fx = DbFixture::new(1);
        let a = fx.msg_ids[0];

        // Click to read, then immediately press `u` to mark unread while the first RPC is in flight.
        fx.begin_seen(a, true);
        assert!(fx.seen(a));
        assert_eq!(fx.ui.unified_unread(), 0);

        fx.begin_seen(a, false);
        assert!(!fx.seen(a));
        assert_eq!(fx.ui.unified_unread(), 1);
        assert_eq!(
            fx.ui.pending_seen.get(&a).map(|p| (p.in_flight, p.desired)),
            Some((true, false))
        );

        // First RPC (`seen = true`) commits to DB and emits MessagesUpdated.
        {
            let conn = fx.conn();
            crate::store::mark_seen(&conn, a, true).unwrap();
            crate::store::adjust_folder_unread_count(&conn, fx.folder_id, -1).unwrap();
        }
        let mut tx = rune::Tx::default();
        fx.ui.on_event(
            AppEvent::MessagesUpdated {
                folder_id: fx.folder_id,
            },
            &mut tx,
        );
        fx.drain();
        // UI must stay unread despite DB momentarily having seen = true.
        assert!(!fx.seen(a));
        assert_eq!(fx.ui.unified_unread(), 1);

        // When the first RPC finishes, pending_seen advances confirmed=true and
        // queues the follow-up (`in_flight = false`), keeping the entry active.
        // Disable backend before calling finish_mark_read so the follow-up doesn't dial IMAP.
        fx.ui.backend = None;
        fx.ui.finish_mark_read(a, true, Ok(()));
        assert_eq!(
            fx.ui
                .pending_seen
                .get(&a)
                .map(|p| (p.confirmed, p.in_flight, p.desired)),
            Some((true, false, false))
        );

        // Follow-up finishes cleanly.
        fx.ui.finish_mark_read(a, false, Ok(()));
        assert!(fx.ui.pending_seen.is_empty());
        assert!(!fx.seen(a));
        assert_eq!(fx.ui.unified_unread(), 1);
    }

    #[test]
    fn failed_mark_read_rolls_back_only_when_still_diverged_and_not_removed() {
        let mut fx = DbFixture::new(3);
        let [a, b, c] = [fx.msg_ids[0], fx.msg_ids[1], fx.msg_ids[2]];
        fx.ui.backend = None;

        // Case 1: straight failure rolls back seen + unread count and toasts.
        fx.begin_seen(a, true);
        assert_eq!(fx.ui.unified_unread(), 2);
        fx.ui
            .finish_mark_read(a, true, Err(AppError::msg("offline")));
        assert!(!fx.seen(a));
        assert_eq!(fx.ui.unified_unread(), 3);
        assert_eq!(fx.ui.toasts.len(), 1);

        // Case 2: user toggled back to unread before the first RPC failed — no rollback/toast.
        fx.begin_seen(b, true);
        fx.begin_seen(b, false);
        assert_eq!(fx.ui.unified_unread(), 3);
        fx.ui
            .finish_mark_read(b, true, Err(AppError::msg("offline")));
        assert!(!fx.seen(b));
        assert_eq!(fx.ui.unified_unread(), 3);
        assert_eq!(fx.ui.toasts.len(), 1);

        // Case 3: message is already being removed — skip unread bump and error toast.
        fx.begin_seen(c, true);
        fx.ui.messages.retain(|m| m.id != c);
        fx.ui.pending_removals.insert(c, "a1".into());
        assert_eq!(fx.ui.unified_unread(), 2);
        fx.ui
            .finish_mark_read(c, true, Err(AppError::msg("message not found")));
        assert_eq!(fx.ui.unified_unread(), 2);
        assert_eq!(fx.ui.toasts.len(), 1);
    }

    #[test]
    fn stale_refetch_and_folder_reload_generations_are_discarded() {
        let mut fx = DbFixture::new(1);
        let a = fx.msg_ids[0];

        // Spawn an old refresh + folder reload while A is still unread in DB.
        fx.ui.refresh_current_page();
        fx.ui.load_folders("a1".into());

        // Before those jobs run, commit A to DB and bump generations via a newer refresh.
        {
            let conn = fx.conn();
            crate::store::mark_seen(&conn, a, true).unwrap();
            crate::store::adjust_folder_unread_count(&conn, fx.folder_id, -1).unwrap();
        }
        fx.ui.patch_message(a, |m| m.seen = true);
        fx.ui.bump_folder(fx.folder_id, 0, -1);
        fx.ui.refresh_current_page();
        fx.ui.load_folders("a1".into());

        // Draining runs the stale jobs too; they must be ignored.
        fx.drain();
        assert!(fx.seen(a));
        assert_eq!(fx.ui.unified_unread(), 0);
    }

    #[test]
    fn pending_flagged_survives_refetch_and_rolls_back_on_error() {
        let mut fx = DbFixture::new(1);
        let a = fx.msg_ids[0];
        fx.ui.patch_message(a, |m| m.flagged = true);
        fx.ui.pending_flagged.insert(
            a,
            PendingFlagged {
                confirmed: false,
                in_flight: true,
                desired: true,
            },
        );

        let mut tx = rune::Tx::default();
        fx.ui.on_event(
            AppEvent::MessagesUpdated {
                folder_id: fx.folder_id,
            },
            &mut tx,
        );
        fx.drain();
        assert!(fx.flagged(a), "refetch must not clear in-flight flag");

        fx.ui
            .finish_mark_flagged(a, true, Err(AppError::msg("offline")));
        assert!(!fx.flagged(a), "failure rolls back flag");
        assert_eq!(fx.ui.toasts.len(), 1);
    }
}
