//! Application error type.
//!
//! Internally the backend uses `anyhow` for ergonomic error propagation. At the
//! Tauri command boundary we convert to [`AppError`], which serializes to a
//! plain human-readable string (its `Display`) so the frontend can show it in a
//! toast. Commands therefore return `Result<T, AppError>`, which appears to the
//! frontend as `Result<T, String>`.

use serde::{Serialize, Serializer};

/// Error surfaced to the frontend as a string.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// A configuration or user-input problem with a friendly message.
    #[error("{0}")]
    Message(String),

    /// Any internal error, carried as an `anyhow` chain.
    #[error("{0}")]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    /// Build an [`AppError`] from anything string-like.
    pub fn msg(s: impl Into<String>) -> Self {
        AppError::Message(s.into())
    }
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Convenience alias for command results.
pub type AppResult<T> = Result<T, AppError>;
