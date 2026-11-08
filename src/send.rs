//! Plain-text message construction and authenticated SMTP submission.

use anyhow::{bail, Context, Result};
use lettre::message::{header::ContentType, Mailbox};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::accounts::{self, Account, AccountKind};
use crate::wire::SendMessageInput;

/// Build and submit a message. Credentials are resolved only after all user input validates.
pub async fn send(
    account: &Account,
    input: &SendMessageInput,
    reply_message_id: Option<&str>,
) -> Result<()> {
    let message = build_message(account, input, reply_message_id)?;
    let (credentials, mechanisms) = match account.kind {
        AccountKind::Imap => (
            Credentials::new(
                account.username.clone(),
                accounts::get_imap_password(&account.id)?,
            ),
            vec![Mechanism::Plain, Mechanism::Login],
        ),
        AccountKind::Gmail => (
            Credentials::new(
                account.username.clone(),
                crate::auth::oauth::access_token(&account.id)
                    .await
                    .context("obtaining Gmail access token")?,
            ),
            vec![Mechanism::Xoauth2],
        ),
    };

    let builder = if account.smtp_port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&account.smtp_host)
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&account.smtp_host)
    }
    .with_context(|| format!("configuring SMTP server {}", account.smtp_host))?;
    let mailer = builder
        .port(account.smtp_port)
        .credentials(credentials)
        .authentication(mechanisms)
        .build();

    mailer
        .send(message)
        .await
        .with_context(|| format!("sending message through {}", account.smtp_host))?;
    Ok(())
}

fn build_message(
    account: &Account,
    input: &SendMessageInput,
    reply_message_id: Option<&str>,
) -> Result<Message> {
    let from_address = account
        .email
        .parse()
        .context("parsing the sending account address")?;
    let from_name = (!account.display_name.trim().is_empty()).then(|| account.display_name.clone());
    let mut builder = Message::builder()
        .from(Mailbox::new(from_name, from_address))
        .subject(input.subject.clone());

    let mut recipient_count = 0;
    for raw in &input.to_addrs {
        builder = builder.to(parse_recipient(raw)?);
        recipient_count += 1;
    }
    for raw in &input.cc_addrs {
        builder = builder.cc(parse_recipient(raw)?);
        recipient_count += 1;
    }
    for raw in &input.bcc_addrs {
        builder = builder.bcc(parse_recipient(raw)?);
        recipient_count += 1;
    }
    if recipient_count == 0 {
        bail!("add at least one recipient");
    }

    if let Some(message_id) = reply_message_id.filter(|id| is_safe_message_id(id)) {
        builder = builder
            .in_reply_to(message_id.to_string())
            .references(message_id.to_string());
    }

    builder
        .header(ContentType::TEXT_PLAIN)
        .body(input.body_text.clone())
        .context("building outgoing message")
}

fn parse_recipient(raw: &str) -> Result<Mailbox> {
    let value = raw.trim();
    if value.is_empty() {
        bail!("recipient address cannot be empty");
    }
    value
        .parse()
        .with_context(|| format!("invalid recipient address: {value}"))
}

fn is_safe_message_id(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && !trimmed.contains(['\r', '\n'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> Account {
        Account {
            id: "account".into(),
            email: "me@example.com".into(),
            display_name: "Cosmic User".into(),
            kind: AccountKind::Imap,
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            smtp_host: "smtp.example.com".into(),
            smtp_port: 587,
            username: "me@example.com".into(),
        }
    }

    fn input() -> SendMessageInput {
        SendMessageInput {
            account_id: "account".into(),
            to_addrs: vec!["Recipient <to@example.com>".into()],
            cc_addrs: vec!["cc@example.com".into()],
            bcc_addrs: vec!["hidden@example.com".into()],
            subject: "Hello".into(),
            body_text: "Plain body".into(),
            reply_to_message_id: None,
        }
    }

    #[test]
    fn builds_plain_message_and_hides_bcc_header() {
        let message = build_message(&account(), &input(), None).expect("build message");
        let formatted = String::from_utf8(message.formatted()).expect("message is utf-8");

        assert!(formatted.contains("From:"));
        assert!(formatted.contains("Cosmic User"));
        assert!(formatted.contains("me@example.com"));
        assert!(formatted.contains("To:"));
        assert!(formatted.contains("Recipient"));
        assert!(formatted.contains("to@example.com"));
        assert!(formatted.contains("Cc: cc@example.com"));
        assert!(formatted.contains("Subject: Hello"));
        assert!(formatted.contains("Content-Type: text/plain"));
        assert!(formatted.ends_with("Plain body"));
        assert!(!formatted.contains("Bcc:"));
        assert_eq!(message.envelope().to().len(), 3);
    }

    #[test]
    fn adds_direct_parent_reply_headers() {
        let message =
            build_message(&account(), &input(), Some("<parent@example.com>")).expect("build reply");
        let formatted = String::from_utf8(message.formatted()).expect("message is utf-8");

        assert!(formatted.contains("In-Reply-To: <parent@example.com>"));
        assert!(formatted.contains("References: <parent@example.com>"));
    }

    #[test]
    fn rejects_missing_or_invalid_recipients() {
        let mut no_recipients = input();
        no_recipients.to_addrs.clear();
        no_recipients.cc_addrs.clear();
        no_recipients.bcc_addrs.clear();
        assert!(build_message(&account(), &no_recipients, None).is_err());

        let mut invalid = input();
        invalid.to_addrs = vec!["not an address".into()];
        assert!(build_message(&account(), &invalid, None).is_err());
    }

    #[test]
    fn omits_unsafe_reply_header() {
        let message = build_message(
            &account(),
            &input(),
            Some("<parent@example.com>\r\nBcc: attacker@example.com"),
        )
        .expect("build message");
        let formatted = String::from_utf8(message.formatted()).expect("message is utf-8");
        assert!(!formatted.contains("In-Reply-To:"));
        assert!(!formatted.contains("attacker@example.com"));
    }
}
