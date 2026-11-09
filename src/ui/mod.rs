//! The native Rune interface.
//!
//! [`Ui`] is the whole application state the view draws; it plays the part the
//! Svelte `MailStore` played. Backend work runs on the backend's tokio runtime
//! and lands back here as a [`Job`] through the [`Inbox`], which `poll` drains
//! on the UI thread. Backend events ([`AppEvent`]) arrive the same way.
//!
//! - `store`: state transitions (selection, paging, optimistic flag/move/delete,
//!   event handling) — a port of the old `mail.svelte.ts`.
//! - `view`: the element tree (rail, header, folders, list, reader, footer,
//!   modals, toasts) and keyboard handling.
//! - `format`, `compose`, `palette`, `pane`, `html`, `theme`: pure helpers.

pub mod compose;
pub mod demo;
pub mod format;
pub mod html;
pub mod images;
pub mod mail;
pub mod mail_doc;
pub mod palette;
pub mod pane;
pub mod store;
pub mod theme;
pub mod view;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};

use rune::{AppHandle, TextEdit, Tx, Waker};

use crate::accounts::AccountPublic;
use crate::autoconfig::DiscoveredConfig;
use crate::backend::Backend;
use crate::events::AppEvent;
use crate::settings::Settings;
use crate::sync::SyncState;
use crate::wire::{Folder, MessageBody, MessageSummary, Shipment};

use self::compose::ComposeSeed;
pub use self::images::{short_hash, BodyImage};
use self::theme::{Appearance, Palette};
pub use rune_theme_foundation::ColorScheme;

/// Messages fetched per page.
pub const PAGE_LIMIT: i64 = 50;

/// Work finished off the UI thread, applied to the state on it.
pub type Job = Box<dyn FnOnce(&mut Ui, &mut Tx) + Send>;

/// How background work lands results on the UI thread.
///
/// In the app, jobs go through [`AppHandle::update`], which runs them on the
/// UI thread even while the window is hidden. Without a handle (headless runs
/// and tests) they queue here and `poll` drains them; pushing wakes the window.
#[derive(Clone, Default)]
pub struct Inbox {
    jobs: Arc<Mutex<Vec<Job>>>,
    waker: Arc<Mutex<Option<Waker>>>,
    handle: Arc<Mutex<Option<AppHandle<Ui>>>>,
}

impl Inbox {
    /// Deliver a job to the UI thread.
    pub fn push(&self, job: Job) {
        if let Some(handle) = lock(&self.handle).as_ref() {
            handle.update(job);
            return;
        }
        lock(&self.jobs).push(job);
        if let Some(wake) = lock(&self.waker).as_ref() {
            wake();
        }
    }

    /// Whether jobs go through the app (a window runs), rather than queueing for `poll`.
    pub fn has_handle(&self) -> bool {
        lock(&self.handle).is_some()
    }

    /// Deliver jobs through the app's handle from now on.
    pub fn set_handle(&self, handle: AppHandle<Ui>) {
        *lock(&self.handle) = Some(handle);
    }

    /// Queue a backend event.
    pub fn event(&self, event: AppEvent) {
        self.push(Box::new(move |ui: &mut Ui, tx: &mut Tx| {
            ui.on_event(event, tx)
        }));
    }

    /// The waker `AppSpec::setup` hands over.
    pub fn set_waker(&self, waker: Waker) {
        *lock(&self.waker) = Some(waker);
    }

    /// Everything queued so far, oldest first.
    pub fn take(&self) -> Vec<Job> {
        std::mem::take(&mut *lock(&self.jobs))
    }
}

/// Lock a mutex, recovering the data if a panicking thread poisoned it.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A transient notice in the bottom-right corner.
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub error: bool,
    pub message: String,
    /// `None` until `poll` stamps it with the clock (then it expires 4 s / 8 s later).
    pub expires_at: Option<f64>,
}

/// The message-list filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Unread,
    Flagged,
}

/// The open modal, if any. Only one at a time.
pub enum Modal {
    AddAccount(Box<AddAccountForm>),
    Settings(SettingsState),
    Compose(Box<ComposeForm>),
    Palette(Box<PaletteState>),
}

/// Add-account dialog tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddTab {
    Imap,
    Gmail,
}

/// The add-account dialog.
pub struct AddAccountForm {
    pub tab: AddTab,
    pub busy: bool,
    pub error: Option<String>,
    pub email: TextEdit,
    pub display_name: TextEdit,
    pub imap_host: TextEdit,
    pub imap_port: TextEdit,
    pub smtp_host: TextEdit,
    pub smtp_port: TextEdit,
    pub username: TextEdit,
    pub password: TextEdit,
    pub detecting: bool,
    pub discovered: Option<DiscoveredConfig>,
    /// The address the last discovery ran for.
    pub discovered_for: Option<String>,
    /// Host/port/username exactly as discovery last filled them; fields that
    /// differ were typed by the user and are never overwritten.
    pub autofilled: Option<[String; 5]>,
    /// Whether the email field had focus last frame (blur runs discovery).
    pub email_was_focused: bool,
}

impl AddAccountForm {
    pub fn new() -> Self {
        let mut email = TextEdit::new("");
        email.focused = true;
        AddAccountForm {
            tab: AddTab::Imap,
            busy: false,
            error: None,
            email,
            display_name: TextEdit::new(""),
            imap_host: TextEdit::new(""),
            imap_port: TextEdit::new("993"),
            smtp_host: TextEdit::new(""),
            smtp_port: TextEdit::new("587"),
            username: TextEdit::new(""),
            password: TextEdit::new("").secure(),
            detecting: false,
            discovered: None,
            discovered_for: None,
            autofilled: None,
            email_was_focused: true,
        }
    }

    /// The host fields as they stand.
    pub fn host_fields(&self) -> [String; 5] {
        [
            self.imap_host.text.clone(),
            self.imap_port.text.clone(),
            self.smtp_host.text.clone(),
            self.smtp_port.text.clone(),
            self.username.text.clone(),
        ]
    }

    /// Whether the user has typed into any host/port/username field.
    pub fn host_fields_touched(&self) -> bool {
        match &self.autofilled {
            Some(filled) => *filled != self.host_fields(),
            None => {
                !self.imap_host.text.trim().is_empty()
                    || !self.smtp_host.text.trim().is_empty()
                    || !self.username.text.trim().is_empty()
            }
        }
    }
}

impl Default for AddAccountForm {
    fn default() -> Self {
        Self::new()
    }
}

/// The settings dialog's transient state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsState {
    /// Account armed for removal (a second click removes) and when it disarms.
    pub pending_removal: Option<(String, Option<f64>)>,
    /// The account whose Gmail consent is running.
    pub reauthing: Option<String>,
}

/// The compose dialog.
pub struct ComposeForm {
    pub seed: ComposeSeed,
    pub account_id: String,
    pub to: TextEdit,
    pub cc: TextEdit,
    pub bcc: TextEdit,
    pub subject: TextEdit,
    pub body: TextEdit,
    pub busy: bool,
    pub error: Option<String>,
}

impl ComposeForm {
    pub fn new(seed: ComposeSeed) -> Self {
        let mut to = TextEdit::new(seed.to_addrs.join(", "));
        let mut body = TextEdit::new(seed.body_text.clone());
        // A reply already has its recipients: start in the body, above the quote.
        if seed.reply_to_message_id.is_some() && !seed.to_addrs.is_empty() {
            body.focused = true;
            body.set_caret(0);
        } else {
            to.focused = true;
        }
        ComposeForm {
            account_id: seed.account_id.clone(),
            cc: TextEdit::new(seed.cc_addrs.join(", ")),
            bcc: TextEdit::new(""),
            subject: TextEdit::new(seed.subject.clone()),
            to,
            body,
            seed,
            busy: false,
            error: None,
        }
    }
}

/// What a palette command does when run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Unified,
    OpenAccount(String),
    GoFolder(i64),
    Compose,
    Reply { all: bool },
    ToggleRead,
    Archive,
    Delete,
    ToggleFlag,
    MovePicker,
    MoveTo { message_id: i64, folder_id: i64 },
    SyncFolder,
    SyncAccount(String),
    AddAccount,
    OpenSettings,
    FocusSearch,
}

/// One command palette entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: String,
    pub title: String,
    pub hint: Option<&'static str>,
    pub keywords: Vec<String>,
    pub action: Action,
}

impl palette::Rankable for Command {
    fn title(&self) -> &str {
        &self.title
    }
    fn keywords(&self) -> &[String] {
        &self.keywords
    }
}

/// The command palette (also the move-to-folder picker).
pub struct PaletteState {
    pub query: TextEdit,
    pub commands: Vec<Command>,
    pub placeholder: &'static str,
    /// Index into the ranked results.
    pub selected: usize,
}

impl PaletteState {
    pub fn new(commands: Vec<Command>, placeholder: &'static str) -> Self {
        let mut query = TextEdit::new("");
        query.focused = true;
        PaletteState {
            query,
            commands,
            placeholder,
            selected: 0,
        }
    }

    /// Indices of `commands` matching the query, best first.
    pub fn results(&self) -> Vec<usize> {
        palette::rank(&self.query.text, &self.commands)
    }
}

/// The whole UI state.
pub struct Ui {
    pub(crate) backend: Option<Arc<Backend>>,
    pub(crate) inbox: Inbox,
    /// Shows and focuses the window on activation (absent headless and in tests).
    pub(crate) window: Option<AppHandle<Ui>>,
    /// The app's clock at the last `poll`, in seconds.
    pub(crate) now: f64,

    /// The foundation colour scheme, and whether it is light or dark (UI preferences).
    pub scheme: ColorScheme,
    pub appearance: Appearance,

    pub accounts: Vec<AccountPublic>,
    pub folders: HashMap<String, Vec<Folder>>,
    pub sync_states: HashMap<String, SyncState>,
    /// Accounts whose Gmail credentials are dead — Reconnect (not retry) fixes them.
    pub reauth_required: HashSet<String>,
    pub loading_accounts: bool,

    /// True: the list shows every account's inbox.
    pub unified: bool,
    pub selected_account: Option<String>,
    pub selected_folder: Option<i64>,

    pub messages: Vec<MessageSummary>,
    pub offset: i64,
    pub has_more: bool,
    pub loading_messages: bool,

    pub selected_message: Option<i64>,
    /// Messages removed optimistically whose server move hasn't answered yet;
    /// a refetch issued before the move must not bring them back.
    pub(crate) pending_removals: HashSet<i64>,
    pub body: Option<MessageBody>,
    /// `body.html` rendered for the reader.
    pub body_lines: Vec<html::Line>,
    /// `body.html` laid out by Blitz (see `mail.rs`); drawn instead of `body_lines`.
    pub(crate) mail: std::cell::RefCell<mail::MailState>,
    pub loading_body: bool,
    pub shipments: Vec<Shipment>,
    /// Messages whose remote images were allowed this session.
    pub remote_allowed: HashSet<i64>,
    /// The open message's images by `src`.
    pub body_images: HashMap<String, BodyImage>,
    pub(crate) image_cache: Option<Arc<rune::image::ImageCache>>,
    /// `decoded_images()` at the last poll.
    pub(crate) decoded_seen: usize,

    pub toasts: Vec<Toast>,
    toast_seq: u64,

    pub filter: Filter,
    /// The live filter over loaded messages, and the backend search term on Enter.
    pub query: TextEdit,
    pub search_active: bool,
    pub search_query: String,

    pub settings: Settings,
    pub modal: Option<Modal>,
    /// Takes edits addressed to a dialog field after the dialog closed.
    pub(crate) scratch_edit: TextEdit,

    /// List share of the list+reader width.
    pub pane_ratio: f32,
    /// Where UI preferences (the pane split) persist; `None` keeps them in memory.
    pub(crate) prefs: Option<rune::prefs::Prefs>,
    pub dragging_divider: bool,
    /// List width at the start of a divider drag.
    pub drag_start_width: f32,

    /// Pages still to load for `G` (jump to the true bottom).
    pub(crate) jump_pages_left: u32,
    /// When the last lone `g` was pressed (for `gg`).
    pub(crate) last_g: Option<rune::time::Instant>,
    /// A row to scroll into view on the next frame.
    pub(crate) reveal: Option<String>,
    /// A window activation requested by the backend, applied on the UI thread.
    pub(crate) activate_requested: bool,
}

impl Ui {
    /// UI state over a backend (`None` renders without one: scenes and tests).
    pub fn new(backend: Option<Arc<Backend>>, inbox: Inbox) -> Self {
        Ui {
            backend,
            inbox,
            window: None,
            now: 0.0,
            scheme: ColorScheme::default(),
            appearance: Appearance::default(),
            accounts: Vec::new(),
            folders: HashMap::new(),
            sync_states: HashMap::new(),
            reauth_required: HashSet::new(),
            loading_accounts: true,
            unified: true,
            selected_account: None,
            selected_folder: None,
            messages: Vec::new(),
            offset: 0,
            has_more: false,
            loading_messages: false,
            selected_message: None,
            pending_removals: HashSet::new(),
            body: None,
            body_lines: Vec::new(),
            mail: Default::default(),
            loading_body: false,
            shipments: Vec::new(),
            remote_allowed: HashSet::new(),
            body_images: HashMap::new(),
            image_cache: None,
            decoded_seen: 0,
            toasts: Vec::new(),
            toast_seq: 0,
            filter: Filter::All,
            query: TextEdit::new(""),
            search_active: false,
            search_query: String::new(),
            settings: Settings::default(),
            modal: None,
            scratch_edit: TextEdit::new(""),
            pane_ratio: pane::DEFAULT_LIST_PANE_RATIO,
            prefs: None,
            dragging_divider: false,
            drag_start_width: 0.0,
            jump_pages_left: 0,
            last_g: None,
            reveal: None,
            activate_requested: false,
        }
    }

    /// Persist UI preferences here, restoring the saved pane split, scheme and appearance.
    pub fn set_prefs(&mut self, prefs: rune::prefs::Prefs) {
        if let Some(ratio) = prefs.get::<f32>("listPaneRatio") {
            if pane::is_valid_pane_ratio(ratio) {
                self.pane_ratio = ratio;
            }
        }
        if let Some(name) = prefs.get::<String>("colorScheme") {
            self.scheme = ColorScheme::from_name(&name);
        }
        if let Some(name) = prefs.get::<String>("appearance") {
            self.appearance = Appearance::from_name(&name);
        }
        self.prefs = Some(prefs);
    }

    /// Attach the window handle used for activation.
    pub fn set_window(&mut self, window: AppHandle<Ui>) {
        self.window = Some(window);
    }

    /// Run `work` against the backend on its runtime, then `done` with its
    /// result on the UI thread. Without a backend nothing runs.
    pub fn spawn<T, F, Fut>(&self, work: F, done: impl FnOnce(&mut Ui, T, &mut Tx) + Send + 'static)
    where
        T: Send + 'static,
        F: FnOnce(Arc<Backend>) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let inbox = self.inbox.clone();
        let runtime = backend.runtime().clone();
        runtime.spawn(async move {
            let result = work(backend).await;
            inbox.push(Box::new(move |ui: &mut Ui, tx: &mut Tx| {
                done(ui, result, tx)
            }));
        });
    }

    /// Show a toast (errors linger longer).
    pub fn toast(&mut self, message: impl Into<String>, error: bool) {
        self.toast_seq += 1;
        self.toasts.push(Toast {
            id: self.toast_seq,
            error,
            message: message.into(),
            expires_at: None,
        });
    }

    /// Show `prefix: error` as an error toast.
    pub fn error(&mut self, prefix: &str, err: impl std::fmt::Display) {
        self.toast(format!("{prefix}: {err}"), true);
    }

    pub fn dismiss_toast(&mut self, id: u64) {
        self.toasts.retain(|t| t.id != id);
    }

    /// The colours for this frame, given whether the desktop is dark.
    pub fn palette(&self, system_dark: bool) -> Palette {
        Palette::new(self.scheme, self.appearance.is_dark(system_dark))
    }

    /// Switch the colour scheme and remember it.
    pub fn set_scheme(&mut self, scheme: ColorScheme) {
        self.scheme = scheme;
        self.persist_pref("colorScheme", scheme.name());
    }

    /// Switch between light, dark and the desktop's choice, and remember it.
    pub fn set_appearance(&mut self, appearance: Appearance) {
        self.appearance = appearance;
        self.persist_pref("appearance", appearance.name());
    }

    fn persist_pref(&mut self, key: &str, value: &str) {
        if let Some(prefs) = &mut self.prefs {
            if let Err(err) = prefs.set(key, &value.to_string()) {
                tracing::debug!(error = %err, key, "could not persist a UI preference");
            }
        }
    }

    /// Per frame: land finished work, stamp and expire toasts, and run the
    /// small focus-driven reactions. True when visible state changed.
    pub fn poll(&mut self, now: f64, tx: &mut Tx) -> bool {
        self.now = now;
        let jobs = self.inbox.take();
        let mut changed = !jobs.is_empty();
        for job in jobs {
            job(self, tx);
        }

        // Toasts: stamp new ones, drop expired ones, wake for the next expiry.
        let mut next: Option<f64> = None;
        for t in &mut self.toasts {
            let at = *t
                .expires_at
                .get_or_insert(now + if t.error { 8.0 } else { 4.0 });
            next = Some(next.map_or(at, |n: f64| n.min(at)));
        }
        let before = self.toasts.len();
        self.toasts
            .retain(|t| t.expires_at.is_none_or(|at| at > now));
        changed |= self.toasts.len() != before;
        if let Some(at) = next.filter(|at| *at > now) {
            tx.wake_after(std::time::Duration::from_secs_f64(at - now));
        }

        // Settings: disarm a pending removal after its window.
        if let Some(Modal::Settings(st)) = &mut self.modal {
            if let Some((_, deadline)) = &mut st.pending_removal {
                let at = *deadline.get_or_insert(now + 4.0);
                if at <= now {
                    st.pending_removal = None;
                    changed = true;
                } else {
                    tx.wake_after(std::time::Duration::from_secs_f64(at - now));
                }
            }
        }

        // Add account: leaving the email field runs discovery.
        if let Some(Modal::AddAccount(form)) = &mut self.modal {
            let focused = form.email.focused;
            let blurred = form.email_was_focused && !focused;
            form.email_was_focused = focused;
            if blurred {
                self.run_discovery();
                changed = true;
            }
        }

        // Images decode on worker threads: redraw when one lands.
        let decoded = self.decoded_images();
        if decoded != self.decoded_seen {
            self.decoded_seen = decoded;
            changed = true;
        }

        if let Some(key) = self.reveal.take() {
            tx.scroll_into_view(key);
        }
        if std::mem::take(&mut self.activate_requested) {
            if let Some(window) = &self.window {
                window.open();
                crate::desktop::hyprland_focus();
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests;
