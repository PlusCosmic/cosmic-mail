//! Shared application state managed by Tauri.

use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::sync::SyncManager;

/// A thread-safe handle to the single SQLite connection.
///
/// A `Mutex`-wrapped `Connection` behind an `Arc` is adequate for the current
/// scale (light, bursty query load from commands and a handful of sync tasks).
pub type Db = Arc<Mutex<Connection>>;

/// Global application state stored in Tauri's state map.
pub struct AppState {
    /// Shared database connection.
    pub db: Db,
    /// Owns the per-account sync tasks.
    pub sync: SyncManager,
}

impl AppState {
    /// Construct app state from an open connection.
    pub fn new(conn: Connection) -> Self {
        AppState {
            db: Arc::new(Mutex::new(conn)),
            sync: SyncManager::new(),
        }
    }
}
