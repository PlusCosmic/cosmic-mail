//! Shipment / tracking-number extraction from cached message bodies.
//!
//! Pure, dependency-free heuristics — no network calls, no new crates, no
//! I/O. Given the parsed text/html body of a message (the same parts already
//! produced by `sync::imap::parse_body`), [`extract_shipments`] returns zero
//! or more detected shipments. A false positive here means a bogus shipment
//! card on an unrelated email, which is worse than missing a real one, so
//! every heuristic below is deliberately conservative:
//!
//! - Formats with a published check digit (UPS `1Z`, and the UPU S10 postal
//!   standard used by Royal Mail and USPS's 13-character international
//!   format) are only kept when the checksum actually validates. A
//!   format-shaped number with a failing checksum is dropped outright, not
//!   downgraded to a guess.
//! - Formats with no checksum (FedEx 12/15/20-digit, DHL 10-digit) are only
//!   kept when a shipping-related keyword (or the carrier's own name) appears
//!   within a small window of the match. Bare numbers of these lengths are
//!   common false positives (phone numbers, invoice numbers, order numbers).
//! - A 20/22-digit numeric token is ambiguous between USPS and FedEx by
//!   shape alone. A Mod-10 checksum alone is not enough evidence either — it
//!   passes about 1 in 10 random numbers of that length, and 20/22-digit
//!   reference numbers (bank references, account numbers, GUID-ish ids) do
//!   appear in emails — so it is only treated as deterministic USPS when the
//!   checksum validates *and* the token starts with `9`, matching every real
//!   USPS IMpb service-type prefix (92/93/94/95/96/...) the issue calls out.
//!   Anything else at this length (checksum-invalid, or a valid checksum
//!   without the `9` prefix) falls back to requiring the carrier's own name
//!   nearby, same as the other unchecksummed formats.
//! - Tracking links in the body disambiguate the carrier for free and are
//!   preferred over a guessed carrier when both are present.
//!
//! The UPS and USPS Mod-10 variants and the S10 checksum below were verified
//! by hand against published worked examples before being encoded here (see
//! the module tests) — crate-free reimplementations of small check-digit
//! algorithms are exactly the kind of thing that silently rots if copied
//! from a half-remembered description.

use std::collections::HashSet;

/// A carrier or marketplace a shipment was attributed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carrier {
    Ups,
    FedEx,
    Usps,
    Dhl,
    RoyalMail,
    Amazon,
}

impl Carrier {
    /// Stable lowercase code stored in the database and sent over the wire.
    /// The frontend maps this to a display label/glyph (see `lib/format.ts`),
    /// mirroring how `FolderRole` is handled.
    pub fn as_str(self) -> &'static str {
        match self {
            Carrier::Ups => "ups",
            Carrier::FedEx => "fedex",
            Carrier::Usps => "usps",
            Carrier::Dhl => "dhl",
            Carrier::RoyalMail => "royal_mail",
            Carrier::Amazon => "amazon",
        }
    }

    pub fn from_db_str(s: &str) -> Option<Carrier> {
        match s {
            "ups" => Some(Carrier::Ups),
            "fedex" => Some(Carrier::FedEx),
            "usps" => Some(Carrier::Usps),
            "dhl" => Some(Carrier::Dhl),
            "royal_mail" => Some(Carrier::RoyalMail),
            "amazon" => Some(Carrier::Amazon),
            _ => None,
        }
    }

    /// Public carrier tracking-page URL template, used as a fallback when
    /// the email itself carried no tracking link.
    fn fallback_url(self, tracking_number: &str) -> Option<String> {
        match self {
            Carrier::Ups => Some(format!(
                "https://www.ups.com/track?tracknum={tracking_number}"
            )),
            Carrier::FedEx => Some(format!(
                "https://www.fedex.com/fedextrack/?trknbr={tracking_number}"
            )),
            Carrier::Usps => Some(format!(
                "https://tools.usps.com/go/TrackConfirmAction?tLabels={tracking_number}"
            )),
            Carrier::Dhl => Some(format!(
                "https://www.dhl.com/global-en/home/tracking.html?tracking-id={tracking_number}"
            )),
            Carrier::RoyalMail => Some(format!(
                "https://www.royalmail.com/track-your-item#/tracking-results/{tracking_number}"
            )),
            // Amazon tracking pages require an authenticated session tied to
            // the specific order; there is no generic public template.
            Carrier::Amazon => None,
        }
    }
}

/// A shipment detected in a message body, ready to persist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedShipment {
    pub carrier: Carrier,
    pub tracking_number: Option<String>,
    /// Captured from the email when present, otherwise a synthesized
    /// carrier tracking-page URL built from `tracking_number` (never set for
    /// Amazon, which has no generic public tracking URL).
    pub tracking_url: Option<String>,
    pub order_id: Option<String>,
}

/// How many characters of surrounding (lowercased) body text count as
/// "nearby" when looking for shipping context around a candidate number.
const CONTEXT_WINDOW: usize = 120;

const CONTEXT_KEYWORDS: &[&str] = &[
    "tracking",
    "track your",
    "track my",
    "track this",
    "shipment",
    "shipped",
    "has shipped",
    "on its way",
    "out for delivery",
    "delivery",
    "dispatched",
    "courier",
    "parcel",
    "package",
    "consignment",
    "carrier",
];

/// Carrier name keywords, used both as part of the generic context check and
/// (alone) to disambiguate the genuinely-ambiguous 20/22-digit numeric case.
const CARRIER_KEYWORDS: &[(&str, Carrier)] = &[
    ("ups", Carrier::Ups),
    ("united parcel service", Carrier::Ups),
    ("fedex", Carrier::FedEx),
    ("federal express", Carrier::FedEx),
    ("usps", Carrier::Usps),
    ("postal service", Carrier::Usps),
    ("dhl", Carrier::Dhl),
    ("royal mail", Carrier::RoyalMail),
    ("amazon", Carrier::Amazon),
];

/// Detect shipments in a message's text and/or HTML body. Either part may be
/// absent (mirrors `FetchedBody`); both are scanned when present since some
/// senders only populate one.
pub fn extract_shipments(text: Option<&str>, html: Option<&str>) -> Vec<ExtractedShipment> {
    let mut combined = String::new();
    if let Some(t) = text {
        combined.push_str(t);
        combined.push('\n');
    }
    if let Some(h) = html {
        combined.push_str(&strip_html_tags(h));
        combined.push('\n');
    }
    if combined.trim().is_empty() {
        return Vec::new();
    }

    let chars: Vec<char> = combined.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let lower_all: String = lower.iter().collect();

    // Links: hrefs from the raw HTML (case preserved for the captured URL),
    // plus bare URLs anywhere in the combined visible text.
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut links: Vec<(Carrier, String)> = Vec::new();
    if let Some(h) = html {
        for url in extract_hrefs(h) {
            if let Some(c) = carrier_for_url(&url) {
                if seen_urls.insert(url.clone()) {
                    links.push((c, url));
                }
            }
        }
    }
    for url in extract_bare_urls(&combined) {
        if let Some(c) = carrier_for_url(&url) {
            if seen_urls.insert(url.clone()) {
                links.push((c, url));
            }
        }
    }

    let numbers = find_number_candidates(&chars, &lower);
    let order_ids = find_amazon_order_ids(&chars);
    let mentions_amazon = lower_all.contains("amazon");

    let mut shipments: Vec<ExtractedShipment> = Vec::new();
    let mut used = vec![false; numbers.len()];

    // 1. Checksum-verified numbers are certain. Attach a same-carrier link
    // when one is present; drop the (rare) format-shaped-but-invalid match
    // outright rather than falling back to a guess.
    for (i, nm) in numbers.iter().enumerate() {
        if !nm.deterministic {
            continue;
        }
        used[i] = true;
        if !nm.checksum_valid {
            continue;
        }
        let captured = links
            .iter()
            .find(|(c, _)| *c == nm.carrier)
            .map(|(_, u)| u.clone());
        push_unique(
            &mut shipments,
            ExtractedShipment {
                carrier: nm.carrier,
                tracking_number: Some(nm.number.clone()),
                tracking_url: resolve_tracking_url(nm.carrier, &nm.number, captured),
                order_id: None,
            },
        );
    }

    // 2. Context-gated guesses (no checksum exists, or none was available
    // for the format) — but only when step 1 found no checksum-verified
    // shipment for this message at all. A generic shipping keyword (e.g.
    // "delivery", "parcel") is trivially present throughout a real shipping
    // email, so once one tracking number is *certain*, a second, weaker,
    // co-occurring number-shaped token is almost always something else in
    // the same email (an order id, a transaction/reference number, ...) —
    // exactly the "prefer missing over bogus" case, not a second shipment.
    // A candidate is kept when either a specific carrier-name match or a
    // generic shipping keyword is found nearby (either is sufficient on its
    // own). When no specific carrier-name evidence exists for the number's
    // own guessed carrier, an unrelated single link in the message is
    // allowed to relabel the carrier instead (it disambiguates "for free").
    if shipments.is_empty() {
        for (i, nm) in numbers.iter().enumerate() {
            if used[i] {
                continue;
            }
            let specific = specific_carrier_keyword_near(&lower, nm.start, nm.end, nm.carrier);
            let generic = generic_keyword_near(&lower, nm.start, nm.end);
            if !specific && !generic {
                continue;
            }
            let carrier = if !specific {
                links.first().map(|(c, _)| *c).unwrap_or(nm.carrier)
            } else {
                nm.carrier
            };
            let captured = links
                .iter()
                .find(|(c, _)| *c == carrier)
                .map(|(_, u)| u.clone());
            push_unique(
                &mut shipments,
                ExtractedShipment {
                    carrier,
                    tracking_number: Some(nm.number.clone()),
                    tracking_url: resolve_tracking_url(carrier, &nm.number, captured),
                    order_id: None,
                },
            );
        }
    }

    // 3. Links with no attached number become their own (number-less)
    // shipment — this is the common Amazon shape, but applies to any
    // carrier whose email only carries a tracking link.
    for (carrier, url) in &links {
        if *carrier == Carrier::Amazon {
            continue; // handled below, together with any order id
        }
        if shipments.iter().any(|s| s.carrier == *carrier) {
            continue;
        }
        push_unique(
            &mut shipments,
            ExtractedShipment {
                carrier: *carrier,
                tracking_number: None,
                tracking_url: Some(url.clone()),
                order_id: None,
            },
        );
    }

    // 4. Amazon: link and/or order id, gated on the message actually
    // mentioning Amazon (an order-id-shaped number is specific enough on its
    // own, but not so specific that it's worth surfacing out of context).
    if mentions_amazon {
        let amazon_link = links
            .iter()
            .find(|(c, _)| *c == Carrier::Amazon)
            .map(|(_, u)| u.clone());
        let order_id = order_ids.first().map(|(id, ..)| id.clone());
        if amazon_link.is_some() || order_id.is_some() {
            push_unique(
                &mut shipments,
                ExtractedShipment {
                    carrier: Carrier::Amazon,
                    tracking_number: None,
                    tracking_url: amazon_link,
                    order_id,
                },
            );
        }
    }

    shipments
}

fn push_unique(shipments: &mut Vec<ExtractedShipment>, s: ExtractedShipment) {
    if !shipments.contains(&s) {
        shipments.push(s);
    }
}

/// Final tracking URL for a shipment: whatever the email itself carried, or
/// a synthesized carrier tracking-page URL from the number.
fn resolve_tracking_url(
    carrier: Carrier,
    tracking_number: &str,
    captured: Option<String>,
) -> Option<String> {
    captured.or_else(|| carrier.fallback_url(tracking_number))
}

// --- Number-format candidates -------------------------------------------------

#[derive(Debug, Clone)]
struct NumberCandidate {
    carrier: Carrier,
    number: String,
    /// True for formats with a defined check digit (UPS, S10).
    deterministic: bool,
    checksum_valid: bool,
    /// Char indices (in the combined body) the token spans, for context lookup.
    start: usize,
    end: usize,
}

/// Scan the combined body for maximal alphanumeric runs and classify each
/// one against the known carrier tracking-number shapes.
fn find_number_candidates(chars: &[char], lower: &[char]) -> Vec<NumberCandidate> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_alphanumeric() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_alphanumeric() {
            i += 1;
        }
        let end = i;
        let token: String = chars[start..end].iter().collect();
        if let Some(cand) = classify_token(&token, start, end, lower) {
            out.push(cand);
        }
    }
    out
}

fn classify_token(
    token: &str,
    start: usize,
    end: usize,
    lower: &[char],
) -> Option<NumberCandidate> {
    let len = token.chars().count();
    let upper = token.to_ascii_uppercase();

    // UPS: "1Z" + 16 alphanumeric (18 total), Mod-10 check digit.
    if len == 18 && upper.starts_with("1Z") {
        let valid = ups_checksum_valid(&upper);
        return Some(NumberCandidate {
            carrier: Carrier::Ups,
            number: upper,
            deterministic: true,
            checksum_valid: valid,
            start,
            end,
        });
    }

    // S10 (UPU standard): 2 letters + 9 digits + 2-letter country code.
    // Royal Mail uses "GB"; USPS's international 13-char format uses "US".
    if len == 13 {
        let cs: Vec<char> = upper.chars().collect();
        let shape_ok = cs[0].is_ascii_alphabetic()
            && cs[1].is_ascii_alphabetic()
            && cs[2..11].iter().all(|c| c.is_ascii_digit())
            && cs[11].is_ascii_alphabetic()
            && cs[12].is_ascii_alphabetic();
        if shape_ok {
            let suffix: String = cs[11..13].iter().collect();
            let carrier = match suffix.as_str() {
                "GB" => Some(Carrier::RoyalMail),
                "US" => Some(Carrier::Usps),
                _ => None,
            };
            if let Some(carrier) = carrier {
                let serial: String = cs[2..10].iter().collect();
                let check_digit = cs[10].to_digit(10);
                let valid = check_digit.is_some_and(|d| s10_checksum_valid(&serial, d));
                return Some(NumberCandidate {
                    carrier,
                    number: upper,
                    deterministic: true,
                    checksum_valid: valid,
                    start,
                    end,
                });
            }
        }
        return None;
    }

    if token.bytes().all(|b| b.is_ascii_digit()) {
        match len {
            10 => {
                return Some(NumberCandidate {
                    carrier: Carrier::Dhl,
                    number: token.to_string(),
                    deterministic: false,
                    checksum_valid: false,
                    start,
                    end,
                });
            }
            12 | 15 => {
                return Some(NumberCandidate {
                    carrier: Carrier::FedEx,
                    number: token.to_string(),
                    deterministic: false,
                    checksum_valid: false,
                    start,
                    end,
                });
            }
            20 | 22 => {
                // A Mod-10 check alone passes ~10% of random 20/22-digit
                // numbers, and reference numbers of this length (bank
                // references, account numbers, GUID-ish ids) do turn up in
                // emails — so the checksum by itself is not enough evidence.
                // Real USPS IMpb numbers always start with a service-type
                // digit of 9 (92/93/94/95/96/...), which the issue calls
                // out explicitly; require both before treating this as
                // deterministic.
                let valid = usps_mod10_valid(token) && token.starts_with('9');
                if valid {
                    return Some(NumberCandidate {
                        carrier: Carrier::Usps,
                        number: token.to_string(),
                        deterministic: true,
                        checksum_valid: true,
                        start,
                        end,
                    });
                }
                // Ambiguous with FedEx's own 20-digit format at this exact
                // length (and, absent the '9' prefix, with plain reference
                // numbers too); only keep it when the carrier's own name is
                // unambiguously nearby (checked directly here rather than
                // via the generic context pass, since a generic shipping
                // keyword alone can't tell the two carriers apart).
                let near = window_text(lower, start, end, CONTEXT_WINDOW);
                let fedex_near = contains_any(&near, &["fedex", "federal express"]);
                let usps_near = contains_any(&near, &["usps", "postal service"]);
                let carrier = match (fedex_near, usps_near) {
                    (true, false) => Some(Carrier::FedEx),
                    (false, true) => Some(Carrier::Usps),
                    _ => None, // neither, or both — genuinely ambiguous
                };
                if let Some(carrier) = carrier {
                    return Some(NumberCandidate {
                        carrier,
                        number: token.to_string(),
                        deterministic: false,
                        checksum_valid: false,
                        start,
                        end,
                    });
                }
                return None;
            }
            _ => {}
        }
    }
    None
}

// --- Context / keyword windows -----------------------------------------------

/// Lowercased text within `radius` characters of `[start, end)`.
fn window_text(lower: &[char], start: usize, end: usize, radius: usize) -> String {
    let s = start.saturating_sub(radius);
    let e = (end + radius).min(lower.len());
    lower[s..e].iter().collect()
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn generic_keyword_near(lower: &[char], start: usize, end: usize) -> bool {
    let w = window_text(lower, start, end, CONTEXT_WINDOW);
    contains_any(&w, CONTEXT_KEYWORDS)
}

fn specific_carrier_keyword_near(
    lower: &[char],
    start: usize,
    end: usize,
    carrier: Carrier,
) -> bool {
    let w = window_text(lower, start, end, CONTEXT_WINDOW);
    CARRIER_KEYWORDS
        .iter()
        .any(|(kw, c)| *c == carrier && w.contains(kw))
}

// --- Checksums -----------------------------------------------------------------

/// UPS `1Z` tracking-number Mod-10 check digit: letters map to
/// `(ascii_value - 3) % 10` (A=2 .. Z=7, matching UPS's published
/// conversion table), each of the 15 body characters is multiplied by 1
/// (even index) or 2 (odd index) with no digit-sum reduction, and the check
/// digit is `(10 - sum % 10) % 10`. Verified by hand against
/// `1Z5R89390357567127` and the widely-published example
/// `1Z999AA10123456784` (see tests).
fn ups_checksum_valid(upper18: &str) -> bool {
    let chars: Vec<char> = upper18.chars().collect();
    if chars.len() != 18 {
        return false;
    }
    let body = &chars[2..18]; // 15 serial chars + 1 check digit
    let (serial, check) = body.split_at(15);
    let Some(check_digit) = check[0].to_digit(10) else {
        return false;
    };
    let mut sum: u32 = 0;
    for (i, &c) in serial.iter().enumerate() {
        let value = if let Some(d) = c.to_digit(10) {
            d
        } else if c.is_ascii_uppercase() {
            (c as u32 - 3) % 10
        } else {
            return false;
        };
        let mult = if i % 2 == 0 { 1 } else { 2 };
        sum += value * mult;
    }
    let rem = sum % 10;
    let expected = if rem == 0 { 0 } else { 10 - rem };
    expected == check_digit
}

/// UPU S10 postal standard check digit: weights `[8,6,4,2,3,5,9,7]` over the
/// 8 serial digits, `sum % 11`, then check = 0 when the remainder is 1, 5
/// when it is 0, otherwise `11 - remainder`. Verified by hand against the
/// standard's own worked example (serial `47312482` -> check digit `9`).
fn s10_checksum_valid(serial8: &str, check_digit: u32) -> bool {
    const WEIGHTS: [u32; 8] = [8, 6, 4, 2, 3, 5, 9, 7];
    let digits: Vec<u32> = serial8.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != 8 {
        return false;
    }
    let sum: u32 = digits.iter().zip(WEIGHTS.iter()).map(|(d, w)| d * w).sum();
    let rem = sum % 11;
    let expected = match rem {
        1 => 0,
        0 => 5,
        r => 11 - r,
    };
    expected == check_digit
}

/// USPS Mod-10 check digit: weights 3 (even index) / 1 (odd index)
/// alternating across the serial digits from the left, no digit-sum
/// reduction, check = `(10 - sum % 10) % 10`. USPS's own reference
/// implementation processes the 22-digit format's serial in reverse before
/// weighting, but that's a no-op here: both the 20-digit format's 19-digit
/// serial and the 22-digit format's 21-digit serial are odd-length, and
/// reversing an odd-length sequence preserves every position's index
/// parity, so reversed-and-then-left-weighted assigns ×3 to exactly the
/// same digits as left-weighted-without-reversing. Verified by hand for
/// both lengths (see tests) before simplifying away the reversal.
fn usps_mod10_valid(all_digits: &str) -> bool {
    let chars: Vec<char> = all_digits.chars().collect();
    if chars.len() < 2 {
        return false;
    }
    let (serial, check) = chars.split_at(chars.len() - 1);
    let Some(check_digit) = check[0].to_digit(10) else {
        return false;
    };
    let mut sum: u32 = 0;
    for (i, c) in serial.iter().enumerate() {
        let Some(d) = c.to_digit(10) else {
            return false;
        };
        let mult = if i % 2 == 0 { 3 } else { 1 };
        sum += d * mult;
    }
    let rem = sum % 10;
    let expected = if rem == 0 { 0 } else { 10 - rem };
    expected == check_digit
}

// --- HTML / link scanning -------------------------------------------------------

/// Strip HTML tags to recover visible text for tokenizing/keyword search
/// (link hrefs are extracted separately, from the raw markup). Not a real
/// parser — a linear tag-stripper is enough for local heuristic scanning.
fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                // A removed tag must not glue adjacent inline content
                // together — e.g. "...952</p><a href=...>Change</a>" would
                // otherwise merge into one bogus "952Change" alphanumeric
                // token, silently hiding a real tracking number that
                // happens to sit right against another element. Collapsed
                // below, so this never adds visible double-spacing.
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let decoded = out
        .replace("&amp;", "&")
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    // Collapse after entity decoding so an `&nbsp;` sitting right against a
    // tag-boundary space (a common real-world pattern) doesn't leave a
    // double space behind.
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Extract `href="..."` / `href='...'` attribute values from raw HTML.
///
/// Scans a byte-aligned ASCII-lowercased copy for the `href=` marker (ASCII
/// lowercasing never changes byte length or alignment, so offsets found in
/// it are valid, char-boundary-safe offsets into the original string too),
/// then slices the value out of the original so its case is preserved.
fn extract_hrefs(html: &str) -> Vec<String> {
    let hay = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = hay[search_from..].find("href=") {
        let pos = search_from + rel + 5;
        let Some(&quote_byte) = html.as_bytes().get(pos) else {
            break;
        };
        let quote = match quote_byte {
            b'"' => '"',
            b'\'' => '\'',
            _ => {
                search_from = pos;
                continue;
            }
        };
        let val_start = pos + 1;
        match html[val_start..].find(quote) {
            Some(rel_end) => {
                let val_end = val_start + rel_end;
                out.push(html[val_start..val_end].to_string());
                search_from = val_end + 1;
            }
            None => break,
        }
    }
    out
}

/// Extract bare `http://`/`https://` URLs from plain text.
fn extract_bare_urls(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for scheme in ["https://", "http://"] {
        let mut search_from = 0usize;
        while let Some(rel) = lower[search_from..].find(scheme) {
            let start = search_from + rel;
            let mut end = start;
            while end < bytes.len() {
                let b = bytes[end];
                if b.is_ascii_whitespace()
                    || matches!(b, b'"' | b'\'' | b'<' | b'>' | b')' | b']' | b'}')
                {
                    break;
                }
                end += 1;
            }
            let mut trimmed_end = end;
            while trimmed_end > start
                && matches!(bytes[trimmed_end - 1], b'.' | b',' | b';' | b':' | b'!')
            {
                trimmed_end -= 1;
            }
            if trimmed_end > start {
                out.push(text[start..trimmed_end].to_string());
            }
            search_from = end.max(start + 1);
        }
    }
    out
}

/// Map a URL to a carrier via the tracking-page host/path substrings called
/// out in the issue: ups.com/track, fedex.com/fedextrack, USPS's
/// TrackConfirmAction, dhl.com tracking URLs, royalmail.com/track-your-item,
/// and Amazon's tracking/progress-tracker links.
fn carrier_for_url(url: &str) -> Option<Carrier> {
    let lower = url.to_ascii_lowercase();
    if lower.contains("ups.com") && lower.contains("track") {
        Some(Carrier::Ups)
    } else if lower.contains("fedex.com") && lower.contains("track") {
        Some(Carrier::FedEx)
    } else if lower.contains("usps.com") && lower.contains("track") {
        Some(Carrier::Usps)
    } else if lower.contains("dhl.com") && lower.contains("track") {
        Some(Carrier::Dhl)
    } else if lower.contains("royalmail.com") && lower.contains("track") {
        Some(Carrier::RoyalMail)
    } else if lower.contains("amazon.")
        && (lower.contains("track")
            || lower.contains("progress-tracker")
            || lower.contains("ship-track"))
    {
        Some(Carrier::Amazon)
    } else {
        None
    }
}

/// Extracted Amazon order id (`\d{3}-\d{7}-\d{7}`) occurrences, with the char
/// span so a keyword-proximity check could be added later if needed.
fn find_amazon_order_ids(chars: &[char]) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i + 19 <= n {
        if is_digit_run(chars, i, 3)
            && chars[i + 3] == '-'
            && is_digit_run(chars, i + 4, 7)
            && chars[i + 11] == '-'
            && is_digit_run(chars, i + 12, 7)
        {
            let before_ok = i == 0 || !chars[i - 1].is_ascii_alphanumeric();
            let after_ok = i + 19 >= n || !chars[i + 19].is_ascii_alphanumeric();
            if before_ok && after_ok {
                let s: String = chars[i..i + 19].iter().collect();
                out.push((s, i, i + 19));
                i += 19;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn is_digit_run(chars: &[char], start: usize, len: usize) -> bool {
    start + len <= chars.len() && chars[start..start + len].iter().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Checksum unit tests, hand-verified against published/self-derived worked examples ---

    #[test]
    fn ups_checksum_accepts_known_valid_numbers() {
        // Hand-verified against this module's own algorithm description.
        assert!(ups_checksum_valid("1Z5R89390357567127"));
        // The widely-published canonical UPS example number.
        assert!(ups_checksum_valid("1Z999AA10123456784"));
        // Self-derived: serial "T3STSHIP1234567" -> check digit 9 (see module docs).
        assert!(ups_checksum_valid("1ZT3STSHIP12345679"));
    }

    #[test]
    fn ups_checksum_rejects_corrupted_check_digit() {
        assert!(!ups_checksum_valid("1Z5R89390357567120"));
        assert!(!ups_checksum_valid("1Z999AA10123456780"));
    }

    #[test]
    fn ups_checksum_rejects_wrong_length() {
        // The "1Z" prefix itself is the caller's responsibility
        // (`classify_token` checks it before calling); this only covers the
        // checksum function's own length guard.
        assert!(!ups_checksum_valid("1Z5R8939035756712")); // 17 chars
        assert!(!ups_checksum_valid("1Z5R893903575671277")); // 19 chars
    }

    #[test]
    fn s10_checksum_matches_standard_worked_example() {
        // UPU S10 standard's own worked example: serial 47312482 -> check digit 9.
        assert!(s10_checksum_valid("47312482", 9));
        assert!(!s10_checksum_valid("47312482", 0));
    }

    #[test]
    fn usps_mod10_matches_hand_derived_vectors() {
        // Self-derived 20-digit: serial "1234567890123456789" -> check 0.
        assert!(usps_mod10_valid("12345678901234567890"));
        assert!(!usps_mod10_valid("12345678901234567891"));
        // Self-derived 22-digit: serial "123456789012345678901" -> check 7.
        assert!(usps_mod10_valid("1234567890123456789017"));
        assert!(!usps_mod10_valid("1234567890123456789010"));
        // Self-derived 22-digit, '9'-prefixed serial "940000000000000000000" -> check 9
        // (the shape a real USPS IMpb number has; see the extraction-level tests below).
        assert!(usps_mod10_valid("9400000000000000000009"));
    }

    #[test]
    fn checksum_check_digit_is_unique_per_serial() {
        // Regression safety net independent of hand arithmetic: for a fixed
        // serial, exactly one of the ten possible trailing digits validates.
        let serial_prefix = "1Z999AA1012345678"; // 17 chars: "1Z" + 15-char serial
        let valid_digits: Vec<u32> = (0..10)
            .filter(|d| ups_checksum_valid(&format!("{serial_prefix}{d}")))
            .collect();
        assert_eq!(valid_digits, vec![4]);

        let s10_valid: Vec<u32> = (0..10)
            .filter(|&d| s10_checksum_valid("47312482", d))
            .collect();
        assert_eq!(s10_valid, vec![9]);
    }

    // --- Extraction integration tests over realistic email-body fixtures ---

    #[test]
    fn detects_ups_number_and_link_together() {
        let text = "Your package is on the way!\n\
                     Tracking number: 1Z999AA10123456784\n\
                     Track your shipment: https://www.ups.com/track?trackingNumber=1Z999AA10123456784";
        let shipments = extract_shipments(Some(text), None);
        assert_eq!(shipments.len(), 1);
        assert_eq!(shipments[0].carrier, Carrier::Ups);
        assert_eq!(
            shipments[0].tracking_number.as_deref(),
            Some("1Z999AA10123456784")
        );
        assert!(shipments[0]
            .tracking_url
            .as_deref()
            .unwrap()
            .contains("ups.com"));
    }

    #[test]
    fn ups_email_with_transaction_reference_number_yields_only_the_ups_shipment() {
        // Modeled on a real "Schedule Delivery Update" UPS email that
        // previously produced a bogus second "shipment": a checksum-valid
        // UPS 1Z number (real one, hand-verified) plus a 10-digit
        // "Transaction Reference Number" sitting in an HTML table, in a
        // message saturated with generic shipping keywords ("parcel",
        // "delivery") that used to be enough on their own to let the
        // reference number through as a guessed DHL/relabeled-UPS shipment.
        let html = "<p>Your parcel delivery has been scheduled.</p>\
                     <p>Tracking Number: 1Z17X3X96850653952</p>\
                     <a href=\"https://www.ups.com/track?loc=en_US&tracknum=1Z17X3X96850653952&requester=ST/trackdetails\">Change delivery</a>\
                     <table><tr><td>Transaction Reference Number:</td><td>4066509478</td></tr></table>\
                     <p>Thank you for shipping with UPS. Your package delivery is on its way.</p>";
        let shipments = extract_shipments(None, Some(html));
        assert_eq!(
            shipments.len(),
            1,
            "a co-occurring reference number must not surface as a second shipment: {shipments:?}"
        );
        assert_eq!(shipments[0].carrier, Carrier::Ups);
        assert_eq!(
            shipments[0].tracking_number.as_deref(),
            Some("1Z17X3X96850653952")
        );
    }

    #[test]
    fn ups_shaped_number_with_bad_checksum_is_dropped_even_with_context() {
        let text = "Your UPS tracking number is 1Z999AA10123456780 — track your shipment now.";
        let shipments = extract_shipments(Some(text), None);
        assert!(
            shipments.is_empty(),
            "invalid UPS checksum must not surface a shipment: {shipments:?}"
        );
    }

    #[test]
    fn detects_fedex_number_with_context_and_link() {
        let text = "Good news — your order has shipped via FedEx.\n\
                     Tracking Number: 799340540586\n\
                     Track it here: https://www.fedex.com/fedextrack/?trknbr=799340540586";
        let shipments = extract_shipments(Some(text), None);
        assert_eq!(shipments.len(), 1);
        assert_eq!(shipments[0].carrier, Carrier::FedEx);
        assert_eq!(
            shipments[0].tracking_number.as_deref(),
            Some("799340540586")
        );
        assert_eq!(
            shipments[0].tracking_url.as_deref(),
            Some("https://www.fedex.com/fedextrack/?trknbr=799340540586")
        );
    }

    #[test]
    fn bare_12_digit_number_without_shipping_context_is_ignored() {
        let text = "Your invoice reference is 799340540586. Please retain this for your records.";
        let shipments = extract_shipments(Some(text), None);
        assert!(
            shipments.is_empty(),
            "an unrelated 12-digit number must not become a shipment: {shipments:?}"
        );
    }

    #[test]
    fn bare_10_digit_number_reads_as_a_phone_number_not_dhl() {
        let text = "Questions about your reservation? Call us at 5551234567 any time.";
        let shipments = extract_shipments(Some(text), None);
        assert!(
            shipments.is_empty(),
            "a 10-digit phone number must not become a DHL shipment: {shipments:?}"
        );
    }

    #[test]
    fn detects_dhl_number_with_context() {
        let text = "Your parcel has been dispatched with DHL. Tracking: 1234567890.";
        let shipments = extract_shipments(Some(text), None);
        assert_eq!(shipments.len(), 1);
        assert_eq!(shipments[0].carrier, Carrier::Dhl);
        assert_eq!(shipments[0].tracking_number.as_deref(), Some("1234567890"));
    }

    #[test]
    fn detects_usps_22_digit_via_checksum_without_needing_context() {
        // Self-derived 22-digit valid number that also has the real-world
        // '9'-prefixed USPS IMpb shape; no shipping keyword anywhere near
        // it, on purpose — the checksum (plus prefix) alone must be enough
        // for a deterministic format.
        let text = "Reference code for your records: 9400000000000000000009.";
        let shipments = extract_shipments(Some(text), None);
        assert_eq!(shipments.len(), 1);
        assert_eq!(shipments[0].carrier, Carrier::Usps);
        assert_eq!(
            shipments[0].tracking_number.as_deref(),
            Some("9400000000000000000009")
        );
    }

    #[test]
    fn usps_shaped_valid_checksum_without_nine_prefix_is_dropped_without_context() {
        // Same self-derived 22-digit number as the checksum unit test above,
        // and its Mod-10 check digit genuinely validates — but it does not
        // start with '9', so it does not have the shape any real USPS IMpb
        // number has. A checksum alone is not enough evidence at this
        // length (~1 in 10 random 22-digit numbers would also pass), so
        // this must be dropped rather than surfaced as a USPS shipment.
        let text = "Reference code for your records: 1234567890123456789017.";
        assert!(
            extract_shipments(Some(text), None).is_empty(),
            "a valid-checksum but non-9-prefixed number must not become a bogus USPS shipment"
        );
    }

    #[test]
    fn ambiguous_20_digit_number_needs_a_specific_carrier_name_not_a_generic_word() {
        // Fails the USPS checksum, and "shipment" alone can't disambiguate
        // USPS from FedEx at this exact length — must be dropped.
        let text = "Your shipment reference is 12345678901234567891.";
        assert!(extract_shipments(Some(text), None).is_empty());

        // Naming FedEx specifically resolves the ambiguity.
        let text = "Your FedEx shipment reference is 12345678901234567891.";
        let shipments = extract_shipments(Some(text), None);
        assert_eq!(shipments.len(), 1);
        assert_eq!(shipments[0].carrier, Carrier::FedEx);
    }

    #[test]
    fn detects_royal_mail_s10_via_checksum() {
        // Self-derived S10 vector: serial 47312482 -> check digit 9, GB suffix.
        let html = "<p>Your item RR473124829GB has been dispatched.</p>\
                     <a href=\"https://www.royalmail.com/track-your-item#/tracking-results/RR473124829GB\">Track</a>";
        let shipments = extract_shipments(None, Some(html));
        assert_eq!(shipments.len(), 1);
        assert_eq!(shipments[0].carrier, Carrier::RoyalMail);
        assert_eq!(
            shipments[0].tracking_number.as_deref(),
            Some("RR473124829GB")
        );
        assert!(shipments[0]
            .tracking_url
            .as_deref()
            .unwrap()
            .contains("royalmail.com"));
    }

    #[test]
    fn detects_usps_13_char_s10_via_checksum() {
        // Self-derived S10 vector: serial 12345670 -> check digit 6, US suffix.
        let text = "Your international item EC123456706US is on its way.";
        let shipments = extract_shipments(Some(text), None);
        assert_eq!(shipments.len(), 1);
        assert_eq!(shipments[0].carrier, Carrier::Usps);
        assert_eq!(
            shipments[0].tracking_number.as_deref(),
            Some("EC123456706US")
        );
    }

    #[test]
    fn amazon_link_without_tracking_number_captures_link_and_order_id() {
        let html = "<p>Hi, your Amazon.com order #123-4567890-1234567 has shipped.</p>\
                     <a href=\"https://www.amazon.com/progress-tracker/package/ref=abc\">Track package</a>";
        let shipments = extract_shipments(None, Some(html));
        assert_eq!(shipments.len(), 1);
        assert_eq!(shipments[0].carrier, Carrier::Amazon);
        assert_eq!(shipments[0].tracking_number, None);
        assert_eq!(
            shipments[0].order_id.as_deref(),
            Some("123-4567890-1234567")
        );
        assert!(shipments[0]
            .tracking_url
            .as_deref()
            .unwrap()
            .contains("progress-tracker"));
    }

    #[test]
    fn amazon_order_id_shape_without_amazon_mention_is_ignored() {
        let text = "Your reference number is 123-4567890-1234567 for this transaction.";
        let shipments = extract_shipments(Some(text), None);
        assert!(
            shipments.is_empty(),
            "order-id-shaped number needs an Amazon mention: {shipments:?}"
        );
    }

    #[test]
    fn link_carrier_is_preferred_over_ambiguous_number_guess() {
        // A bare 15-digit number (FedEx-shaped by length) with only a
        // generic "shipped" keyword nearby, but the only link in the message
        // points at DHL's tracking page — the link should win.
        let html = "<p>Your order has shipped. Reference: 123456789012345.</p>\
                     <a href=\"https://www.dhl.com/us-en/home/tracking.html?tracking-id=123\">Track with DHL</a>";
        let shipments = extract_shipments(None, Some(html));
        assert!(shipments.iter().any(|s| s.carrier == Carrier::Dhl
            && s.tracking_number.as_deref() == Some("123456789012345")));
    }

    #[test]
    fn no_body_yields_no_shipments() {
        assert!(extract_shipments(None, None).is_empty());
        assert!(extract_shipments(Some(""), Some("")).is_empty());
        assert!(extract_shipments(
            Some("just a normal email with no shipping content at all"),
            None
        )
        .is_empty());
    }

    #[test]
    fn extract_hrefs_finds_quoted_attribute_values() {
        let html = r#"<a href="https://example.com/a">x</a><a href='https://example.com/b'>y</a>"#;
        let hrefs = extract_hrefs(html);
        assert_eq!(
            hrefs,
            vec!["https://example.com/a", "https://example.com/b"]
        );
    }

    #[test]
    fn extract_bare_urls_trims_trailing_punctuation() {
        let text = "See https://example.com/track?x=1, or https://example.com/y.";
        let urls = extract_bare_urls(text);
        assert_eq!(
            urls,
            vec!["https://example.com/track?x=1", "https://example.com/y"]
        );
    }

    #[test]
    fn strip_html_tags_keeps_text_and_decodes_common_entities() {
        let html = "<p>Order &amp; shipment <b>update</b>&nbsp;here</p>";
        assert_eq!(strip_html_tags(html), "Order & shipment update here");
    }
}
