//! Thunderbird-style mail settings autodiscovery.
//!
//! Given only an email address, [`discover`] resolves IMAP/SMTP connection
//! settings by trying, in order (first hit wins, ~12s total budget):
//!
//! 1. `https://autoconfig.{domain}/mail/config-v1.1.xml?emailaddress={email}`
//! 2. `https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml`
//! 3. Thunderbird ISPDB: `https://autoconfig.thunderbird.net/v1.1/{domain}`
//! 4. MX lookup → registrable domain of the MX host → that provider's own
//!    autoconfig endpoint, then the ISPDB again
//! 5. RFC 6186 SRV records (`_imaps._tcp`, `_submission._tcp`)
//! 6. Heuristic guess `imap.{domain}:993` / `smtp.{domain}:587` (unconfident)
//!
//! Discovery never connects to the mail servers themselves; `add_imap_account`
//! still validates by connecting.

use std::time::Duration;

use hickory_resolver::proto::rr::RData;
use hickory_resolver::TokioResolver;
use serde::Serialize;

/// Per-HTTP-request timeout.
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
/// Overall discovery time budget.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(12);

const USER_AGENT: &str = concat!("CosmicMail/", env!("CARGO_PKG_VERSION"), " (autoconfig)");

/// Where a [`DiscoveredConfig`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiscoverySource {
    /// Provider-hosted autoconfig XML (autoconfig subdomain or `.well-known`).
    Autoconfig,
    /// Thunderbird ISPDB.
    Ispdb,
    /// ISPDB reached via the registrable domain of an MX host.
    Mx,
    /// RFC 6186 DNS SRV records.
    Srv,
    /// Heuristic guess of common defaults.
    Guess,
}

/// Account kind implied by discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiscoveredKind {
    /// Plain IMAP account.
    Imap,
    /// Gmail (frontend should steer the user to Google sign-in).
    Gmail,
}

/// Result of settings discovery. Mirrors the `DiscoveredConfig` wire type.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredConfig {
    /// `gmail` ⇒ frontend steers the user to the Gmail OAuth tab.
    pub kind: DiscoveredKind,
    /// IMAP host (implicit-TLS only — we don't do IMAP STARTTLS yet).
    pub imap_host: String,
    /// IMAP port.
    pub imap_port: u16,
    /// SMTP host.
    pub smtp_host: String,
    /// SMTP port.
    pub smtp_port: u16,
    /// Login username (placeholders already resolved).
    pub username: String,
    /// Where this config came from.
    pub source: DiscoverySource,
    /// `false` ⇒ heuristic guess; the UI must show it as unverified.
    pub confident: bool,
}

/// A parsed server config, before source/kind is decided.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedConfig {
    imap_host: String,
    imap_port: u16,
    smtp_host: String,
    smtp_port: u16,
    username: String,
}

/// An invalid email address (missing `@` or empty local/domain part).
#[derive(Debug)]
pub struct InvalidEmail;

impl std::fmt::Display for InvalidEmail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid email address")
    }
}

impl std::error::Error for InvalidEmail {}

/// Split an email into (local_part, domain), validating both are non-empty and
/// there is exactly one `@`. The domain is lower-cased.
fn split_email(email: &str) -> Result<(String, String), InvalidEmail> {
    let email = email.trim();
    let mut parts = email.splitn(2, '@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    if local.is_empty() || domain.is_empty() || domain.contains('@') || domain.contains(' ') {
        return Err(InvalidEmail);
    }
    Ok((local.to_string(), domain.to_ascii_lowercase()))
}

/// Detect whether an email/MX/host combination is Google-hosted.
///
/// Rules: domain is `gmail.com`/`googlemail.com`, discovered IMAP host is
/// `imap.gmail.com`, or an MX host ends in `.google.com`/`.googlemail.com`.
fn is_gmail(domain: &str, imap_host: Option<&str>, mx_host: Option<&str>) -> bool {
    let domain = domain.to_ascii_lowercase();
    if domain == "gmail.com" || domain == "googlemail.com" {
        return true;
    }
    if let Some(h) = imap_host {
        if h.eq_ignore_ascii_case("imap.gmail.com") {
            return true;
        }
    }
    if let Some(mx) = mx_host {
        let mx = mx.trim_end_matches('.').to_ascii_lowercase();
        if mx.ends_with(".google.com") || mx.ends_with(".googlemail.com") || mx == "google.com" {
            return true;
        }
    }
    false
}

/// Two-part public suffixes we recognise for the registrable-domain heuristic.
/// This is intentionally tiny and best-effort (no PSL crate).
const TWO_PART_SUFFIXES: &[&str] = &[
    "co.uk", "org.uk", "me.uk", "gov.uk", "ac.uk", "co.nz", "com.au", "net.au", "org.au", "co.za",
    "com.br", "co.jp", "co.in", "co.kr",
];

/// Return the registrable domain (eTLD+1) of a host using a last-two-labels
/// heuristic with a small hardcoded list of two-part public suffixes.
fn registrable_domain(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    let last_two = format!("{}.{}", labels[labels.len() - 2], labels[labels.len() - 1]);
    if TWO_PART_SUFFIXES.contains(&last_two.as_str()) && labels.len() >= 3 {
        // eTLD is two labels, so registrable domain is the last three.
        labels[labels.len() - 3..].join(".")
    } else {
        last_two
    }
}

/// Resolve `%EMAILADDRESS%` / `%EMAILLOCALPART%` placeholders.
fn resolve_placeholders(value: &str, email: &str, local_part: &str) -> String {
    value
        .replace("%EMAILADDRESS%", email)
        .replace("%EMAILLOCALPART%", local_part)
}

/// Parse a Thunderbird `config-v1.1.xml` document.
///
/// Returns `None` unless there is at least one `incomingServer type="imap"`
/// with `socketType SSL` (STARTTLS IMAP is unsupported by our connector). The
/// outgoing SMTP server prefers a STARTTLS entry (typically :587) then falls
/// back to an SSL entry (typically :465). Username placeholders are resolved.
fn parse_autoconfig_xml(xml: &str, email: &str) -> Option<ParsedConfig> {
    let (local_part, _domain) = match email.rsplit_once('@') {
        Some((l, d)) if !l.is_empty() && !d.is_empty() => (l, d),
        _ => return None,
    };

    let doc = roxmltree::Document::parse(xml).ok()?;

    // First SSL IMAP incoming server.
    let mut imap: Option<(String, u16, String)> = None;
    for node in doc
        .descendants()
        .filter(|n| n.has_tag_name("incomingServer"))
    {
        if node.attribute("type") != Some("imap") {
            continue;
        }
        if child_text(&node, "socketType").as_deref() != Some("SSL") {
            continue;
        }
        let host = child_text(&node, "hostname")?;
        let port = child_text(&node, "port").and_then(|p| p.parse::<u16>().ok())?;
        let user = child_text(&node, "username").unwrap_or_else(|| "%EMAILADDRESS%".to_string());
        imap = Some((host, port, user));
        break;
    }
    let (imap_host, imap_port, imap_user) = imap?;

    // Outgoing: prefer STARTTLS, then SSL.
    let mut smtp_starttls: Option<(String, u16)> = None;
    let mut smtp_ssl: Option<(String, u16)> = None;
    for node in doc
        .descendants()
        .filter(|n| n.has_tag_name("outgoingServer"))
    {
        if node.attribute("type") != Some("smtp") {
            continue;
        }
        let socket = child_text(&node, "socketType");
        let host = match child_text(&node, "hostname") {
            Some(h) => h,
            None => continue,
        };
        let port = match child_text(&node, "port").and_then(|p| p.parse::<u16>().ok()) {
            Some(p) => p,
            None => continue,
        };
        match socket.as_deref() {
            Some("STARTTLS") if smtp_starttls.is_none() => smtp_starttls = Some((host, port)),
            Some("SSL") if smtp_ssl.is_none() => smtp_ssl = Some((host, port)),
            _ => {}
        }
    }
    let (smtp_host, smtp_port) = smtp_starttls.or(smtp_ssl)?;

    Some(ParsedConfig {
        imap_host,
        imap_port,
        smtp_host,
        smtp_port,
        username: resolve_placeholders(&imap_user, email, local_part),
    })
}

/// Read the trimmed text of the first direct-or-descendant child element with
/// the given tag name under `node`.
fn child_text(node: &roxmltree::Node, tag: &str) -> Option<String> {
    node.children()
        .find(|c| c.has_tag_name(tag))
        .and_then(|c| c.text())
        .map(|t| t.trim().to_string())
        .filter(|s| !s.is_empty())
}

// --- network chain -----------------------------------------------------------

/// Run the full discovery chain for `email`. Errors only on an invalid address;
/// otherwise always returns a config (falling through to an unconfident guess).
pub async fn discover(email: &str) -> Result<DiscoveredConfig, InvalidEmail> {
    let (local_part, domain) = split_email(email)?;
    let email = email.trim().to_string();

    let result = tokio::time::timeout(
        TOTAL_TIMEOUT,
        run_chain(email.clone(), local_part, domain.clone()),
    )
    .await;

    Ok(match result {
        Ok(Some(cfg)) => cfg,
        Ok(None) | Err(_) => guess(&domain, &email),
    })
}

/// The ordered discovery steps. Returns `None` if nothing matched (caller falls
/// back to a guess).
async fn run_chain(email: String, _local_part: String, domain: String) -> Option<DiscoveredConfig> {
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .ok()?;

    // 1. Provider autoconfig subdomain.
    let url = format!(
        "https://autoconfig.{}/mail/config-v1.1.xml?emailaddress={}",
        domain,
        urlencode(&email)
    );
    if let Some(p) = fetch_and_parse(&client, &url, &email).await {
        return Some(finalize(p, &domain, DiscoverySource::Autoconfig, None));
    }

    // 2. `.well-known`.
    let url = format!(
        "https://{}/.well-known/autoconfig/mail/config-v1.1.xml",
        domain
    );
    if let Some(p) = fetch_and_parse(&client, &url, &email).await {
        return Some(finalize(p, &domain, DiscoverySource::Autoconfig, None));
    }

    // 3. Thunderbird ISPDB.
    if let Some(p) = fetch_ispdb(&client, &domain, &email).await {
        return Some(finalize(p, &domain, DiscoverySource::Ispdb, None));
    }

    // 4. MX → registrable domain → provider autoconfig, then ISPDB.
    if let Some(mx_host) = mx_lookup(&domain).await {
        // Google-hosted domains often only reveal themselves via MX.
        if is_gmail(&domain, None, Some(&mx_host)) {
            return Some(gmail_config(&email));
        }
        let reg = registrable_domain(&mx_host);
        if !reg.is_empty() && reg != domain {
            // The mail provider's own autoconfig server frequently serves
            // configs for hosted custom domains (e.g. Purelymail), like
            // Thunderbird's MX step does.
            let url = format!(
                "https://autoconfig.{}/mail/config-v1.1.xml?emailaddress={}",
                reg,
                urlencode(&email)
            );
            if let Some(p) = fetch_and_parse(&client, &url, &email).await {
                return Some(finalize(p, &domain, DiscoverySource::Mx, Some(&mx_host)));
            }
            if let Some(p) = fetch_ispdb(&client, &reg, &email).await {
                return Some(finalize(p, &domain, DiscoverySource::Mx, Some(&mx_host)));
            }
        }
    }

    // 5. RFC 6186 SRV.
    if let Some(p) = srv_lookup(&domain, &email).await {
        return Some(finalize(p, &domain, DiscoverySource::Srv, None));
    }

    None
}

/// Fetch an autoconfig XML URL and parse it.
async fn fetch_and_parse(client: &reqwest::Client, url: &str, email: &str) -> Option<ParsedConfig> {
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().await.ok()?;
    parse_autoconfig_xml(&body, email)
}

/// Query the Thunderbird ISPDB for a domain.
async fn fetch_ispdb(client: &reqwest::Client, domain: &str, email: &str) -> Option<ParsedConfig> {
    let url = format!("https://autoconfig.thunderbird.net/v1.1/{}", domain);
    fetch_and_parse(client, &url, email).await
}

/// Build a `TokioResolver` from system configuration.
fn resolver() -> Option<TokioResolver> {
    TokioResolver::builder_tokio().ok()?.build().ok()
}

/// Look up the lowest-preference MX host for a domain.
async fn mx_lookup(domain: &str) -> Option<String> {
    let resolver = resolver()?;
    let lookup = resolver.mx_lookup(domain).await.ok()?;
    let mut best: Option<(u16, String)> = None;
    for record in lookup.answers() {
        if let RData::MX(mx) = &record.data {
            let host = mx.exchange.to_utf8();
            let host = host.trim_end_matches('.').to_string();
            if host.is_empty() {
                continue;
            }
            match &best {
                Some((pref, _)) if *pref <= mx.preference => {}
                _ => best = Some((mx.preference, host)),
            }
        }
    }
    best.map(|(_, host)| host)
}

/// RFC 6186 SRV discovery for IMAPS + submission. Ignores targets of "."
/// ("service not offered") and picks the lowest-priority record.
async fn srv_lookup(domain: &str, email: &str) -> Option<ParsedConfig> {
    let resolver = resolver()?;

    let imap = srv_pick(&resolver, &format!("_imaps._tcp.{}", domain)).await?;
    let smtp = srv_pick(&resolver, &format!("_submission._tcp.{}", domain)).await?;

    Some(ParsedConfig {
        imap_host: imap.0,
        imap_port: imap.1,
        smtp_host: smtp.0,
        smtp_port: smtp.1,
        // SRV doesn't tell us the username; default to the full address.
        username: email.to_string(),
    })
}

/// Resolve a single SRV name to (host, port), lowest priority wins.
async fn srv_pick(resolver: &TokioResolver, name: &str) -> Option<(String, u16)> {
    let lookup = resolver.srv_lookup(name).await.ok()?;
    let mut best: Option<(u16, String, u16)> = None;
    for record in lookup.answers() {
        if let RData::SRV(srv) = &record.data {
            let target = srv.target.to_utf8();
            let target = target.trim_end_matches('.').to_string();
            // RFC 6186: a target of "." means the service is not offered.
            if target.is_empty() {
                continue;
            }
            match &best {
                Some((prio, _, _)) if *prio <= srv.priority => {}
                _ => best = Some((srv.priority, target, srv.port)),
            }
        }
    }
    best.map(|(_, host, port)| (host, port))
}

/// Turn a `ParsedConfig` into a `DiscoveredConfig`, applying Gmail detection.
fn finalize(
    p: ParsedConfig,
    domain: &str,
    source: DiscoverySource,
    mx_host: Option<&str>,
) -> DiscoveredConfig {
    if is_gmail(domain, Some(&p.imap_host), mx_host) {
        return DiscoveredConfig {
            kind: DiscoveredKind::Gmail,
            source,
            confident: true,
            ..gmail_fields(&p.username)
        };
    }
    DiscoveredConfig {
        kind: DiscoveredKind::Imap,
        imap_host: p.imap_host,
        imap_port: p.imap_port,
        smtp_host: p.smtp_host,
        smtp_port: p.smtp_port,
        username: p.username,
        source,
        confident: true,
    }
}

/// Gmail server fields (used when detection triggers).
fn gmail_fields(username: &str) -> DiscoveredConfig {
    DiscoveredConfig {
        kind: DiscoveredKind::Gmail,
        imap_host: "imap.gmail.com".to_string(),
        imap_port: 993,
        smtp_host: "smtp.gmail.com".to_string(),
        smtp_port: 587,
        username: username.to_string(),
        source: DiscoverySource::Mx,
        confident: true,
    }
}

/// A confident Gmail result keyed off the email address.
fn gmail_config(email: &str) -> DiscoveredConfig {
    gmail_fields(email)
}

/// Heuristic fallback guess.
fn guess(domain: &str, email: &str) -> DiscoveredConfig {
    if is_gmail(domain, None, None) {
        return DiscoveredConfig {
            source: DiscoverySource::Guess,
            confident: true,
            ..gmail_fields(email)
        };
    }
    DiscoveredConfig {
        kind: DiscoveredKind::Imap,
        imap_host: format!("imap.{}", domain),
        imap_port: 993,
        smtp_host: format!("smtp.{}", domain),
        smtp_port: 587,
        username: email.to_string(),
        source: DiscoverySource::Guess,
        confident: false,
    }
}

/// Minimal percent-encoding for the query-string email (encodes `@` and a few
/// reserved chars; addresses rarely contain anything else).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FASTMAIL_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
  <emailProvider id="fastmail.com">
    <domain>fastmail.com</domain>
    <displayName>Fastmail</displayName>
    <incomingServer type="imap">
      <hostname>imap.fastmail.com</hostname>
      <port>993</port>
      <socketType>SSL</socketType>
      <authentication>password-cleartext</authentication>
      <username>%EMAILADDRESS%</username>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.fastmail.com</hostname>
      <port>465</port>
      <socketType>SSL</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>password-cleartext</authentication>
    </outgoingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.fastmail.com</hostname>
      <port>587</port>
      <socketType>STARTTLS</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>password-cleartext</authentication>
    </outgoingServer>
  </emailProvider>
</clientConfig>"#;

    // IMAP is STARTTLS-only here → must be rejected (parse yields None).
    const STARTTLS_ONLY_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
  <emailProvider id="example.com">
    <domain>example.com</domain>
    <incomingServer type="imap">
      <hostname>imap.example.com</hostname>
      <port>143</port>
      <socketType>STARTTLS</socketType>
      <username>%EMAILLOCALPART%</username>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.example.com</hostname>
      <port>587</port>
      <socketType>STARTTLS</socketType>
      <username>%EMAILADDRESS%</username>
    </outgoingServer>
  </emailProvider>
</clientConfig>"#;

    #[test]
    fn parses_fastmail_prefers_starttls_smtp() {
        let cfg = parse_autoconfig_xml(FASTMAIL_XML, "jane@fastmail.com").unwrap();
        assert_eq!(cfg.imap_host, "imap.fastmail.com");
        assert_eq!(cfg.imap_port, 993);
        // STARTTLS/587 preferred over SSL/465.
        assert_eq!(cfg.smtp_host, "smtp.fastmail.com");
        assert_eq!(cfg.smtp_port, 587);
        // %EMAILADDRESS% resolved.
        assert_eq!(cfg.username, "jane@fastmail.com");
    }

    #[test]
    fn rejects_starttls_only_imap() {
        assert!(parse_autoconfig_xml(STARTTLS_ONLY_XML, "bob@example.com").is_none());
    }

    #[test]
    fn falls_back_to_ssl_smtp_when_no_starttls() {
        let xml = r#"<clientConfig version="1.1"><emailProvider id="x">
          <incomingServer type="imap"><hostname>imap.x.com</hostname><port>993</port>
            <socketType>SSL</socketType><username>%EMAILLOCALPART%</username></incomingServer>
          <outgoingServer type="smtp"><hostname>smtp.x.com</hostname><port>465</port>
            <socketType>SSL</socketType><username>%EMAILADDRESS%</username></outgoingServer>
        </emailProvider></clientConfig>"#;
        let cfg = parse_autoconfig_xml(xml, "sam@x.com").unwrap();
        assert_eq!(cfg.smtp_port, 465);
        // %EMAILLOCALPART% resolved.
        assert_eq!(cfg.username, "sam");
    }

    #[test]
    fn placeholder_resolution() {
        assert_eq!(
            resolve_placeholders("%EMAILADDRESS%", "a@b.com", "a"),
            "a@b.com"
        );
        assert_eq!(
            resolve_placeholders("%EMAILLOCALPART%", "a@b.com", "a"),
            "a"
        );
        assert_eq!(resolve_placeholders("literal", "a@b.com", "a"), "literal");
    }

    #[test]
    fn registrable_domain_heuristic() {
        assert_eq!(registrable_domain("mx.example.com"), "example.com");
        assert_eq!(registrable_domain("example.com"), "example.com");
        assert_eq!(registrable_domain("aspmx.l.google.com"), "google.com");
        // Two-part suffix.
        assert_eq!(
            registrable_domain("mx1.mail.example.co.uk"),
            "example.co.uk"
        );
        assert_eq!(registrable_domain("example.co.uk"), "example.co.uk");
        assert_eq!(registrable_domain("mail.example.com.au"), "example.com.au");
    }

    #[test]
    fn gmail_detection() {
        assert!(is_gmail("gmail.com", None, None));
        assert!(is_gmail("googlemail.com", None, None));
        assert!(is_gmail("mydomain.com", Some("imap.gmail.com"), None));
        assert!(is_gmail("mydomain.com", None, Some("aspmx.l.google.com.")));
        assert!(is_gmail(
            "mydomain.com",
            None,
            Some("alt1.gmail-smtp-in.l.google.com")
        ));
        assert!(!is_gmail("mydomain.com", Some("imap.mydomain.com"), None));
        assert!(!is_gmail("mydomain.com", None, Some("mx.mydomain.com")));
    }

    #[test]
    fn split_email_validation() {
        assert!(split_email("a@b.com").is_ok());
        assert!(split_email("  a@b.com  ").is_ok());
        assert!(split_email("").is_err());
        assert!(split_email("noatsign").is_err());
        assert!(split_email("@b.com").is_err());
        assert!(split_email("a@").is_err());
        assert!(split_email("a@b@c").is_err());
    }

    #[test]
    fn guess_is_unconfident() {
        let g = guess("example.com", "me@example.com");
        assert_eq!(g.source, DiscoverySource::Guess);
        assert!(!g.confident);
        assert_eq!(g.imap_host, "imap.example.com");
        assert_eq!(g.smtp_host, "smtp.example.com");
        assert_eq!(g.imap_port, 993);
        assert_eq!(g.smtp_port, 587);
    }

    #[test]
    fn guess_gmail_domain() {
        let g = guess("gmail.com", "me@gmail.com");
        assert!(matches!(g.kind, DiscoveredKind::Gmail));
        assert_eq!(g.imap_host, "imap.gmail.com");
    }

    fn live_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .unwrap()
    }

    /// Live network test against the real Thunderbird ISPDB. Run manually:
    /// `cargo test --lib autoconfig::tests -- --ignored`.
    /// Note: fastmail.com is NOT in the ISPDB (they self-host autoconfig, see
    /// the test below), so this uses gmx.de, a long-standing ISPDB entry.
    #[tokio::test]
    #[ignore = "hits the live Thunderbird ISPDB"]
    async fn ispdb_gmx_live() {
        let cfg = fetch_ispdb(&live_client(), "gmx.de", "jane@gmx.de")
            .await
            .expect("ISPDB should return a config for gmx.de");
        assert_eq!(cfg.imap_host, "imap.gmx.net");
        assert_eq!(cfg.imap_port, 993);
    }

    /// Live full-chain test for a custom domain hosted by Purelymail: steps
    /// 1-3 miss (the site serves an HTML catch-all on `.well-known`, and
    /// neither domain is in the ISPDB), so this exercises the MX → provider
    /// autoconfig path. Run manually with `-- --ignored`.
    #[tokio::test]
    #[ignore = "hits live DNS and autoconfig.purelymail.com"]
    async fn mx_provider_autoconfig_purelymail_live() {
        let cfg = discover("harry@pluscosmic.dev")
            .await
            .expect("valid email address");
        assert_eq!(cfg.imap_host, "imap.purelymail.com");
        assert_eq!(cfg.imap_port, 993);
        assert_eq!(cfg.smtp_host, "smtp.purelymail.com");
        assert_eq!(cfg.username, "harry@pluscosmic.dev");
        assert_eq!(cfg.source, DiscoverySource::Mx);
        assert!(cfg.confident);
    }

    /// Live network test against Fastmail's self-hosted provider autoconfig
    /// (chain step 1). Run manually with `-- --ignored`.
    #[tokio::test]
    #[ignore = "hits Fastmail's live autoconfig endpoint"]
    async fn provider_autoconfig_fastmail_live() {
        let url =
            "https://autoconfig.fastmail.com/mail/config-v1.1.xml?emailaddress=jane@fastmail.com";
        let cfg = fetch_and_parse(&live_client(), url, "jane@fastmail.com")
            .await
            .expect("Fastmail autoconfig should return a config");
        assert_eq!(cfg.imap_host, "imap.fastmail.com");
        assert_eq!(cfg.imap_port, 993);
        assert!(cfg.smtp_host.contains("fastmail.com"));
    }
}
