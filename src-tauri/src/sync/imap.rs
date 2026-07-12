//! IMAP connector and per-session operations built on `async-imap` over
//! `tokio-rustls`.
//!
//! Supports two auth mechanisms:
//! - `LOGIN` with a keyring password (plain IMAP accounts), and
//! - SASL `AUTHENTICATE XOAUTH2` (Gmail), via [`async_imap::Authenticator`].

use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use async_imap::types::{Fetch, Name};
use async_imap::Session;
use futures::StreamExt;
use mail_parser::MessageParser;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

use crate::accounts::{Account, AccountKind};
use crate::store::FolderRole;

const BODY_FETCH_QUERY: &str = "(BODY.PEEK[])";

/// An authenticated IMAP session over a TLS stream.
pub type ImapSession = Session<TlsStream<TcpStream>>;

/// SASL authenticator for `XOAUTH2`.
struct XOAuth2 {
    user: String,
    access_token: String,
}

impl async_imap::Authenticator for &XOAuth2 {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        // Raw SASL XOAUTH2 string; async-imap base64-encodes the response
        // itself, so returning it pre-encoded would double-encode it and
        // Gmail rejects that with "Invalid SASL argument".
        format!(
            "user={}\x01auth=Bearer {}\x01\x01",
            self.user, self.access_token
        )
    }
}

/// Build a rustls client config trusting the OS native roots.
fn tls_config() -> Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        // Ignore individual bad certs; a broken root shouldn't abort startup.
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        bail!("no native root certificates available for TLS");
    }
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Open a TLS connection to `host:port` and return an unauthenticated client.
async fn tls_client(host: &str, port: u16) -> Result<async_imap::Client<TlsStream<TcpStream>>> {
    let config = tls_config()?;
    let connector = TlsConnector::from(config);
    let server_name = ServerName::try_from(host.to_string())
        .with_context(|| format!("invalid server name {host}"))?;

    let tcp = TcpStream::connect((host, port))
        .await
        .with_context(|| format!("connecting to {host}:{port}"))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .with_context(|| format!("TLS handshake with {host}"))?;

    let mut client = async_imap::Client::new(tls);
    // Read the server greeting.
    let _greeting = client
        .read_response()
        .await
        .context("reading IMAP greeting")?
        .context("server closed before greeting")?;
    Ok(client)
}

/// Connect and authenticate an IMAP session for the given account.
///
/// For `imap` accounts the password comes from the keyring; for `gmail`
/// accounts an OAuth access token is obtained (and refreshed) via
/// [`crate::auth::oauth`].
pub async fn connect(account: &Account) -> Result<ImapSession> {
    let client = tls_client(&account.imap_host, account.imap_port).await?;

    match account.kind {
        AccountKind::Imap => {
            let password = crate::accounts::get_imap_password(&account.id)
                .context("no stored IMAP password")?;
            let session = client
                .login(&account.username, &password)
                .await
                .map_err(|(e, _client)| anyhow!("IMAP LOGIN failed: {e}"))?;
            Ok(session)
        }
        AccountKind::Gmail => {
            let token = crate::auth::oauth::access_token(&account.id)
                .await
                .context("obtaining Gmail access token")?;
            let auth = XOAuth2 {
                user: account.username.clone(),
                access_token: token,
            };
            let session = client
                .authenticate("XOAUTH2", &auth)
                .await
                .map_err(|(e, _client)| anyhow!("IMAP XOAUTH2 auth failed: {e}"))?;
            Ok(session)
        }
    }
}

/// A folder discovered on the server.
#[derive(Debug, Clone)]
pub struct RemoteFolder {
    pub name: String,
    pub role: FolderRole,
}

/// List all folders and assign roles from SPECIAL-USE attributes, falling back
/// to name heuristics.
pub async fn list_folders_with_roles(session: &mut ImapSession) -> Result<Vec<RemoteFolder>> {
    let stream = session
        .list(Some(""), Some("*"))
        .await
        .context("issuing LIST")?;
    let names: Vec<Name> = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<async_imap::error::Result<Vec<_>>>()
        .context("collecting LIST results")?;

    let mut folders = Vec::new();
    for name in names {
        // Skip non-selectable containers.
        if name
            .attributes()
            .iter()
            .any(|a| matches!(a, async_imap::imap_proto::NameAttribute::NoSelect))
        {
            continue;
        }
        let full = name.name().to_string();
        let role = role_from_attributes(&name).unwrap_or_else(|| role_from_name(&full));
        folders.push(RemoteFolder { name: full, role });
    }
    Ok(folders)
}

fn role_from_attributes(name: &Name) -> Option<FolderRole> {
    use async_imap::imap_proto::NameAttribute;
    for attr in name.attributes() {
        let role = match attr {
            NameAttribute::All | NameAttribute::Archive => Some(FolderRole::Archive),
            NameAttribute::Drafts => Some(FolderRole::Drafts),
            NameAttribute::Junk => Some(FolderRole::Spam),
            NameAttribute::Sent => Some(FolderRole::Sent),
            NameAttribute::Trash => Some(FolderRole::Trash),
            _ => None,
        };
        if role.is_some() {
            return role;
        }
    }
    None
}

fn role_from_name(name: &str) -> FolderRole {
    let lower = name.to_ascii_lowercase();
    let leaf = lower.rsplit('/').next().unwrap_or(&lower);
    if leaf == "inbox" {
        FolderRole::Inbox
    } else if leaf.contains("sent") {
        FolderRole::Sent
    } else if leaf.contains("draft") {
        FolderRole::Drafts
    } else if leaf.contains("trash") || leaf.contains("deleted") {
        FolderRole::Trash
    } else if leaf.contains("spam") || leaf.contains("junk") {
        FolderRole::Spam
    } else if leaf.contains("archive") || leaf.contains("all mail") {
        FolderRole::Archive
    } else {
        FolderRole::Normal
    }
}

/// Authoritative mailbox counters and UID metadata returned by IMAP STATUS.
#[derive(Debug, Clone, Copy)]
pub struct FolderStatus {
    pub uidvalidity: u32,
    pub exists: u32,
    pub unseen: u32,
}

/// Read mailbox counters without selecting it.
pub async fn status(session: &mut ImapSession, mailbox: &str) -> Result<FolderStatus> {
    let mb = session
        .status(mailbox, "(MESSAGES UNSEEN UIDNEXT UIDVALIDITY)")
        .await
        .with_context(|| format!("reading status for {mailbox}"))?;
    Ok(FolderStatus {
        uidvalidity: mb.uid_validity.unwrap_or(0),
        exists: mb.exists,
        // For STATUS responses async-imap maps the UNSEEN count onto this field.
        unseen: mb.unseen.unwrap_or(0),
    })
}

/// Select (read-write) a mailbox and return its UID metadata.
pub async fn select(session: &mut ImapSession, mailbox: &str) -> Result<()> {
    session
        .select(mailbox)
        .await
        .with_context(|| format!("selecting {mailbox}"))?;
    Ok(())
}

/// A decoded envelope summary for one message.
#[derive(Debug, Clone)]
pub struct EnvelopeSummary {
    pub uid: u32,
    pub message_id: Option<String>,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub to_addrs: Vec<String>,
    pub cc_addrs: Vec<String>,
    pub date: String,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachments: bool,
    pub rfc822_size: u32,
}

/// Fetch envelope summaries for a UID range string (e.g. `"12:*"` or `"1:200"`).
pub async fn fetch_envelopes(
    session: &mut ImapSession,
    uid_set: &str,
) -> Result<Vec<EnvelopeSummary>> {
    let query = "(UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE RFC822.SIZE)";
    let mut stream = session
        .uid_fetch(uid_set, query)
        .await
        .context("issuing UID FETCH for envelopes")?;

    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        let fetch = item.context("reading fetch item")?;
        if let Some(summary) = envelope_from_fetch(&fetch) {
            out.push(summary);
        }
    }
    Ok(out)
}

/// Fetch at most the newest `limit` existing messages using sequence numbers.
///
/// Sequence numbers are dense within the selected mailbox, unlike UIDs, so the
/// server returns exactly the desired tail even when the mailbox has UID gaps.
pub async fn fetch_recent_envelopes(
    session: &mut ImapSession,
    exists: u32,
    limit: u32,
) -> Result<Vec<EnvelopeSummary>> {
    let Some(sequence_set) = recent_sequence_set(exists, limit) else {
        return Ok(Vec::new());
    };
    let query = "(UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE RFC822.SIZE)";
    let mut stream = session
        .fetch(sequence_set, query)
        .await
        .context("issuing FETCH for recent envelopes")?;

    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        let fetch = item.context("reading recent fetch item")?;
        if let Some(summary) = envelope_from_fetch(&fetch) {
            out.push(summary);
        }
    }
    Ok(out)
}

fn recent_sequence_set(exists: u32, limit: u32) -> Option<String> {
    if exists == 0 || limit == 0 {
        return None;
    }
    let start = exists.saturating_sub(limit).saturating_add(1);
    Some(format!("{start}:*"))
}

fn envelope_from_fetch(fetch: &Fetch) -> Option<EnvelopeSummary> {
    let uid = fetch.uid?;
    let env = fetch.envelope()?;

    let subject = env
        .subject
        .as_ref()
        .map(|b| decode_mime_words(b))
        .unwrap_or_default();

    let (from_name, from_addr) = env
        .from
        .as_ref()
        .and_then(|list| list.first())
        .map(address_parts)
        .unwrap_or_default();

    let to_addrs = env
        .to
        .as_ref()
        .map(|list| list.iter().filter_map(|a| address_email(a)).collect())
        .unwrap_or_default();
    let cc_addrs = env
        .cc
        .as_ref()
        .map(|list| list.iter().filter_map(|a| address_email(a)).collect())
        .unwrap_or_default();

    let message_id = env
        .message_id
        .as_ref()
        .map(|b| String::from_utf8_lossy(b).trim().to_string());

    // Prefer INTERNALDATE for stable ordering; fall back to envelope date.
    let date = fetch
        .internal_date()
        .map(|d| d.to_rfc3339())
        .or_else(|| env.date.as_ref().and_then(|b| parse_env_date(b)))
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

    let mut seen = false;
    let mut flagged = false;
    for flag in fetch.flags() {
        match flag {
            async_imap::types::Flag::Seen => seen = true,
            async_imap::types::Flag::Flagged => flagged = true,
            _ => {}
        }
    }

    let has_attachments = bodystructure_has_attachment(fetch);

    Some(EnvelopeSummary {
        uid,
        message_id,
        subject,
        from_name,
        from_addr,
        to_addrs,
        cc_addrs,
        date,
        seen,
        flagged,
        has_attachments,
        rfc822_size: fetch.size.unwrap_or(0),
    })
}

/// Very rough attachment heuristic from BODYSTRUCTURE: any `multipart/mixed`
/// signals an attachment is likely present.
fn bodystructure_has_attachment(fetch: &Fetch) -> bool {
    use async_imap::imap_proto::BodyStructure;
    fn walk(bs: &BodyStructure<'_>) -> bool {
        match bs {
            BodyStructure::Multipart { common, bodies, .. } => {
                if common.ty.subtype.eq_ignore_ascii_case("mixed") {
                    return true;
                }
                bodies.iter().any(walk)
            }
            BodyStructure::Basic { common, .. } => common.ty.ty.eq_ignore_ascii_case("application"),
            _ => false,
        }
    }
    fetch.bodystructure().map(walk).unwrap_or(false)
}

/// Split an IMAP address into (display-name, email).
fn address_parts(addr: &async_imap::imap_proto::Address<'_>) -> (String, String) {
    let name = addr
        .name
        .as_ref()
        .map(|b| decode_mime_words(b))
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    let email = address_email(addr).unwrap_or_default();
    let name = if name.is_empty() { email.clone() } else { name };
    (name, email)
}

/// Build the `mailbox@host` email string from an IMAP address.
fn address_email(addr: &async_imap::imap_proto::Address<'_>) -> Option<String> {
    let mailbox = addr.mailbox.as_ref().map(|b| String::from_utf8_lossy(b));
    let host = addr.host.as_ref().map(|b| String::from_utf8_lossy(b));
    match (mailbox, host) {
        (Some(m), Some(h)) => Some(format!("{m}@{h}")),
        (Some(m), None) => Some(m.to_string()),
        _ => None,
    }
}

/// Decode RFC 2047 encoded-words in a raw header value using mail-parser.
fn decode_mime_words(raw: &[u8]) -> String {
    // Parse a synthetic header so mail-parser handles RFC 2047 for us.
    let mut synthetic = b"Subject: ".to_vec();
    synthetic.extend_from_slice(raw);
    synthetic.extend_from_slice(b"\r\n\r\n");
    if let Some(msg) = MessageParser::default().parse(&synthetic) {
        if let Some(s) = msg.subject() {
            return s.to_string();
        }
    }
    String::from_utf8_lossy(raw).trim().to_string()
}

/// Parse an envelope date (RFC 2822) into RFC 3339, if possible.
fn parse_env_date(raw: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(raw);
    chrono::DateTime::parse_from_rfc2822(s.trim())
        .ok()
        .map(|d| d.to_rfc3339())
}

/// A fetched, parsed message body.
#[derive(Debug, Clone)]
pub struct FetchedBody {
    pub text: Option<String>,
    pub html: Option<String>,
    pub to_addrs: Vec<String>,
    pub cc_addrs: Vec<String>,
}

/// Fetch and parse the full body for a single UID.
pub async fn fetch_body(session: &mut ImapSession, uid: u32) -> Result<FetchedBody> {
    let mut stream = session
        .uid_fetch(uid.to_string(), BODY_FETCH_QUERY)
        .await
        .context("issuing UID FETCH for body")?;

    let mut raw: Option<Vec<u8>> = None;
    while let Some(item) = stream.next().await {
        let fetch = item.context("reading body fetch item")?;
        if let Some(body) = fetch.body().or_else(|| fetch.text()) {
            raw = Some(body.to_vec());
        }
    }
    let raw = raw.context("server returned no body for message")?;

    let parsed = MessageParser::default()
        .parse(&raw)
        .context("parsing message body")?;

    let text = parsed.body_text(0).map(|c| c.into_owned());
    let html = parsed.body_html(0).map(|c| c.into_owned());

    let to_addrs = collect_addrs(parsed.to());
    let cc_addrs = collect_addrs(parsed.cc());

    Ok(FetchedBody {
        text,
        html,
        to_addrs,
        cc_addrs,
    })
}

/// Read the server-reported full-message size without fetching body content.
pub async fn fetch_message_size(session: &mut ImapSession, uid: u32) -> Result<u32> {
    let mut stream = session
        .uid_fetch(uid.to_string(), "(RFC822.SIZE)")
        .await
        .context("issuing UID FETCH for message size")?;

    while let Some(item) = stream.next().await {
        let fetch = item.context("reading message size fetch item")?;
        if let Some(size) = fetch.size {
            return Ok(size);
        }
    }
    bail!("server returned no size for message")
}

fn collect_addrs(addr: Option<&mail_parser::Address<'_>>) -> Vec<String> {
    addr.map(|a| {
        a.iter()
            .filter_map(|item| {
                item.address().map(|s| {
                    if let Some(name) = item.name() {
                        format!("{name} <{s}>")
                    } else {
                        s.to_string()
                    }
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Set or clear the `\Seen` flag on a message by UID.
pub async fn set_seen_flag(session: &mut ImapSession, uid: u32, seen: bool) -> Result<()> {
    let op = if seen {
        "+FLAGS (\\Seen)"
    } else {
        "-FLAGS (\\Seen)"
    };
    let mut stream = session
        .uid_store(uid.to_string(), op)
        .await
        .context("issuing UID STORE")?;
    // Drain the response.
    while let Some(item) = stream.next().await {
        item.context("reading store response")?;
    }
    Ok(())
}

/// Compute a single-line snippet from a text body (~160 chars).
pub fn snippet_from_text(text: &str) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use super::{recent_sequence_set, BODY_FETCH_QUERY};

    #[test]
    fn recent_sequence_range_is_bounded_to_existing_messages() {
        assert_eq!(recent_sequence_set(1_497, 200).as_deref(), Some("1298:*"));
        assert_eq!(recent_sequence_set(42, 200).as_deref(), Some("1:*"));
        assert_eq!(recent_sequence_set(0, 200), None);
        assert_eq!(recent_sequence_set(42, 0), None);
    }

    #[test]
    fn body_fetch_query_never_sets_seen() {
        assert_eq!(BODY_FETCH_QUERY, "(BODY.PEEK[])");
        assert!(!BODY_FETCH_QUERY.contains("RFC822"));
    }
}
