//! Small formatting helpers for the UI (no Rune types).

use chrono::{DateTime, Datelike, Local};

/// 1–2 uppercase chars from a display name, falling back to the address's
/// local part.
pub fn initials(name: &str, addr: &str) -> String {
    let local = addr.split('@').next().unwrap_or("");
    let src = if name.trim().is_empty() {
        local
    } else {
        name.trim()
    };
    // Words of letters and digits: "Harry (Gmail)" → "HG", "jane.doe" → "JD".
    let parts: Vec<&str> = src
        .split(|c: char| !c.is_alphanumeric())
        .filter(|p| !p.is_empty())
        .collect();
    let first_char = |s: &str| s.chars().next().map(String::from).unwrap_or_default();
    let out = match parts.as_slice() {
        [a, b, ..] => format!("{}{}", first_char(a), first_char(b)),
        [a] => a.chars().take(2).collect(),
        [] => "?".to_string(),
    };
    out.to_uppercase()
}

/// Palette slots avatars cycle through (`--c2`..`--c6`, `--c12`..`--c14`).
pub const AVATAR_SLOTS: [usize; 8] = [2, 3, 4, 5, 6, 12, 13, 14];

/// Deterministic avatar palette slot for a seed string (a 31-multiplier string
/// hash over UTF-16 code units, as the web UI computed it).
pub fn avatar_slot(seed: &str) -> usize {
    let mut h: u32 = 0;
    for unit in seed.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(u32::from(unit));
    }
    AVATAR_SLOTS[h as usize % AVATAR_SLOTS.len()]
}

/// Display label per shipment carrier code.
pub fn carrier_label(carrier: &str) -> &str {
    match carrier {
        "ups" => "UPS",
        "fedex" => "FedEx",
        "usps" => "USPS",
        "dhl" => "DHL",
        "royal_mail" => "Royal Mail",
        "amazon" => "Amazon",
        other => other,
    }
}

fn parse_date(iso: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(iso)
        .or_else(|_| DateTime::parse_from_rfc2822(iso))
        .ok()
        .map(|d| d.with_timezone(&Local))
}

/// Compact relative date for list rows, relative to `now`.
pub fn relative_date_at(iso: &str, now: DateTime<Local>) -> String {
    let Some(d) = parse_date(iso) else {
        return String::new();
    };
    let diff = now.signed_duration_since(d);
    let mins = diff.num_minutes();
    if diff.num_seconds() < 60 {
        return "now".to_string();
    }
    if mins < 60 {
        return format!("{mins}m");
    }
    if diff.num_hours() < 24 {
        return format!("{}h", diff.num_hours());
    }
    if diff.num_days() < 7 {
        return d.format("%a").to_string();
    }
    if d.year() == now.year() {
        d.format("%b %-d").to_string()
    } else {
        d.format("%b %-d, %y").to_string()
    }
}

/// Compact relative date for list rows.
pub fn relative_date(iso: &str) -> String {
    relative_date_at(iso, Local::now())
}

/// Full timestamp for the reader header.
pub fn full_date(iso: &str) -> String {
    match parse_date(iso) {
        Some(d) => d.format("%a, %b %-d, %Y, %H:%M").to_string(),
        None => iso.to_string(),
    }
}

/// Human-readable byte size, e.g. 1536 → "1.5 KB", 0 → "0 B".
pub fn format_bytes(bytes: f64) -> String {
    if !bytes.is_finite() || bytes <= 0.0 {
        return "0 B".to_string();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let exp = ((bytes.ln() / 1024f64.ln()).floor() as usize).min(UNITS.len() - 1);
    let value = bytes / 1024f64.powi(exp as i32);
    // Whole bytes and round values read better without a trailing ".0".
    let rounded = if exp == 0 {
        value
    } else {
        (value * 10.0).round() / 10.0
    };
    let text = if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded:.1}")
    };
    format!("{text} {}", UNITS[exp])
}

/// Gmail nests its system mailboxes under a literal `[Gmail]/` IMAP path.
/// Strip that prefix for display only — every lookup keeps the real name.
pub fn display_folder_name(name: &str) -> &str {
    name.strip_prefix("[Gmail]/").unwrap_or(name)
}

/// A folder's name as the UI shows it: "Inbox" for the inbox, whatever the server calls it.
pub fn folder_title<'a>(name: &'a str, role: &str) -> &'a str {
    if role == "inbox" {
        "Inbox"
    } else {
        display_folder_name(name)
    }
}

/// The local part of an email address (`harry` for `harry@example.com`).
pub fn short_email(email: &str) -> &str {
    match email.find('@') {
        Some(at) if at > 0 => &email[..at],
        _ => email,
    }
}

/// Badge text: empty for 0, "99+" above 99.
pub fn cap_badge(n: i64) -> String {
    match n {
        n if n > 99 => "99+".to_string(),
        n if n > 0 => n.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn formats_byte_sizes_with_sensible_units_and_precision() {
        assert_eq!(format_bytes(0.0), "0 B");
        assert_eq!(format_bytes(512.0), "512 B");
        assert_eq!(format_bytes(1024.0), "1 KB");
        assert_eq!(format_bytes(1536.0), "1.5 KB");
        assert_eq!(format_bytes(1024.0 * 1024.0), "1 MB");
        assert_eq!(format_bytes(1024.0 * 1024.0 * 1024.0), "1 GB");
        assert_eq!(format_bytes(1024.0 * 1024.0 * 2.55), "2.6 MB");
    }

    #[test]
    fn handles_invalid_or_non_positive_inputs_safely() {
        assert_eq!(format_bytes(-1.0), "0 B");
        assert_eq!(format_bytes(f64::NAN), "0 B");
        assert_eq!(format_bytes(f64::INFINITY), "0 B");
    }

    #[test]
    fn initials_from_names_and_addresses() {
        assert_eq!(initials("Jane Doe", "jane@example.com"), "JD");
        assert_eq!(initials("", "jane.doe@example.com"), "JD");
        assert_eq!(initials("", "harry@example.com"), "HA");
        assert_eq!(initials("  ", ""), "?");
        assert_eq!(initials("Harry (Gmail)", "h@gmail.com"), "HG");
        assert_eq!(initials("", "+++@x.com"), "?");
    }

    #[test]
    fn avatar_slot_is_deterministic_and_in_range() {
        let a = avatar_slot("jane@example.com");
        assert_eq!(a, avatar_slot("jane@example.com"));
        assert!(AVATAR_SLOTS.contains(&a));
        // The hash matches the web UI's: "a" is 97 → 97 % 8 = 1 → slot 3.
        assert_eq!(avatar_slot("a"), 3);
    }

    #[test]
    fn strips_gmail_prefix_for_display() {
        assert_eq!(display_folder_name("[Gmail]/Sent Mail"), "Sent Mail");
        assert_eq!(display_folder_name("INBOX"), "INBOX");
    }

    #[test]
    fn relative_dates() {
        let now = Local.with_ymd_and_hms(2026, 7, 12, 12, 0, 0).unwrap();
        let iso = |d: DateTime<Local>| d.to_rfc3339();
        assert_eq!(relative_date_at(&iso(now), now), "now");
        assert_eq!(
            relative_date_at(&iso(now - chrono::Duration::minutes(5)), now),
            "5m"
        );
        assert_eq!(
            relative_date_at(&iso(now - chrono::Duration::hours(3)), now),
            "3h"
        );
        assert_eq!(
            relative_date_at(&iso(now - chrono::Duration::days(30)), now),
            "Jun 12"
        );
        assert_eq!(
            relative_date_at(&iso(now - chrono::Duration::days(400)), now),
            "Jun 7, 25"
        );
        assert_eq!(relative_date_at("not a date", now), "");
    }

    #[test]
    fn badges_cap_at_99() {
        assert_eq!(cap_badge(0), "");
        assert_eq!(cap_badge(7), "7");
        assert_eq!(cap_badge(100), "99+");
    }
}
