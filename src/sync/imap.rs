//! IMAP connector and per-session operations built on `async-imap` over
//! `tokio-rustls`.
//!
//! Supports two auth mechanisms:
//! - `LOGIN` with a keyring password (plain IMAP accounts), and
//! - SASL `AUTHENTICATE XOAUTH2` (Gmail), via [`async_imap::Authenticator`].

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use async_imap::types::{Fetch, Name};
use async_imap::Session;
use base64::Engine;
use futures::StreamExt;
use mail_parser::decoders::html::html_to_text;
use mail_parser::{Message, MessageParser, MessagePart, MimeHeaders};
use socket2::{SockRef, TcpKeepalive};
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

/// TCP keepalive settings for the IMAP socket (issue #41).
///
/// Without these, a socket that dies silently — laptop suspend/resume, wifi
/// roam, VPN flap, NAT idle-timeout, a server that drops the connection
/// without a FIN/RST — leaves the app sitting in a dead IDLE receiving
/// nothing, which is byte-for-byte indistinguishable from "no mail has
/// arrived" until the (much longer) full-resync deadline in `sync/mod.rs`
/// eventually fires. With keepalive probes enabled, the kernel itself detects
/// the dead peer and the next socket read/write fails with an IO error in
/// roughly `KEEPALIVE_IDLE + KEEPALIVE_INTERVAL * KEEPALIVE_RETRIES` (~90s),
/// which propagates up through `async-imap` and is handled by the ordinary
/// reconnect-with-backoff path.
///
/// ~60s before the first probe (an idling IMAP connection is expected to sit
/// quiet for long stretches — much shorter would fire probes during normal
/// server-side think time), ~10s between probes, 3 probes before the kernel
/// gives up on the connection.
const TCP_KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
const TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const TCP_KEEPALIVE_RETRIES: u32 = 3;

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
    // Debug builds only: trust an extra CA from `COSMIC_MAIL_EXTRA_CA` so E2E
    // tests can point the app at a local IMAPS fixture (GreenMail) serving a
    // committed self-signed `localhost` cert. Never compiled into releases.
    #[cfg(debug_assertions)]
    add_extra_ca(&mut roots);

    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Add certificates from the PEM file named by `COSMIC_MAIL_EXTRA_CA`, if set,
/// to the trust store. Failures are logged and ignored — this is a debug-only
/// testing affordance, never required for the app to run.
#[cfg(debug_assertions)]
fn add_extra_ca(roots: &mut RootCertStore) {
    let Some(path) = std::env::var_os("COSMIC_MAIL_EXTRA_CA") else {
        return;
    };
    let pem = match std::fs::read(&path) {
        Ok(pem) => pem,
        Err(err) => {
            tracing::warn!(error = %err, "COSMIC_MAIL_EXTRA_CA unreadable");
            return;
        }
    };
    let mut added = 0usize;
    for cert in rustls_pemfile::certs(&mut pem.as_slice()).flatten() {
        if roots.add(cert).is_ok() {
            added += 1;
        }
    }
    tracing::warn!(added, "trusting extra CA certificates (debug build)");
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

    // Set TCP keepalive before the TLS handshake so it covers the connection
    // for its entire lifetime, including the handshake itself. `SockRef`
    // borrows the raw fd for the duration of this call only — it does not
    // take ownership of `tcp`, so the stream is unaffected afterwards (see
    // the vendored `socket2::SockRef` docs).
    let keepalive = TcpKeepalive::new()
        .with_time(TCP_KEEPALIVE_IDLE)
        .with_interval(TCP_KEEPALIVE_INTERVAL)
        .with_retries(TCP_KEEPALIVE_RETRIES);
    SockRef::from(&tcp)
        .set_tcp_keepalive(&keepalive)
        .with_context(|| format!("setting TCP keepalive for {host}:{port}"))?;

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

/// Attachment heuristic from BODYSTRUCTURE, used for envelope-only rows
/// before a body has been fetched and parsed for real (the real parse in
/// `store::replace_attachments` corrects `has_attachments` once the body is
/// cached). A part counts as a plausible real attachment when it carries
/// `Content-Disposition: attachment`, or — absent an explicit `inline`
/// disposition and a Content-ID — a `filename`/`name` param (some senders
/// omit `Content-Disposition` entirely on real attachments).
///
/// Deliberately excludes:
/// - `multipart/related` inline images (Content-ID'd parts referenced by
///   `cid:` from the HTML body), identified by a present Content-ID with no
///   explicit `attachment` disposition, regardless of any filename param;
/// - the `text/plain`/`text/html` body parts themselves (no disposition, no
///   Content-ID, no filename param ⇒ excluded by the rule above);
/// - PGP/S-MIME signature parts (`application/pgp-signature`,
///   `application/pkcs7-signature`/`x-pkcs7-signature`), excluded
///   unconditionally since they ride along on every signed message but are
///   never something a user would download as an attachment.
fn bodystructure_has_attachment(fetch: &Fetch) -> bool {
    fetch
        .bodystructure()
        .map(body_structure_has_real_attachment)
        .unwrap_or(false)
}

fn body_structure_has_real_attachment(bs: &async_imap::imap_proto::BodyStructure<'_>) -> bool {
    use async_imap::imap_proto::{
        BodyContentCommon, BodyContentSinglePart, BodyParams, BodyStructure,
    };

    fn is_signature_mime(ty: &str, subtype: &str) -> bool {
        ty.eq_ignore_ascii_case("application")
            && (subtype.eq_ignore_ascii_case("pgp-signature")
                || subtype.eq_ignore_ascii_case("pkcs7-signature")
                || subtype.eq_ignore_ascii_case("x-pkcs7-signature"))
    }

    fn has_filename_param(params: &BodyParams<'_>) -> bool {
        params.as_ref().is_some_and(|list| {
            list.iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("filename") || k.eq_ignore_ascii_case("name"))
        })
    }

    fn is_real_attachment(
        common: &BodyContentCommon<'_>,
        other: &BodyContentSinglePart<'_>,
    ) -> bool {
        if is_signature_mime(&common.ty.ty, &common.ty.subtype) {
            return false;
        }
        let disposition_ty = common
            .disposition
            .as_ref()
            .map(|d| d.ty.to_ascii_lowercase());
        if disposition_ty.as_deref() == Some("attachment") {
            return true;
        }
        if disposition_ty.as_deref() == Some("inline") {
            return false;
        }
        if other.id.is_some() {
            // A Content-ID with no explicit "attachment" disposition is an
            // inline `cid:`-referenced part (multipart/related image), not a
            // listed attachment, even if it also carries a filename param.
            return false;
        }
        has_filename_param(&common.ty.params)
            || common
                .disposition
                .as_ref()
                .is_some_and(|d| has_filename_param(&d.params))
    }

    fn walk(bs: &BodyStructure<'_>) -> bool {
        match bs {
            BodyStructure::Multipart { bodies, .. } => bodies.iter().any(walk),
            BodyStructure::Basic { common, other, .. } => is_real_attachment(common, other),
            BodyStructure::Text { common, other, .. } => is_real_attachment(common, other),
            BodyStructure::Message { common, other, .. } => is_real_attachment(common, other),
        }
    }
    walk(bs)
}

/// Split an IMAP address into (display-name, email).
fn address_parts(addr: &async_imap::imap_proto::Address<'_>) -> (String, String) {
    let name = addr
        .name
        .as_ref()
        .map(|b| unquote_display_name(&decode_mime_words(b)))
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    let email = address_email(addr).unwrap_or_default();
    let name = if name.is_empty() { email.clone() } else { name };
    (name, email)
}

/// Strip lingering RFC 5322 quoted-string syntax from a decoded display name.
///
/// The IMAP ENVELOPE personal-name is sometimes handed back still carrying
/// the quoted-string form from the original `From:` header: either fully
/// wrapped in a pair of literal `"` quotes, or (more commonly, e.g. some
/// marketing senders) missing the real outer quotes but still containing
/// escaped `\"` sequences from the original quoting. In both cases the
/// backslash-escapes (`\"` -> `"`, `\\` -> `\`) need undoing so the name
/// renders as plain text instead of `\"Amazon.co.uk\"`.
fn unquote_display_name(name: &str) -> String {
    fn unescape_quoted_pairs(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
                // A trailing lone backslash has nothing to escape; drop it.
            } else {
                out.push(c);
            }
        }
        out
    }

    if name.len() >= 2 && name.starts_with('"') && name.ends_with('"') {
        return unescape_quoted_pairs(&name[1..name.len() - 1]);
    }
    if name.contains("\\\"") {
        // Unescaping may expose a quoted-string wrapper that was itself
        // escaped (`\"Amazon.co.uk\"` -> `"Amazon.co.uk"`); strip it too.
        let unescaped = unescape_quoted_pairs(name);
        if unescaped.len() >= 2 && unescaped.starts_with('"') && unescaped.ends_with('"') {
            return unescaped[1..unescaped.len() - 1].to_string();
        }
        return unescaped;
    }
    name.to_string()
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

/// Max length (in `char`s) of a generated preview snippet.
const SNIPPET_MAX_CHARS: usize = 160;

/// Compute a single-line preview snippet from a message body.
///
/// Prefers `text` (the text/plain part); when that yields nothing usable,
/// falls back to `html` by converting it to plain text first via
/// [`mail_parser::decoders::html::html_to_text`] (which already drops
/// `<style>`/`<script>`/`<head>` content and decodes entities) before
/// running it through the same cleanup as the text path. Returns `None`
/// when there is nothing to show, so callers can leave a prior snippet
/// untouched (via `COALESCE`) or store an empty snippet.
///
/// Also falls back to `html` (when present) when `text` *looks* machine-glued
/// — some senders' text/plain parts are tag-stripped HTML with no separators
/// left between cells (e.g. Uber receipts: "15 Jul 202620:44Refunded…") — see
/// [`looks_machine_glued`]. Text-only messages (no `html` part) always keep
/// the text-derived snippet regardless of how it looks.
///
/// Whichever source is chosen, if it is itself machine-glued it is run
/// through [`deglue`] before snippeting. This covers the sender class whose
/// HTML is the *same* separator-free table markup the text part was stripped
/// from — `html_to_text` then reproduces the glued text byte for byte, so
/// falling back alone fixes nothing — as well as glued text-only messages.
/// `deglue` is only ever applied to sources [`looks_machine_glued`] flagged:
/// normal prose must never pass through it (it would split brand names like
/// "PayPal" — acceptable collateral inside already-broken glued text, not in
/// prose).
pub fn snippet_for_body(text: Option<&str>, html: Option<&str>) -> Option<String> {
    if let Some(text) = text {
        let glued = looks_machine_glued(text);
        let prefer_text = html.is_none() || !glued;
        if prefer_text {
            let snippet = if glued {
                snippet_from_text(&deglue(text))
            } else {
                snippet_from_text(text)
            };
            if !snippet.is_empty() {
                return Some(snippet);
            }
        }
    }
    let html = html?;
    let converted = html_to_text(html);
    let source = if looks_machine_glued(&converted) {
        deglue(&converted)
    } else {
        converted
    };
    let snippet = snippet_from_text(&source);
    if snippet.is_empty() {
        None
    } else {
        Some(snippet)
    }
}

/// Number of leading characters inspected by [`looks_machine_glued`].
const GLUED_TEXT_SAMPLE_CHARS: usize = 200;
/// Below this many sampled characters there isn't enough signal to judge;
/// never flag short text as glued (avoids false positives on short prose).
const GLUED_TEXT_MIN_SAMPLE_CHARS: usize = 40;
/// Whitespace-to-character ratio below which the sample is considered
/// machine-glued. Ordinary prose runs well above this (roughly one space per
/// 5-6 characters, ratio ~0.15-0.2); tag-stripped machine-generated text with
/// no separators between cells runs far below it. Kept conservative so real
/// prose never trips it.
const GLUED_TEXT_WHITESPACE_RATIO_THRESHOLD: f64 = 0.08;
/// Number of glued boundaries (see [`glue_boundary_count`]) at or above which
/// the sample is considered machine-glued. Real receipts hit several within
/// their first line ("44Refunded", "Refunded15", "44Just"); normal prose
/// almost never accumulates three in 200 chars (an occasional "3pm" or
/// "PayPal" contributes one).
const GLUED_TEXT_BOUNDARY_THRESHOLD: usize = 3;

/// Whether the start of `text` looks like machine-glued, tag-stripped HTML
/// rather than normal prose. Two signals over the first
/// [`GLUED_TEXT_SAMPLE_CHARS`] characters, either of which qualifies:
///
/// - a very low whitespace-to-character ratio (fully glued output with no
///   separators at all), or
/// - several "glued boundaries" — digit→letter, letter→digit, or
///   lowercase→uppercase adjacencies (partially glued output such as Uber
///   receipts: "15 Jul 202620:44Refunded…" keeps normal spaces between many
///   words, so its whitespace ratio looks like prose, but the cell seams
///   leave these transitions behind).
///
/// Conservative by design — only the leading sample is checked, and short
/// text never qualifies.
fn looks_machine_glued(text: &str) -> bool {
    let sample: Vec<char> = text
        .trim_start()
        .chars()
        .take(GLUED_TEXT_SAMPLE_CHARS)
        .collect();
    if sample.len() < GLUED_TEXT_MIN_SAMPLE_CHARS {
        return false;
    }
    let whitespace = sample.iter().filter(|c| c.is_whitespace()).count();
    let ratio = whitespace as f64 / sample.len() as f64;
    ratio < GLUED_TEXT_WHITESPACE_RATIO_THRESHOLD
        || glue_boundary_count(&sample) >= GLUED_TEXT_BOUNDARY_THRESHOLD
}

/// Count adjacencies in `sample` that betray glued cell seams: an ASCII digit
/// immediately followed by an ASCII letter ("44Refunded"), an ASCII letter
/// immediately followed by an ASCII digit ("Refunded15"), or a lowercase
/// ASCII letter immediately followed by an uppercase one ("Street.Total" is
/// not one, but "TotalDue" is).
fn glue_boundary_count(sample: &[char]) -> usize {
    sample
        .windows(2)
        .filter(|pair| {
            let (a, b) = (pair[0], pair[1]);
            (a.is_ascii_digit() && b.is_ascii_alphabetic())
                || (a.is_ascii_alphabetic() && b.is_ascii_digit())
                || (a.is_ascii_lowercase() && b.is_ascii_uppercase())
        })
        .count()
}

/// Insert a single space at cell-seam boundaries in machine-glued text.
///
/// Splits between a character pair `(a, b)` when:
/// - `a` is an ASCII digit and `b` an ASCII **uppercase** letter
///   ("44Refunded" → "44 Refunded", "20:44Just" → "20:44 Just"),
/// - `a` is an ASCII lowercase letter and `b` an ASCII uppercase letter
///   ("appliedPrevious" → "applied Previous"),
/// - `a` is an ASCII letter and `b` an ASCII digit
///   ("Refunded15" → "Refunded 15"),
/// - `a` is one of `.,!?;:` and `b` an ASCII uppercase letter
///   ("Street.Total" → "Street. Total", "applied.To" → "applied. To").
///
/// Deliberately does **not** split digit→lowercase ("3pm" stays "3pm") or
/// digit→digit ("202620:44" — the seam between "2026" and "20:44" — is
/// ambiguous and must stay as-is). Punctuation followed by lowercase is also
/// left alone so domains ("example.com") survive. Only ever called on text
/// that [`looks_machine_glued`] already flagged — never on normal prose,
/// where the lowercase→uppercase rule would split brand names ("PayPal").
fn deglue(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut prev: Option<char> = None;
    for c in text.chars() {
        if let Some(p) = prev {
            let split = (p.is_ascii_digit() && c.is_ascii_uppercase())
                || (p.is_ascii_lowercase() && c.is_ascii_uppercase())
                || (p.is_ascii_alphabetic() && c.is_ascii_digit())
                || (matches!(p, '.' | ',' | '!' | '?' | ';' | ':') && c.is_ascii_uppercase());
            if split {
                out.push(' ');
            }
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// Compute a single-line snippet from a plain-text body (~160 chars).
///
/// Strips invisible preheader padding (literal HTML entities like `&zwnj;`
/// and zero-width Unicode codepoints some senders abuse to pad the inbox
/// preview text — see [`strip_invisible_padding`]), bare and bracket-wrapped
/// (`()`, `[]`, `<>`) URLs — the "alt text + link soup" that marketing
/// `text/plain` parts are usually made of — as well as raw HTML tags (some
/// `text/plain` parts, e.g. The Economist's newsletters, embed literal markup
/// like `<link rel=stylesheet href=...>`) — then collapses whitespace and
/// trims leftover punctuation debris at the edges.
pub fn snippet_from_text(text: &str) -> String {
    let depadded = strip_invisible_padding(text);
    let stripped = strip_urls(&depadded);
    let collapsed: String = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '-' | '\u{2013}'
                    | '\u{2014}'
                    | '|'
                    | '*'
                    | '>'
                    | '<'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '\u{2022}'
                    | '\u{00B7}'
                    | ':'
                    | ';'
                    | ','
            )
    });
    trimmed.chars().take(SNIPPET_MAX_CHARS).collect()
}

/// Named HTML entities some senders abuse as invisible preheader padding
/// (e.g. "A Message From Our Team &zwnj; &zwnj; &zwnj; …" to push the real
/// preview text out of the inbox list), paired with their plain-text
/// replacement. `&nbsp;` becomes a literal space rather than being dropped so
/// the words either side of it don't glue together; the rest are pure
/// padding with nothing to preserve. Full entity decoding is not attempted —
/// only this abused-as-padding set is handled.
const PADDING_ENTITIES: &[(&str, &str)] = &[
    ("&zwnj;", ""),
    ("&zwj;", ""),
    ("&shy;", ""),
    ("&nbsp;", " "),
];

/// Whether `c` is an invisible/zero-width codepoint abused for the same
/// preheader-padding purpose as [`PADDING_ENTITIES`] (e.g. a literal U+034F
/// COMBINING GRAPHEME JOINER dropped between repeated entities).
fn is_invisible_padding_char(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'
            ..='\u{200D}' // zero-width space/non-joiner/joiner
            | '\u{FEFF}'        // zero-width no-break space / BOM
            | '\u{034F}'        // combining grapheme joiner
            | '\u{00AD}' // soft hyphen
    )
}

/// Strip literal HTML-entity and invisible-codepoint preheader padding from
/// `text` before whitespace collapsing. Senders' `text/plain` parts sometimes
/// contain literal entity text (not real HTML, so nothing decodes it
/// upstream) interleaved with invisible Unicode padding characters purely to
/// push real content out of the inbox preview.
fn strip_invisible_padding(text: &str) -> String {
    let mut result = text.to_string();
    for (entity, replacement) in PADDING_ENTITIES {
        result = replace_ascii_case_insensitive(&result, entity, replacement);
    }
    result
        .chars()
        .filter(|c| !is_invisible_padding_char(*c))
        .collect()
}

/// Remove `http(s)://` URLs and raw HTML tags from `text`: bare URL tokens
/// (run to the next whitespace), URLs wrapped in `()`, `[]`, or `<>` (the
/// whole bracketed span, brackets included), and tag-like `<...>` spans
/// (opening, closing, or `<!...>` markers, with or without attributes).
///
/// A `<`/`>` that isn't part of a URL wrap or a tag — e.g. "1 < 2" or "5 > 4"
/// — is left alone; only spans that actually look like markup are dropped.
fn strip_urls(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let closing = match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '<' => Some('>'),
            _ => None,
        };
        if let Some(close) = closing {
            if is_url_start(&chars, i + 1) {
                match chars[i + 1..].iter().position(|&ch| ch == close) {
                    Some(offset) => {
                        i = i + 1 + offset + 1;
                    }
                    None => {
                        // Unbalanced bracket: drop the rest of the string.
                        i = chars.len();
                    }
                }
                continue;
            }
        }
        if c == '<' && is_tag_start(&chars, i + 1) {
            match chars[i + 1..].iter().position(|&ch| ch == '>') {
                Some(offset) => {
                    i = i + 1 + offset + 1;
                }
                None => {
                    // Unterminated tag: drop the rest of the string.
                    i = chars.len();
                }
            }
            continue;
        }
        if is_url_start(&chars, i) {
            let mut j = i;
            while j < chars.len() && !chars[j].is_whitespace() {
                j += 1;
            }
            i = j;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Whether `chars[idx..]` begins with `http://` or `https://`.
fn is_url_start(chars: &[char], idx: usize) -> bool {
    const SCHEMES: [&str; 2] = ["http://", "https://"];
    SCHEMES.iter().any(|scheme| {
        let len = scheme.chars().count();
        idx + len <= chars.len() && chars[idx..idx + len].iter().collect::<String>() == *scheme
    })
}

/// Whether `chars[idx]` looks like the start of an HTML tag name/marker
/// immediately after a `<`: a `/` (closing tag), `!` (comment/doctype), or
/// an ASCII letter (opening tag). Used to distinguish real markup like
/// `<div ...>` / `</p>` from a bare `<` used as "less than" in prose (e.g.
/// "1 < 2" or "<3").
fn is_tag_start(chars: &[char], idx: usize) -> bool {
    chars
        .get(idx)
        .is_some_and(|c| *c == '/' || *c == '!' || c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::{
        body_structure_has_real_attachment, decode_mime_words, deglue, parse_body,
        recent_sequence_set, replace_ascii_case_insensitive, snippet_for_body, snippet_from_text,
        unquote_display_name, BODY_FETCH_QUERY, SNIPPET_MAX_CHARS,
    };
    use async_imap::imap_proto::{
        BodyContentCommon, BodyContentSinglePart, BodyStructure, ContentDisposition,
        ContentEncoding, ContentType,
    };
    use base64::Engine;
    use mail_parser::MessageParser;
    use std::borrow::Cow;

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

    #[test]
    fn unquote_display_name_unescapes_unquoted_amazon_artifact() {
        // No real outer quotes remain, but the escaped inner quotes from the
        // original header survived ENVELOPE parsing — the exact bug report.
        // Unescaping exposes a quote wrapper, which is stripped as well.
        assert_eq!(unquote_display_name("\\\"Amazon.co.uk\\\""), "Amazon.co.uk");
    }

    #[test]
    fn unquote_display_name_keeps_exposed_non_wrapping_quotes() {
        // Unescaping yields interior quotes that do not wrap the whole name;
        // they must be kept, not stripped.
        assert_eq!(unquote_display_name("say \\\"hi\\\" now"), "say \"hi\" now");
    }

    #[test]
    fn unquote_display_name_keeps_lone_backslash_without_escaped_quote() {
        // A bare backslash with no `\"` artifact is not quoted-pair syntax;
        // the name must pass through untouched.
        assert_eq!(unquote_display_name("AC\\DC"), "AC\\DC");
    }

    #[test]
    fn unquote_display_name_strips_plain_quoted_string() {
        assert_eq!(unquote_display_name("\"Plain Name\""), "Plain Name");
    }

    #[test]
    fn unquote_display_name_unescapes_interior_quoted_pair() {
        assert_eq!(
            unquote_display_name("\"He said \\\"hi\\\"\""),
            "He said \"hi\""
        );
    }

    #[test]
    fn unquote_display_name_passes_through_unquoted_plain_name() {
        assert_eq!(unquote_display_name("Jane Doe"), "Jane Doe");
    }

    #[test]
    fn unquote_display_name_handles_lone_quote_and_empty() {
        assert_eq!(unquote_display_name("\""), "\"");
        assert_eq!(unquote_display_name(""), "");
        assert_eq!(unquote_display_name("\"\""), "");
    }

    #[test]
    fn decode_mime_words_still_decodes_encoded_names() {
        assert_eq!(decode_mime_words("=?UTF-8?B?Sm9zw6k=?=".as_bytes()), "José");
        // The unquote step is a no-op for a name with no quoting artifacts.
        assert_eq!(
            unquote_display_name(&decode_mime_words("=?UTF-8?B?Sm9zw6k=?=".as_bytes())),
            "José"
        );
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

    #[test]
    fn snippet_from_text_strips_alt_text_url_soup() {
        // The exact repro from the papercut report: an image alt-text run
        // followed by parenthesised and bracketed tracking links.
        let input = "tick tock backblaze logo (https://link.example.com/xyz) Big Sale! [https://track.example.com/abc?x=1] View in browser";
        assert_eq!(
            snippet_from_text(input),
            "tick tock backblaze logo Big Sale! View in browser"
        );
    }

    #[test]
    fn snippet_from_text_strips_bare_urls() {
        assert_eq!(
            snippet_from_text("Check this out: https://example.com/path?q=1 today"),
            "Check this out: today"
        );
    }

    #[test]
    fn snippet_from_text_strips_angle_wrapped_urls() {
        assert_eq!(
            snippet_from_text("See <https://example.com/x> for details"),
            "See for details"
        );
    }

    #[test]
    fn snippet_from_text_leaves_non_url_brackets_alone() {
        // Only brackets that wrap a URL are removed; ordinary bracketed text
        // (e.g. a parenthetical) must survive untouched.
        assert_eq!(
            snippet_from_text("Order #1234 (thanks!) has shipped"),
            "Order #1234 (thanks!) has shipped"
        );
    }

    #[test]
    fn snippet_from_text_plain_text_is_unchanged() {
        assert_eq!(
            snippet_from_text("Hi team, quick update on the roadmap for next week."),
            "Hi team, quick update on the roadmap for next week."
        );
    }

    #[test]
    fn snippet_from_text_enforces_char_cap() {
        let long = "word ".repeat(100);
        let snippet = snippet_from_text(&long);
        assert_eq!(snippet.chars().count(), SNIPPET_MAX_CHARS);
    }

    #[test]
    fn snippet_from_text_trims_punctuation_debris() {
        assert_eq!(
            snippet_from_text("  -- View in browser (https://example.com/a) --  "),
            "View in browser"
        );
    }

    #[test]
    fn snippet_from_text_strips_economist_style_html_soup() {
        // Real-world repro: The Economist's text/plain part is ~680 lines of
        // whitespace, then a raw <link> tag with attributes, then the real
        // content. The whole tag (including its embedded href URL) must
        // disappear, leaving only the real content.
        let input = "\n\n\n   \n\n <link rel=stylesheet href=https://marber-cdn.economist.com/foundations/latest/css/font-face.css>\nThe Economist July 16th 2026 For subscribers";
        assert_eq!(
            snippet_from_text(input),
            "The Economist July 16th 2026 For subscribers"
        );
    }

    #[test]
    fn snippet_from_text_strips_tags_with_attributes() {
        assert_eq!(
            snippet_from_text("Before <div class=\"foo\" id='bar'> After"),
            "Before After"
        );
    }

    #[test]
    fn snippet_from_text_strips_closing_tags() {
        assert_eq!(snippet_from_text("Hello </p> world </div>"), "Hello world");
    }

    #[test]
    fn snippet_from_text_drops_unterminated_tag_to_end() {
        assert_eq!(
            snippet_from_text("Real content <div class=\"unterminated"),
            "Real content"
        );
    }

    #[test]
    fn snippet_from_text_preserves_non_tag_angle_brackets() {
        assert_eq!(snippet_from_text("1 < 2 and 5 > 4"), "1 < 2 and 5 > 4");
        assert_eq!(
            snippet_from_text("aw <3 that's sweet"),
            "aw <3 that's sweet"
        );
    }

    #[test]
    fn snippet_from_text_still_strips_angle_wrapped_url_alongside_tags() {
        assert_eq!(
            snippet_from_text("See <https://example.com/x> for details <br> ok"),
            "See for details ok"
        );
    }

    #[test]
    fn snippet_for_body_falls_back_to_html_when_no_text_part() {
        let html = "<html><head><style type=\"text/css\">\n\
             body { font-family: Arial; color: #333; }\n\
             .row > .cell { display:flex; }\n\
             </style><script>var x = 1 < 2;</script></head>\n\
             <body><p>Hello team,</p><p>Here&#39;s the update &amp; summary.</p>\n\
             <p>&nbsp;</p></body></html>";
        assert_eq!(
            snippet_for_body(None, Some(html)),
            Some("Hello team, Here's the update & summary.".to_string())
        );
    }

    #[test]
    fn snippet_for_body_prefers_text_over_html() {
        assert_eq!(
            snippet_for_body(Some("Plain text wins"), Some("<p>HTML loses</p>")),
            Some("Plain text wins".to_string())
        );
    }

    #[test]
    fn snippet_for_body_falls_back_to_html_when_text_is_blank() {
        // A text/plain part that collapses to nothing useful (e.g. only
        // whitespace, or only URLs) should still fall back to HTML.
        assert_eq!(
            snippet_for_body(Some("   "), Some("<p>Fallback content</p>")),
            Some("Fallback content".to_string())
        );
    }

    #[test]
    fn snippet_for_body_empty_text_and_html_is_none() {
        assert_eq!(snippet_for_body(None, None), None);
        assert_eq!(snippet_for_body(Some(""), Some("")), None);
        assert_eq!(
            snippet_for_body(Some("   "), Some("<style>a{}</style>")),
            None
        );
    }

    // --- Issue #21: literal entity / invisible-codepoint preheader padding --

    #[test]
    fn snippet_from_text_strips_zwnj_entity_padding() {
        // The exact repro flavor from the papercut report: an image alt-text
        // style preamble padded with repeated literal &zwnj; entities to push
        // the real preview text out of the inbox list.
        let input = "A Message From Our Team &zwnj; &zwnj; &zwnj; &zwnj; Here is the real update for this week.";
        assert_eq!(
            snippet_from_text(input),
            "A Message From Our Team Here is the real update for this week."
        );
    }

    #[test]
    fn snippet_from_text_strips_invisible_codepoints_between_entities() {
        // A literal U+034F COMBINING GRAPHEME JOINER interleaved between
        // repeated padding entities, as seen in the real report.
        let input =
            "A Message From Our Team &zwnj;\u{034F} &zwnj;\u{034F} &zwnj;\u{034F} Real content here";
        assert_eq!(
            snippet_from_text(input),
            "A Message From Our Team Real content here"
        );
    }

    #[test]
    fn snippet_from_text_replaces_nbsp_with_space_not_glue() {
        // &nbsp; must become a space, not disappear, or the words either
        // side of it would glue together.
        assert_eq!(snippet_from_text("Hello&nbsp;World"), "Hello World");
    }

    #[test]
    fn snippet_from_text_strips_other_padding_entities() {
        assert_eq!(
            snippet_from_text("Before &zwj; &shy; After"),
            "Before After"
        );
    }

    #[test]
    fn snippet_from_text_strips_padding_case_insensitively() {
        assert_eq!(snippet_from_text("Hi &ZWNJ; there"), "Hi there");
        assert_eq!(snippet_from_text("Hi&NBSP;there"), "Hi there");
    }

    // --- Issue #20: machine-glued text/plain falls back to HTML ------------

    const GLUED_UBER_STYLE_TEXT: &str = "TripreceiptTotal$24.5015Jul202620:44Refunded15Jul2026,20:44PaymentVisa1234SubtotalCityToCityFare$20.00BookingFee$2.50Tolls$2.00ThanksforridingwithUber";

    /// The real repro from the issue: partially glued — normal spaces remain
    /// between many words (whitespace ratio ~prose level), but the cell seams
    /// leave digit→letter / letter→digit / lower→UPPER boundaries behind
    /// ("44Refunded", "Refunded15", "44Just", "Street.Total£19.54").
    const PARTIALLY_GLUED_UBER_REPRO_TEXT: &str = "15 Jul 202620:44Refunded15 Jul 2026 , 20:44Just a quick update, Harry\nWe adjusted the total for your trip to Hoe Street.Total£19.54 Your refund has been applied.To view your updated receipt, open the app.";

    #[test]
    fn snippet_for_body_falls_back_to_html_for_glued_machine_text() {
        let html =
            "<html><body><p>Trip receipt total is $24.50, refunded on 15 Jul 2026.</p></body></html>";
        assert_eq!(
            snippet_for_body(Some(GLUED_UBER_STYLE_TEXT), Some(html)),
            Some("Trip receipt total is $24.50, refunded on 15 Jul 2026.".to_string())
        );
    }

    #[test]
    fn snippet_for_body_deglues_when_html_fallback_is_equally_glued() {
        // The real Uber case (message row 1119): the sender's HTML is the
        // same separator-free table markup its text part was stripped from,
        // so html_to_text reproduces the glued text byte for byte and the
        // fallback alone is a no-op. The chosen source must be deglued.
        let html = format!(
            "<html><body><table><tr><td>{PARTIALLY_GLUED_UBER_REPRO_TEXT}</td></tr></table></body></html>"
        );
        let snippet = snippet_for_body(Some(PARTIALLY_GLUED_UBER_REPRO_TEXT), Some(&html))
            .expect("snippet present");
        assert!(
            snippet.contains("44 Refunded"),
            "digit→uppercase seam split: {snippet}"
        );
        assert!(
            snippet.contains("Refunded 15"),
            "letter→digit seam split: {snippet}"
        );
        assert!(
            snippet.contains("44 Just"),
            "digit→uppercase seam split: {snippet}"
        );
        assert!(
            !snippet.contains("44Refunded"),
            "glued seam left: {snippet}"
        );
        assert!(!snippet.contains("44Just"), "glued seam left: {snippet}");
    }

    #[test]
    fn snippet_for_body_keeps_normal_prose_text_even_with_html_present() {
        let prose = "Hi team, just a quick update on the roadmap for next week. We shipped the new snippet heuristic and it looks solid.";
        let html = "<html><body><p>HTML alt version, should not be used</p></body></html>";
        assert_eq!(
            snippet_for_body(Some(prose), Some(html)),
            Some(prose.to_string())
        );
    }

    #[test]
    fn snippet_for_body_prose_never_passes_through_deglue() {
        // Brand-style mixed case and times in prose must survive: deglue
        // would split "PayPal", so it must only run on sources flagged as
        // glued — this sentence carries two boundary signals ("yP" in
        // PayPal, "3p" in 3pm), below the three-boundary threshold.
        let prose =
            "Your PayPal payment went through at 3pm as expected, nothing else for you to do.";
        assert_eq!(snippet_for_body(Some(prose), None), Some(prose.to_string()));
    }

    #[test]
    fn snippet_for_body_deglues_glued_text_when_no_html_part_exists() {
        // Text-only messages have nothing to fall back to; the glued text
        // itself is deglued before snippeting.
        let snippet = snippet_for_body(Some(GLUED_UBER_STYLE_TEXT), None).expect("snippet present");
        assert_eq!(snippet, snippet_from_text(&deglue(GLUED_UBER_STYLE_TEXT)));
        assert!(
            snippet.contains("44 Refunded"),
            "seams must be split: {snippet}"
        );
        assert!(
            !snippet.contains("44Refunded"),
            "glued seam left: {snippet}"
        );
    }

    #[test]
    fn deglue_splits_each_boundary_class() {
        // digit → uppercase.
        assert_eq!(deglue("44Refunded"), "44 Refunded");
        assert_eq!(deglue("20:44Just"), "20:44 Just");
        // lowercase → uppercase.
        assert_eq!(deglue("appliedPrevious"), "applied Previous");
        // letter → digit (both cases).
        assert_eq!(deglue("Refunded15"), "Refunded 15");
        assert_eq!(deglue("total19"), "total 19");
        // punctuation → uppercase.
        assert_eq!(deglue("Street.Total"), "Street. Total");
        assert_eq!(deglue("applied.To"), "applied. To");
        assert_eq!(
            deglue("one,Two!Three?Four;Five:Six"),
            "one, Two! Three? Four; Five: Six"
        );
    }

    #[test]
    fn deglue_leaves_ambiguous_pairs_alone() {
        // digit → lowercase: times like "3pm" must stay intact.
        assert_eq!(deglue("at 3pm today"), "at 3pm today");
        // digit → digit: "202620:44" (the "2026"/"20:44" seam) is ambiguous
        // and must stay as-is.
        assert_eq!(deglue("15 Jul 202620:44"), "15 Jul 202620:44");
        // punctuation → lowercase: domains survive.
        assert_eq!(deglue("visit example.com now"), "visit example.com now");
        // uppercase → uppercase: acronyms survive.
        assert_eq!(deglue("HTML and PDF"), "HTML and PDF");
        // Already-spaced text is untouched.
        assert_eq!(deglue("Refunded 15 Jul"), "Refunded 15 Jul");
    }

    // --- Issue #11: tightened BODYSTRUCTURE attachment heuristic -----------

    fn ct(
        ty: &'static str,
        subtype: &'static str,
        params: Option<Vec<(&'static str, &'static str)>>,
    ) -> ContentType<'static> {
        ContentType {
            ty: Cow::Borrowed(ty),
            subtype: Cow::Borrowed(subtype),
            params: params.map(|v| {
                v.into_iter()
                    .map(|(k, val)| (Cow::Borrowed(k), Cow::Borrowed(val)))
                    .collect()
            }),
        }
    }

    fn disp(
        ty: &'static str,
        params: Option<Vec<(&'static str, &'static str)>>,
    ) -> ContentDisposition<'static> {
        ContentDisposition {
            ty: Cow::Borrowed(ty),
            params: params.map(|v| {
                v.into_iter()
                    .map(|(k, val)| (Cow::Borrowed(k), Cow::Borrowed(val)))
                    .collect()
            }),
        }
    }

    fn single_part(content_id: Option<&'static str>) -> BodyContentSinglePart<'static> {
        BodyContentSinglePart {
            id: content_id.map(Cow::Borrowed),
            md5: None,
            description: None,
            transfer_encoding: ContentEncoding::Base64,
            octets: 100,
        }
    }

    fn basic_part(
        ty: &'static str,
        subtype: &'static str,
        disposition: Option<ContentDisposition<'static>>,
        ct_params: Option<Vec<(&'static str, &'static str)>>,
        content_id: Option<&'static str>,
    ) -> BodyStructure<'static> {
        BodyStructure::Basic {
            common: BodyContentCommon {
                ty: ct(ty, subtype, ct_params),
                disposition,
                language: None,
                location: None,
            },
            other: single_part(content_id),
            extension: None,
        }
    }

    fn text_part(subtype: &'static str) -> BodyStructure<'static> {
        BodyStructure::Text {
            common: BodyContentCommon {
                ty: ct("text", subtype, None),
                disposition: None,
                language: None,
                location: None,
            },
            other: single_part(None),
            lines: 10,
            extension: None,
        }
    }

    fn multipart(
        subtype: &'static str,
        bodies: Vec<BodyStructure<'static>>,
    ) -> BodyStructure<'static> {
        BodyStructure::Multipart {
            common: BodyContentCommon {
                ty: ct("multipart", subtype, None),
                disposition: None,
                language: None,
                location: None,
            },
            bodies,
            extension: None,
        }
    }

    #[test]
    fn bodystructure_flags_a_real_listed_attachment() {
        let bs = multipart(
            "mixed",
            vec![
                text_part("plain"),
                basic_part(
                    "application",
                    "pdf",
                    Some(disp("attachment", Some(vec![("filename", "report.pdf")]))),
                    None,
                    None,
                ),
            ],
        );
        assert!(body_structure_has_real_attachment(&bs));
    }

    #[test]
    fn bodystructure_ignores_multipart_related_inline_image() {
        // No explicit disposition, just a Content-ID referenced by the HTML
        // body via cid: — the classic multipart/related embedded image.
        let no_disposition = multipart(
            "related",
            vec![
                text_part("html"),
                basic_part("image", "png", None, None, Some("img1@example.com")),
            ],
        );
        assert!(!body_structure_has_real_attachment(&no_disposition));

        // Explicit `inline` disposition with a filename param must also be
        // excluded (some senders still attach a name to inline images).
        let explicit_inline = multipart(
            "related",
            vec![
                text_part("html"),
                basic_part(
                    "image",
                    "png",
                    Some(disp("inline", Some(vec![("filename", "image001.png")]))),
                    None,
                    Some("img1@example.com"),
                ),
            ],
        );
        assert!(!body_structure_has_real_attachment(&explicit_inline));
    }

    #[test]
    fn bodystructure_ignores_pgp_and_smime_signature_parts() {
        let pgp_signed = multipart(
            "signed",
            vec![
                text_part("plain"),
                // Even with an explicit attachment disposition + filename,
                // a signature part is never a real user-facing attachment.
                basic_part(
                    "application",
                    "pgp-signature",
                    Some(disp(
                        "attachment",
                        Some(vec![("filename", "signature.asc")]),
                    )),
                    None,
                    None,
                ),
            ],
        );
        assert!(!body_structure_has_real_attachment(&pgp_signed));

        let smime_signed = multipart(
            "signed",
            vec![
                text_part("plain"),
                basic_part("application", "pkcs7-signature", None, None, None),
            ],
        );
        assert!(!body_structure_has_real_attachment(&smime_signed));
    }

    #[test]
    fn bodystructure_flags_attachment_missing_content_disposition() {
        // Some senders omit Content-Disposition entirely; a filename/name
        // param with no Content-ID is still a strong real-attachment signal.
        let bs = multipart(
            "mixed",
            vec![
                text_part("plain"),
                basic_part(
                    "application",
                    "zip",
                    None,
                    Some(vec![("name", "archive.zip")]),
                    None,
                ),
            ],
        );
        assert!(body_structure_has_real_attachment(&bs));
    }

    #[test]
    fn bodystructure_plain_text_and_html_body_alone_is_not_an_attachment() {
        let bs = multipart("alternative", vec![text_part("plain"), text_part("html")]);
        assert!(!body_structure_has_real_attachment(&bs));
    }
}
