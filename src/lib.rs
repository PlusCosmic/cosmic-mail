//! Cosmic Mail: an IMAP/Gmail client for the omarchy desktop.
//!
//! The backend (`backend`, `store`, `sync`, `auth`, ...) is UI-agnostic and
//! reports changes through [`events::Events`]. The `ui` module is the native
//! Rune interface on top of it; `app` wires the two together at startup.

pub mod accounts;
pub mod app;
pub mod attachments;
pub mod auth;
pub mod autoconfig;
pub mod backend;
pub mod desktop;
pub mod error;
pub mod events;
pub mod notifications;
pub mod omarchy;
pub mod send;
pub mod settings;
pub mod shipments;
pub mod store;
pub mod sync;
pub mod ui;
pub mod wire;

pub use app::run;

/// The app version: `COSMIC_MAIL_VERSION` when the build sets it (CI and the
/// packages, from `scripts/version.sh`: MAJOR.MINOR from Cargo.toml plus the
/// commit count), else Cargo.toml's version for local builds.
pub const VERSION: &str = match option_env!("COSMIC_MAIL_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
