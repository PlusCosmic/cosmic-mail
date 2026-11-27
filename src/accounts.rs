//! Account model, `accounts.json` persistence, and keyring secret helpers.
//!
//! Non-secret account configuration is stored in
//! `<config dir>/cosmic-mail/accounts.json` ([`crate::paths::config_dir`]).
//! Secrets (IMAP passwords, Gmail OAuth refresh tokens) live only in the
//! platform keyring (Credential Manager, Keychain, Secret Service) via the
//! `keyring` crate and are never written to disk or logged.

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

/// How the IMAP connection establishes TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImapTls {
    /// Implicit TLS from byte 0 (IMAPS, typically port 993).
    #[default]
    Implicit,
    /// Plain TCP greeting upgraded via required `STARTTLS` (typically port 143).
    Starttls,
}

impl ImapTls {
    pub const ALL: [ImapTls; 2] = [ImapTls::Implicit, ImapTls::Starttls];

    pub fn label(self) -> &'static str {
        match self {
            ImapTls::Implicit => "SSL/TLS",
            ImapTls::Starttls => "STARTTLS",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            ImapTls::Implicit => 993,
            ImapTls::Starttls => 143,
        }
    }
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
    /// IMAP port (typically 993 for implicit TLS, 143 for STARTTLS).
    pub imap_port: u16,
    /// How the IMAP connection establishes TLS (defaults to implicit for older profiles).
    #[serde(default)]
    pub imap_tls: ImapTls,
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
    let dir = crate::paths::config_dir()
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
///
/// Debug builds only: if `COSMIC_MAIL_TEST_IMAP_PASSWORD` is set, return it
/// without touching the keyring, so E2E tests can run against a fixture IMAP
/// server on a headless machine with no Secret Service. Never compiled into
/// releases; the keyring path below is unchanged for real accounts.
pub fn get_imap_password(account_id: &str) -> Result<String> {
    #[cfg(debug_assertions)]
    if let Some(password) = std::env::var_os("COSMIC_MAIL_TEST_IMAP_PASSWORD") {
        return password
            .into_string()
            .map_err(|_| anyhow::anyhow!("COSMIC_MAIL_TEST_IMAP_PASSWORD is not valid UTF-8"));
    }

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

/// Set an account's display name on disk, returning the updated account (`None` if absent).
pub fn rename_account_on_disk(account_id: &str, display_name: &str) -> Result<Option<Account>> {
    let mut accounts = load_accounts()?;
    let Some(account) = accounts.iter_mut().find(|a| a.id == account_id) else {
        return Ok(None);
    };
    account.display_name = display_name.to_string();
    let updated = account.clone();
    save_accounts(&accounts)?;
    Ok(Some(updated))
}

/// Append an account to the on-disk list and persist.
pub fn add_account_to_disk(account: &Account) -> Result<()> {
    let mut accounts = load_accounts()?;
    accounts.push(account.clone());
    save_accounts(&accounts)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_without_imap_tls_defaults_to_implicit() {
        let legacy = r#"{
            "id": "acct-1",
            "email": "user@example.com",
            "displayName": "User",
            "kind": "imap",
            "imapHost": "localhost",
            "imapPort": 3993,
            "smtpHost": "localhost",
            "smtpPort": 3025,
            "username": "user"
        }"#;
        let acc: Account = serde_json::from_str(legacy).expect("deserialize legacy account");
        assert_eq!(acc.imap_tls, ImapTls::Implicit);

        let starttls_json = serde_json::to_string(&Account {
            imap_tls: ImapTls::Starttls,
            ..acc
        })
        .expect("serialize starttls account");
        assert!(starttls_json.contains(r#""imapTls":"starttls""#));
        let roundtrip: Account =
            serde_json::from_str(&starttls_json).expect("deserialize starttls account");
        assert_eq!(roundtrip.imap_tls, ImapTls::Starttls);
    }
}
