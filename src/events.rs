//! Backend → UI events.
//!
//! The backend (sync engine, notifications) reports changes
//! through an [`Events`] sink instead of a webview event bus. The UI installs a
//! sink that queues each event onto its own thread (see `ui::Inbox`); tests
//! install one that records them. Event names and payloads mirror the contract
//! in `docs/ARCHITECTURE.md` ("Events").

use std::fmt;
use std::sync::Arc;

use crate::sync::SyncState;
use crate::wire::MessageSummary;

/// One backend event.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    /// `mail:new-messages`: new envelopes landed in a folder during sync.
    NewMessages {
        account_id: String,
        folder_id: i64,
        messages: Vec<MessageSummary>,
    },
    /// `mail:messages-updated`: flags/deletions/moves changed a folder; refetch.
    MessagesUpdated { folder_id: i64 },
    /// `mail:sync-state`: an account's sync task changed state.
    SyncState {
        account_id: String,
        state: SyncState,
        error: Option<String>,
        /// True only for a classified dead-credentials failure (Reconnect, not retry).
        needs_reauth: bool,
    },
    /// A notification (or a second launch) asked to show and focus the window.
    Activate,
}

/// Where backend events go. Cheap to clone; callable from any thread.
#[derive(Clone)]
pub struct Events(Arc<dyn Fn(AppEvent) + Send + Sync>);

impl Events {
    /// A sink that forwards every event to `f`.
    pub fn new(f: impl Fn(AppEvent) + Send + Sync + 'static) -> Self {
        Events(Arc::new(f))
    }

    /// A sink that drops every event.
    pub fn noop() -> Self {
        Events::new(|_| {})
    }

    /// Deliver one event.
    pub fn emit(&self, event: AppEvent) {
        (self.0)(event);
    }
}

impl fmt::Debug for Events {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Events")
    }
}
