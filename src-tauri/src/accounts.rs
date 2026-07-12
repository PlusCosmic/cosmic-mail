//! Account model, `accounts.json` persistence, and keyring secret helpers.
//!
//! Non-secret account configuration is stored in
//! `$XDG_CONFIG_HOME/cosmic-mail/accounts.json`. Secrets (IMAP passwords, Gmail
//! OAuth refresh tokens) live only in the Secret Service via the `keyring`
//! crate and are never written to disk or logged.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Keyring service name (shared by all secrets for this app).
pub const KEYRING_SERVICE: &str = "dev.pluscosmic.mail";

/// The kind of account, determining the auth mechanism used for IMAP/SMTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountKind {
    /// Plain IMAP with a password stored in the keyring.
    Imap,
    /// Gmail via OAuth2/XOAUTH2.
    Gmail,
}

/// Full account record persisted to `accounts.json` (contains no secrets).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// UUID v4 stable identifier.
    pub id: String,
    /// Primary email address.
    pub email: String,
    /// Human display name.
    pub display_name: String,
    /// Account kind (auth mechanism).
    pub kind: AccountKind,

    // Connection details. For Gmail these are hardcoded to the Gmail servers.
    /// IMAP host, e.g. `imap.gmail.com`.
    pub imap_host: String,
    /// IMAP port (implicit TLS, usually 993).
    pub imap_port: u16,
    /// SMTP host, e.g. `smtp.gmail.com`.
    pub smtp_host: String,
    /// SMTP port (STARTTLS 587 or implicit 465).
    pub smtp_port: u16,
    /// Login username (often equal to `email`).
    pub username: String,
}

/// Public projection of [`Account`] returned to the frontend (no connection
/// details or secrets — only what the UI needs).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPublic {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub kind: AccountKind,
}

impl Account {
    /// Project to the public wire type.
    pub fn public(&self) -> AccountPublic {
        AccountPublic {
            id: self.id.clone(),
            email: self.email.clone(),
            display_name: self.display_name.clone(),
            kind: self.kind,
        }
    }
}

/// Path to `accounts.json`: `$XDG_CONFIG_HOME/cosmic-mail/accounts.json`.
pub fn accounts_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("could not determine config dir (XDG_CONFIG_HOME)")?
        .join("cosmic-mail");
    Ok(dir.join("accounts.json"))
}

/// Load all accounts from disk, returning an empty list if the file is absent.
pub fn load_accounts() -> Result<Vec<Account>> {
    let path = accounts_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let accounts: Vec<Account> = serde_json::from_slice(&data).context("parsing accounts.json")?;
    Ok(accounts)
}

/// Atomically persist the account list to `accounts.json`.
///
/// Writes to a sibling temp file and renames into place so a crash mid-write
/// cannot corrupt the config.
pub fn save_accounts(accounts: &[Account]) -> Result<()> {
    let path = accounts_path()?;
    let dir = path
        .parent()
        .context("accounts.json has no parent directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let json = serde_json::to_vec_pretty(accounts).context("serializing accounts")?;

    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("creating temp file in {}", dir.display()))?;
    tmp.write_all(&json).context("writing accounts temp file")?;
    tmp.flush().context("flushing accounts temp file")?;
    tmp.persist(&path)
        .with_context(|| format!("persisting {}", path.display()))?;
    Ok(())
}

// --- keyring helpers ---------------------------------------------------------

fn imap_password_key(account_id: &str) -> String {
    format!("imap-password:{account_id}")
}

fn oauth_refresh_token_key(account_id: &str) -> String {
    format!("oauth-refresh-token:{account_id}")
}

fn entry(user_key: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, user_key)
        .with_context(|| format!("opening keyring entry {user_key}"))
}

/// Store an IMAP password for the given account.
pub fn set_imap_password(account_id: &str, password: &str) -> Result<()> {
    entry(&imap_password_key(account_id))?
        .set_password(password)
        .context("storing IMAP password in keyring")?;
    Ok(())
}

/// Fetch the IMAP password for the given account.
pub fn get_imap_password(account_id: &str) -> Result<String> {
    entry(&imap_password_key(account_id))?
        .get_password()
        .context("reading IMAP password from keyring")
}

/// Store the Gmail OAuth refresh token for the given account.
pub fn set_oauth_refresh_token(account_id: &str, token: &str) -> Result<()> {
    entry(&oauth_refresh_token_key(account_id))?
        .set_password(token)
        .context("storing OAuth refresh token in keyring")?;
    Ok(())
}

/// Fetch the Gmail OAuth refresh token for the given account.
pub fn get_oauth_refresh_token(account_id: &str) -> Result<String> {
    entry(&oauth_refresh_token_key(account_id))?
        .get_password()
        .context("reading OAuth refresh token from keyring")
}

/// Best-effort deletion of all secrets for an account (ignores missing entries).
pub fn delete_secrets(account_id: &str, kind: AccountKind) {
    let key = match kind {
        AccountKind::Imap => imap_password_key(account_id),
        AccountKind::Gmail => oauth_refresh_token_key(account_id),
    };
    if let Ok(entry) = entry(&key) {
        // Ignore NoEntry / other deletion errors — this is best effort.
        let _ = entry.delete_credential();
    }
}

/// Remove an account from the on-disk list, if present.
pub fn remove_account_from_disk(account_id: &str) -> Result<Option<Account>> {
    let mut accounts = load_accounts()?;
    if let Some(pos) = accounts.iter().position(|a| a.id == account_id) {
        let removed = accounts.remove(pos);
        save_accounts(&accounts)?;
        Ok(Some(removed))
    } else {
        Ok(None)
    }
}

/// Append an account to the on-disk list and persist.
pub fn add_account_to_disk(account: &Account) -> Result<()> {
    let mut accounts = load_accounts()?;
    accounts.push(account.clone());
    save_accounts(&accounts)?;
    Ok(())
}
