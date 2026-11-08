//! Application error type.
//!
//! Internally the backend uses `anyhow` for ergonomic error propagation. At the
//! [`Backend`](crate::backend::Backend) boundary we convert to [`AppError`],
//! whose `Display` is a plain human-readable string the UI shows in a toast.

/// Error surfaced to the UI as a string.
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

/// Convenience alias for command results.
pub type AppResult<T> = Result<T, AppError>;
