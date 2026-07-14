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
use base64::Engine;
use futures::StreamExt;
use mail_parser::{Message, MessageParser, MessagePart, MimeHeaders};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

use crate::accounts::{Account, AccountKind};
use crate::store::{AttachmentMeta, FolderRole};

const BODY_FETCH_QUERY: &str = "(BODY.PEEK[])";

/// Per-part decoded-size cap for inlining a `cid:` image as a `data:` URI.
const INLINE_IMAGE_MAX_BYTES: usize = 512 * 1024;
/// Per-message total budget for inlined `cid:` image payloads.
const INLINE_IMAGE_TOTAL_BUDGET: usize = 2 * 1024 * 1024;

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
    /// HTML body with in-cap inline `cid:` images already rewritten to `data:` URIs.
    pub html: Option<String>,
    pub to_addrs: Vec<String>,
    pub cc_addrs: Vec<String>,
    /// Attachment metadata (listed attachments and inline `cid:` parts).
    pub attachments: Vec<AttachmentMeta>,
}

/// Fetch the raw RFC822 bytes for a single UID with `BODY.PEEK[]` (never `\Seen`).
async fn fetch_raw_body(session: &mut ImapSession, uid: u32) -> Result<Vec<u8>> {
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
    raw.context("server returned no body for message")
}

/// Fetch and parse the full body for a single UID.
pub async fn fetch_body(session: &mut ImapSession, uid: u32) -> Result<FetchedBody> {
    let raw = fetch_raw_body(session, uid).await?;

    let parsed = MessageParser::default()
        .parse(&raw)
        .context("parsing message body")?;

    Ok(parse_body(&parsed))
}

/// Decode a parsed message into cached body parts + attachment metadata.
///
/// Inline `cid:` image references in the HTML body are rewritten to `data:` URIs
/// under strict per-part and per-message size caps; over-cap parts keep their
/// `cid:` reference (which renders blank under the reader CSP). No network I/O.
fn parse_body(parsed: &Message<'_>) -> FetchedBody {
    let text = parsed.body_text(0).map(|c| c.into_owned());
    let html = parsed.body_html(0).map(|c| c.into_owned()).map(|h| {
        let images = inline_image_data_uris(parsed);
        if images.is_empty() {
            h
        } else {
            rewrite_cid_references(&h, &images)
        }
    });

    FetchedBody {
        text,
        html,
        to_addrs: collect_addrs(parsed.to()),
        cc_addrs: collect_addrs(parsed.cc()),
        attachments: extract_attachments(parsed),
    }
}

/// Fetch the full message and return one part's decoded bytes by parse index.
///
/// Used by `save_attachment`: the raw RFC822 is not cached, so the message is
/// refetched (`BODY.PEEK[]`, non-marking) and re-parsed, then the part at
/// `part_index` (a stable mail-parser id) is decoded.
pub async fn fetch_attachment_bytes(
    session: &mut ImapSession,
    uid: u32,
    part_index: u32,
) -> Result<Vec<u8>> {
    let raw = fetch_raw_body(session, uid).await?;
    let parsed = MessageParser::default()
        .parse(&raw)
        .context("parsing message body")?;
    let part = parsed
        .part(part_index)
        .context("attachment part not found in message")?;
    Ok(part.contents().to_vec())
}

/// The MIME type of a part as `type/subtype`, lowercased.
fn part_mime_type(part: &MessagePart<'_>) -> String {
    match part.content_type() {
        Some(ct) => {
            let main = ct.ctype().to_ascii_lowercase();
            match ct.subtype() {
                Some(sub) => format!("{main}/{}", sub.to_ascii_lowercase()),
                None => main,
            }
        }
        None => "application/octet-stream".to_string(),
    }
}

/// Content-ID normalized without surrounding angle brackets.
fn normalize_content_id(cid: &str) -> String {
    cid.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

/// Extract attachment metadata (listed attachments + inline `cid:` parts) in
/// deterministic parse order. `part_index` is the mail-parser part id so the
/// exact part can be re-fetched later.
fn extract_attachments(parsed: &Message<'_>) -> Vec<AttachmentMeta> {
    let mut out = Vec::new();
    for &part_id in &parsed.attachments {
        let Some(part) = parsed.part(part_id) else {
            continue;
        };
        // Skip nested messages and multipart containers: no downloadable payload.
        if part.is_message() || part.is_multipart() {
            continue;
        }
        let content_id = part.content_id().map(normalize_content_id);
        let disposition_attachment = part
            .content_disposition()
            .is_some_and(|d| d.is_attachment());
        // A part with a Content-ID that is not an explicit attachment is inline
        // (multipart/related embedded images render via cid:).
        let is_inline = !disposition_attachment && content_id.is_some();
        // mail-parser handles RFC 2231 in parameters but not RFC 2047
        // encoded-words inside a filename value, so decode those ourselves.
        let filename = part
            .attachment_name()
            .map(|n| decode_mime_words(n.as_bytes()))
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        out.push(AttachmentMeta {
            part_index: part_id,
            filename,
            mime_type: part_mime_type(part),
            size_bytes: part.len() as u32,
            is_inline,
            content_id: content_id.filter(|s| !s.is_empty()),
        });
    }
    out
}

/// Build `(normalized-lowercased content-id, data: URI)` pairs for inline images
/// within the per-part and per-message size caps.
fn inline_image_data_uris(parsed: &Message<'_>) -> Vec<(String, String)> {
    let mut total = 0usize;
    let mut out = Vec::new();
    for &part_id in &parsed.attachments {
        let Some(part) = parsed.part(part_id) else {
            continue;
        };
        let Some(cid) = part.content_id() else {
            continue;
        };
        let mime = part_mime_type(part);
        if !mime.starts_with("image/") {
            continue;
        }
        let bytes = part.contents();
        if bytes.is_empty() || bytes.len() > INLINE_IMAGE_MAX_BYTES {
            continue;
        }
        if total + bytes.len() > INLINE_IMAGE_TOTAL_BUDGET {
            // Over the per-message budget: leave this part as a cid: reference.
            continue;
        }
        total += bytes.len();
        let cid_norm = normalize_content_id(cid);
        if cid_norm.is_empty() {
            continue;
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        out.push((
            cid_norm.to_ascii_lowercase(),
            format!("data:{mime};base64,{encoded}"),
        ));
    }
    out
}

/// Replace each `cid:<content-id>` reference in `html` with its `data:` URI.
/// Matching on the `cid:` prefix + content-id is ASCII case-insensitive.
fn rewrite_cid_references(html: &str, images: &[(String, String)]) -> String {
    let mut result = html.to_string();
    for (cid_lower, data_uri) in images {
        let needle = format!("cid:{cid_lower}");
        result = replace_ascii_case_insensitive(&result, &needle, data_uri);
    }
    result
}

/// ASCII case-insensitive, literal (non-regex) substring replacement.
fn replace_ascii_case_insensitive(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let hay_lower = haystack.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let mut result = String::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if hay_lower[i..].starts_with(&needle_lower) {
            result.push_str(replacement);
            i += needle_lower.len();
        } else {
            let ch = haystack[i..]
                .chars()
                .next()
                .expect("byte index is on a char boundary");
            result.push(ch);
            i += ch.len_utf8();
        }
    }
    result
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

/// Set or clear the `\Flagged` flag on a message by UID.
pub async fn set_flagged_flag(session: &mut ImapSession, uid: u32, flagged: bool) -> Result<()> {
    let op = if flagged {
        "+FLAGS (\\Flagged)"
    } else {
        "-FLAGS (\\Flagged)"
    };
    let mut stream = session
        .uid_store(uid.to_string(), op)
        .await
        .context("issuing UID STORE for \\Flagged")?;
    while let Some(item) = stream.next().await {
        item.context("reading store response")?;
    }
    Ok(())
}

/// Move a message by UID from the currently-selected mailbox to `target_mailbox`.
///
/// Prefers `UID MOVE` (RFC 6851) when the server advertises the `MOVE`
/// capability. Otherwise falls back to the equivalent `UID COPY` + mark
/// `\Deleted` + expunge sequence, preferring `UID EXPUNGE` (RFC 4315 `UIDPLUS`)
/// so only the copied message is expunged; when `UIDPLUS` is absent it falls
/// back to a plain `EXPUNGE` (which removes every `\Deleted` message in the
/// mailbox). The mailbox must already be selected read-write by the caller.
pub async fn move_message(session: &mut ImapSession, uid: u32, target_mailbox: &str) -> Result<()> {
    let (has_move, has_uidplus) = server_capabilities(session).await?;
    if has_move {
        // async-imap quotes/validates the mailbox name internally.
        session
            .uid_mv(uid.to_string(), target_mailbox)
            .await
            .with_context(|| format!("UID MOVE to {target_mailbox}"))?;
        return Ok(());
    }
    session
        .uid_copy(uid.to_string(), target_mailbox)
        .await
        .with_context(|| format!("UID COPY to {target_mailbox}"))?;
    mark_deleted(session, uid).await?;
    expunge_uid(session, uid, has_uidplus).await?;
    Ok(())
}

/// Permanently delete a message by UID from the currently-selected mailbox.
///
/// Sets `\Deleted` then expunges, preferring `UID EXPUNGE` when `UIDPLUS` is
/// available so only this UID is removed; otherwise a plain `EXPUNGE` is used.
pub async fn delete_permanently(session: &mut ImapSession, uid: u32) -> Result<()> {
    let (_has_move, has_uidplus) = server_capabilities(session).await?;
    mark_deleted(session, uid).await?;
    expunge_uid(session, uid, has_uidplus).await?;
    Ok(())
}

/// Report `(supports MOVE, supports UIDPLUS)` for the current session.
async fn server_capabilities(session: &mut ImapSession) -> Result<(bool, bool)> {
    let caps = session
        .capabilities()
        .await
        .context("reading server capabilities")?;
    Ok((caps.has_str("MOVE"), caps.has_str("UIDPLUS")))
}

/// Add `\Deleted` to a single message by UID (draining the STORE response).
async fn mark_deleted(session: &mut ImapSession, uid: u32) -> Result<()> {
    let mut stream = session
        .uid_store(uid.to_string(), "+FLAGS (\\Deleted)")
        .await
        .context("issuing UID STORE +FLAGS (\\Deleted)")?;
    while let Some(item) = stream.next().await {
        item.context("reading store response")?;
    }
    Ok(())
}

/// Expunge a UID, preferring `UID EXPUNGE` when `UIDPLUS` is available so only
/// the given UID is removed; otherwise fall back to a plain `EXPUNGE`.
async fn expunge_uid(session: &mut ImapSession, uid: u32, has_uidplus: bool) -> Result<()> {
    // The expunge streams are not `Unpin`, so pin them before draining.
    if has_uidplus {
        let stream = session
            .uid_expunge(uid.to_string())
            .await
            .context("issuing UID EXPUNGE")?;
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            item.context("reading expunge response")?;
        }
    } else {
        let stream = session.expunge().await.context("issuing EXPUNGE")?;
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            item.context("reading expunge response")?;
        }
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
    use super::{
        parse_body, recent_sequence_set, replace_ascii_case_insensitive, BODY_FETCH_QUERY,
    };
    use base64::Engine;
    use mail_parser::MessageParser;

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

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn parse(raw: &str) -> super::FetchedBody {
        let parsed = MessageParser::default()
            .parse(raw.as_bytes())
            .expect("parse fixture");
        parse_body(&parsed)
    }

    #[test]
    fn extracts_attachment_metadata_and_marks_non_inline() {
        let raw = format!(
            "From: a@b.com\r\nTo: c@d.com\r\nSubject: Test\r\nMIME-Version: 1.0\r\n\
             Content-Type: multipart/mixed; boundary=\"BOUND\"\r\n\r\n\
             --BOUND\r\nContent-Type: text/plain\r\n\r\nHello body\r\n\
             --BOUND\r\nContent-Type: application/pdf; name=\"report.pdf\"\r\n\
             Content-Disposition: attachment; filename=\"report.pdf\"\r\n\
             Content-Transfer-Encoding: base64\r\n\r\n{}\r\n\
             --BOUND--\r\n",
            b64(b"PDF-CONTENT")
        );
        let body = parse(&raw);
        assert_eq!(body.attachments.len(), 1);
        let att = &body.attachments[0];
        assert_eq!(att.filename, "report.pdf");
        assert_eq!(att.mime_type, "application/pdf");
        assert_eq!(att.size_bytes, "PDF-CONTENT".len() as u32);
        assert!(!att.is_inline);
        assert!(att.content_id.is_none());
        // Deterministic part index: the pdf is the third flat part
        // (root multipart = 0, text = 1, pdf = 2).
        assert_eq!(att.part_index, 2);
        // A non-inline attachment is present.
        assert_eq!(body.attachments.iter().filter(|a| !a.is_inline).count(), 1);
    }

    #[test]
    fn decodes_rfc2047_encoded_filenames() {
        // "=?utf-8?B?ZsO2w7YucGRm?=" decodes to "föö.pdf".
        let encoded = format!("=?utf-8?B?{}?=", b64("föö.pdf".as_bytes()));
        let raw = format!(
            "Subject: t\r\nMIME-Version: 1.0\r\n\
             Content-Type: multipart/mixed; boundary=\"B\"\r\n\r\n\
             --B\r\nContent-Type: text/plain\r\n\r\nbody\r\n\
             --B\r\nContent-Type: application/pdf\r\n\
             Content-Disposition: attachment; filename=\"{encoded}\"\r\n\
             Content-Transfer-Encoding: base64\r\n\r\n{}\r\n\
             --B--\r\n",
            b64(b"data")
        );
        let body = parse(&raw);
        assert_eq!(body.attachments.len(), 1);
        assert_eq!(body.attachments[0].filename, "föö.pdf");
    }

    #[test]
    fn inlines_cid_image_within_caps() {
        let png = vec![0x89u8; 1024];
        let raw = format!(
            "Subject: t\r\nMIME-Version: 1.0\r\n\
             Content-Type: multipart/related; boundary=\"R\"\r\n\r\n\
             --R\r\nContent-Type: text/html\r\n\r\n\
             <html><body><img src=\"cid:img1@example.com\"></body></html>\r\n\
             --R\r\nContent-Type: image/png\r\nContent-ID: <img1@example.com>\r\n\
             Content-Transfer-Encoding: base64\r\n\r\n{}\r\n\
             --R--\r\n",
            b64(&png)
        );
        let body = parse(&raw);
        let html = body.html.expect("html present");
        assert!(
            html.contains("data:image/png;base64,"),
            "cid should be rewritten to a data URI: {html}"
        );
        assert!(!html.contains("cid:img1"), "cid reference should be gone");
        // The inline image is recorded as an inline attachment with its content-id.
        let inline: Vec<_> = body.attachments.iter().filter(|a| a.is_inline).collect();
        assert_eq!(inline.len(), 1);
        assert_eq!(inline[0].content_id.as_deref(), Some("img1@example.com"));
        assert_eq!(inline[0].mime_type, "image/png");
    }

    #[test]
    fn leaves_over_per_part_cap_image_as_cid() {
        // One byte over the 512 KiB per-part cap.
        let png = vec![0x89u8; 512 * 1024 + 1];
        let raw = format!(
            "Subject: t\r\nMIME-Version: 1.0\r\n\
             Content-Type: multipart/related; boundary=\"R\"\r\n\r\n\
             --R\r\nContent-Type: text/html\r\n\r\n\
             <img src=\"cid:big@x\">\r\n\
             --R\r\nContent-Type: image/png\r\nContent-ID: <big@x>\r\n\
             Content-Transfer-Encoding: base64\r\n\r\n{}\r\n\
             --R--\r\n",
            b64(&png)
        );
        let body = parse(&raw);
        let html = body.html.expect("html present");
        assert!(
            html.contains("cid:big@x"),
            "over-cap image keeps cid: {html}"
        );
        assert!(!html.contains("data:image/png"));
    }

    #[test]
    fn enforces_total_inline_budget() {
        // Five 500 KiB images: 4 fit within the 2 MiB budget, the 5th does not.
        let mut parts = String::new();
        let mut html = String::from("<html><body>");
        let png = vec![0x89u8; 500 * 1024];
        for i in 0..5 {
            html.push_str(&format!("<img src=\"cid:i{i}@x\">"));
            parts.push_str(&format!(
                "--R\r\nContent-Type: image/png\r\nContent-ID: <i{i}@x>\r\n\
                 Content-Transfer-Encoding: base64\r\n\r\n{}\r\n",
                b64(&png)
            ));
        }
        html.push_str("</body></html>");
        let raw = format!(
            "Subject: t\r\nMIME-Version: 1.0\r\n\
             Content-Type: multipart/related; boundary=\"R\"\r\n\r\n\
             --R\r\nContent-Type: text/html\r\n\r\n{html}\r\n{parts}--R--\r\n"
        );
        let body = parse(&raw);
        let out = body.html.expect("html present");
        let inlined = out.matches("data:image/png;base64,").count();
        assert_eq!(inlined, 4, "only four images fit the 2 MiB budget");
        assert!(
            out.contains("cid:i4@x"),
            "the fifth image stays a cid reference"
        );
    }

    #[test]
    fn cid_rewrite_is_case_insensitive_and_literal() {
        assert_eq!(
            replace_ascii_case_insensitive("<img src=CID:Foo@X>", "cid:foo@x", "data:x"),
            "<img src=data:x>"
        );
        // Regex-special characters in the content-id are matched literally.
        assert_eq!(
            replace_ascii_case_insensitive("cid:a.b+c", "cid:a.b+c", "D"),
            "D"
        );
        // No match leaves the string untouched.
        assert_eq!(
            replace_ascii_case_insensitive("nothing here", "cid:x", "D"),
            "nothing here"
        );
    }
}
