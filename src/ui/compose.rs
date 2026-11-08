//! Compose helpers: reply seeding and mailbox-list parsing (pure).

use crate::accounts::AccountPublic;
use crate::wire::{MessageBody, MessageSummary};

/// The initial contents of a compose window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeSeed {
    pub account_id: String,
    pub to_addrs: Vec<String>,
    pub cc_addrs: Vec<String>,
    pub subject: String,
    pub body_text: String,
    pub reply_to_message_id: Option<i64>,
}

impl ComposeSeed {
    /// An empty new message from `account_id`.
    pub fn new_message(account_id: String) -> Self {
        ComposeSeed {
            account_id,
            to_addrs: Vec::new(),
            cc_addrs: Vec::new(),
            subject: String::new(),
            body_text: String::new(),
            reply_to_message_id: None,
        }
    }
}

/// `Re: subject`, adding the prefix only once.
pub fn reply_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("re") {
        if rest.trim_start().starts_with(':') {
            return trimmed.to_string();
        }
    }
    let subject = if trimmed.is_empty() {
        "(no subject)"
    } else {
        trimmed
    };
    format!("Re: {subject}")
}

/// The address a mailbox refers to, lowercased: `Me <ME@x.com>` → `me@x.com`.
pub fn mailbox_key(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(stripped) = trimmed.strip_suffix('>') {
        if let Some(open) = stripped.rfind('<') {
            let inner = &stripped[open + 1..];
            if !inner.contains('<') && !inner.trim().is_empty() {
                return inner.trim().to_lowercase();
            }
        }
    }
    trimmed.to_lowercase()
}

/// Split a typed recipient list on `,`, `;` and newlines, keeping quoted
/// display names (`"Doe, Jane" <jane@x.com>`) and angle-bracket addresses whole.
pub fn split_mailbox_list(value: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut angle_depth = 0u32;

    for ch in value.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quoted {
            current.push(ch);
            escaped = true;
            continue;
        }
        if ch == '"' {
            quoted = !quoted;
        } else if !quoted && ch == '<' {
            angle_depth += 1;
        } else if !quoted && ch == '>' {
            angle_depth = angle_depth.saturating_sub(1);
        }

        if !quoted && angle_depth == 0 && matches!(ch, ',' | ';' | '\n') {
            if !current.trim().is_empty() {
                result.push(current.trim().to_string());
            }
            current.clear();
        } else {
            current.push(ch);
        }
    }
    if !current.trim().is_empty() {
        result.push(current.trim().to_string());
    }
    result
}

fn unique_addresses<'a>(
    addresses: impl IntoIterator<Item = &'a String>,
    excluded: &std::collections::HashSet<String>,
) -> Vec<String> {
    let mut seen = excluded.clone();
    let mut result = Vec::new();
    for value in addresses {
        let address = value.trim();
        let key = mailbox_key(address);
        if !address.is_empty() && seen.insert(key) {
            result.push(address.to_string());
        }
    }
    result
}

/// The compose seed for replying (or replying all) to `message`.
///
/// Replies to the sender, or — for mail the account itself sent — to the
/// original recipients. Reply-all adds the other recipients as Cc. The
/// account's own address is never a recipient, and addresses are deduplicated
/// by mailbox. A text body is quoted under an attribution line.
pub fn reply_seed(
    message: &MessageSummary,
    body: &MessageBody,
    account: &AccountPublic,
    reply_all: bool,
    quoted_date: &str,
) -> ComposeSeed {
    use std::collections::HashSet;

    let sender = message.from_addr.trim().to_string();
    let own = mailbox_key(&account.email);
    let excluded: HashSet<String> = HashSet::from([own.clone()]);
    let sender_is_own = mailbox_key(&sender) == own;
    let original_to = unique_addresses(&body.to_addrs, &excluded);
    let sender_list = if sender.is_empty() {
        Vec::new()
    } else {
        vec![sender.clone()]
    };
    let to_addrs = if sender_is_own {
        original_to
    } else {
        unique_addresses(&sender_list, &excluded)
    };
    let cc_addrs = if reply_all {
        let mut cc_excluded = excluded.clone();
        cc_excluded.extend(to_addrs.iter().map(|a| mailbox_key(a)));
        let source: Vec<&String> = if sender_is_own {
            body.cc_addrs.iter().collect()
        } else {
            body.to_addrs.iter().chain(body.cc_addrs.iter()).collect()
        };
        unique_addresses(source, &cc_excluded)
    } else {
        Vec::new()
    };
    let quoted = match body.text.as_deref() {
        Some(text) if !text.trim().is_empty() => {
            let who = if message.from_name.is_empty() {
                sender.as_str()
            } else {
                message.from_name.as_str()
            };
            let lines: Vec<String> = text.split('\n').map(|l| format!("> {l}")).collect();
            format!("\n\nOn {quoted_date}, {who} wrote:\n{}", lines.join("\n"))
        }
        _ => String::new(),
    };

    ComposeSeed {
        account_id: account.id.clone(),
        to_addrs,
        cc_addrs,
        subject: reply_subject(&message.subject),
        body_text: quoted,
        reply_to_message_id: Some(message.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::AccountKind;

    fn account() -> AccountPublic {
        AccountPublic {
            id: "account".into(),
            email: "me@example.com".into(),
            display_name: "Me".into(),
            kind: AccountKind::Imap,
        }
    }

    fn message() -> MessageSummary {
        MessageSummary {
            id: 7,
            account_id: "account".into(),
            folder_id: 3,
            uid: 42,
            subject: "Status".into(),
            from_name: "Jane Doe".into(),
            from_addr: "jane@example.com".into(),
            date: "2026-07-12T12:00:00Z".into(),
            snippet: String::new(),
            seen: true,
            flagged: false,
            has_attachments: false,
        }
    }

    fn body() -> MessageBody {
        MessageBody {
            id: 7,
            html: None,
            text: Some("Original body".into()),
            to_addrs: vec!["Me <me@example.com>".into(), "other@example.com".into()],
            cc_addrs: vec!["OTHER@example.com".into(), "copy@example.com".into()],
            attachments: Vec::new(),
        }
    }

    #[test]
    fn splits_mailbox_lists_without_breaking_quoted_display_names() {
        assert_eq!(
            split_mailbox_list(
                r#""Doe, Jane" <jane@example.com>, other@example.com; third@example.com"#
            ),
            vec![
                r#""Doe, Jane" <jane@example.com>"#,
                "other@example.com",
                "third@example.com"
            ]
        );
    }

    #[test]
    fn normalizes_formatted_mailboxes_for_exclusion_and_deduplication() {
        assert_eq!(mailbox_key("Me <ME@example.com>"), "me@example.com");
        let seed = reply_seed(&message(), &body(), &account(), true, "then");
        assert_eq!(seed.to_addrs, vec!["jane@example.com"]);
        assert_eq!(seed.cc_addrs, vec!["other@example.com", "copy@example.com"]);
    }

    #[test]
    fn replies_from_sent_mail_target_the_original_recipients() {
        let mut sent = message();
        sent.from_name = "Me".into();
        sent.from_addr = "me@example.com".into();
        let seed = reply_seed(&sent, &body(), &account(), true, "then");
        assert_eq!(seed.to_addrs, vec!["other@example.com"]);
        assert_eq!(seed.cc_addrs, vec!["copy@example.com"]);
    }

    #[test]
    fn adds_one_reply_prefix_and_quotes_the_source_body() {
        assert_eq!(reply_subject("Re: Status"), "Re: Status");
        assert_eq!(reply_subject("RE : Status"), "RE : Status");
        assert_eq!(reply_subject(""), "Re: (no subject)");
        let seed = reply_seed(&message(), &body(), &account(), false, "then");
        assert_eq!(seed.subject, "Re: Status");
        assert!(seed.body_text.ends_with("> Original body"));
        assert!(seed.body_text.contains("On then, Jane Doe wrote:"));
        assert!(seed.cc_addrs.is_empty());
        assert_eq!(seed.reply_to_message_id, Some(7));
    }
}
