//! A built-in demo mailbox for headless renders and UI tests.
//!
//! `--snap out.png [--scene NAME]` renders these states without a backend, a
//! profile or the network. Nothing here is reachable from a normal launch.

use crate::accounts::{AccountKind, AccountPublic};
use crate::sync::SyncState;
use crate::wire::{AttachmentInfo, Folder, MessageBody, MessageSummary, Shipment};

use super::compose::ComposeSeed;
use super::theme::Appearance;
use super::{html, AddAccountForm, ColorScheme, ComposeForm, Inbox, Modal, SettingsState, Ui};

/// A named demo state: a change to [`state`].
pub type Scene = (&'static str, fn(&mut Ui));

/// Named states for `--scene`.
pub const SCENES: &[Scene] = &[
    ("default", |_| {}),
    ("welcome", welcome),
    ("folder", folder_view),
    ("unread", |s| s.filter = super::Filter::Unread),
    ("search", search),
    ("compose", compose),
    ("reply", reply),
    ("palette", palette),
    ("move", move_picker),
    ("add-account", add_account),
    ("settings", settings),
    ("toasts", toasts),
    ("light", |s| s.appearance = Appearance::Light),
    ("oxblood", |s| s.scheme = ColorScheme::Oxblood),
    ("ink", |s| s.scheme = ColorScheme::Ink),
    ("walnut", |s| s.scheme = ColorScheme::Walnut),
    ("graphite", |s| s.scheme = ColorScheme::Graphite),
    ("graphite-light", |s| {
        s.scheme = ColorScheme::Graphite;
        s.appearance = Appearance::Light;
    }),
];

fn account(id: &str, email: &str, name: &str, kind: AccountKind) -> AccountPublic {
    AccountPublic {
        id: id.into(),
        email: email.into(),
        display_name: name.into(),
        kind,
    }
}

fn folder(id: i64, account: &str, name: &str, role: &str, unread: i64, total: i64) -> Folder {
    Folder {
        id,
        account_id: account.into(),
        name: name.into(),
        role: role.into(),
        unread_count: unread,
        total_count: total,
    }
}

/// Dates relative to now, so list rows read naturally.
fn ago(minutes: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339()
}

#[allow(clippy::too_many_arguments)]
fn message(
    id: i64,
    account: &str,
    folder_id: i64,
    from: (&str, &str),
    subject: &str,
    snippet: &str,
    minutes: i64,
    seen: bool,
    flagged: bool,
) -> MessageSummary {
    MessageSummary {
        id,
        account_id: account.into(),
        folder_id,
        uid: id as u32,
        subject: subject.into(),
        from_name: from.0.into(),
        from_addr: from.1.into(),
        date: ago(minutes),
        snippet: snippet.into(),
        seen,
        flagged,
        has_attachments: false,
    }
}

const DEMO_HTML: &str = r#"
<p>Hi Harry,</p>
<p>Your order <b>#114-5521</b> has shipped and is on its way. You can
<a href="https://www.ups.com/track?tracknum=1Z999AA10123456784">track the parcel here</a>
or reply to this email if anything looks wrong.</p>
<ul><li>Mechanical keyboard (tactile)</li><li>Spare keycaps</li></ul>
<blockquote>Estimated delivery: Thursday, before 7pm.</blockquote>
<p>Thanks for shopping with us,<br><i>The Parts Shop team</i></p>
"#;

/// The demo mailbox: two accounts, a unified inbox, one open message.
pub fn state() -> Ui {
    let mut s = Ui::new(None, Inbox::default());
    // Fixed, so renders don't depend on the machine's light or dark setting.
    s.appearance = Appearance::Dark;
    s.loading_accounts = false;
    s.accounts = vec![
        account("a1", "harry@pluscosmic.dev", "Harry", AccountKind::Imap),
        account(
            "a2",
            "harry.leach@gmail.com",
            "Harry (Gmail)",
            AccountKind::Gmail,
        ),
    ];
    s.folders.insert(
        "a1".into(),
        vec![
            folder(1, "a1", "INBOX", "inbox", 3, 42),
            folder(2, "a1", "Sent", "sent", 0, 120),
            folder(3, "a1", "Drafts", "drafts", 0, 2),
            folder(4, "a1", "Archive", "archive", 0, 812),
            folder(5, "a1", "Trash", "trash", 0, 9),
            folder(6, "a1", "Receipts", "", 1, 33),
        ],
    );
    s.folders.insert(
        "a2".into(),
        vec![
            folder(11, "a2", "INBOX", "inbox", 2, 1204),
            folder(12, "a2", "[Gmail]/Sent Mail", "sent", 0, 340),
            folder(13, "a2", "[Gmail]/All Mail", "archive", 0, 5022),
            folder(14, "a2", "[Gmail]/Spam", "spam", 0, 17),
            folder(15, "a2", "[Gmail]/Trash", "trash", 0, 4),
        ],
    );
    s.sync_states.insert("a1".into(), SyncState::Idle);
    s.sync_states.insert("a2".into(), SyncState::Syncing);
    s.messages = vec![
        message(
            101,
            "a1",
            1,
            ("The Parts Shop", "orders@partsshop.example"),
            "Your order has shipped",
            "Your order #114-5521 has shipped and is on its way. Track the parcel…",
            4,
            false,
            false,
        ),
        message(
            102,
            "a2",
            11,
            ("Mira Okafor", "mira@studio.example"),
            "Re: Thursday's review",
            "Sounds good — I moved the review to 3pm so we can include the new mocks.",
            38,
            false,
            true,
        ),
        message(
            103,
            "a1",
            1,
            ("GitHub", "noreply@github.com"),
            "[cosmic-mail] Rune port is ready for review",
            "A new pull request was opened: port the frontend to Rune.",
            95,
            false,
            false,
        ),
        message(
            104,
            "a2",
            11,
            ("Jonas Weber", "jonas@weber.example"),
            "Photos from the trip",
            "Finally sorted through them, here's the shared album link.",
            60 * 5,
            true,
            false,
        ),
        message(
            105,
            "a1",
            1,
            ("Hetzner Online", "billing@hetzner.example"),
            "Invoice 2026-1102",
            "Your invoice for November is available in the console.",
            60 * 26,
            true,
            false,
        ),
        message(
            106,
            "a2",
            11,
            ("Ana Lima", "ana@lima.example"),
            "Lunch next week?",
            "I'm around Tuesday or Wednesday if either works for you.",
            60 * 50,
            true,
            true,
        ),
        message(
            107,
            "a1",
            1,
            ("omarchy", "news@omarchy.example"),
            "Release notes: themes and state dir",
            "Themes now live under ~/.local/state/omarchy/current.",
            60 * 24 * 9,
            true,
            false,
        ),
        message(
            108,
            "a2",
            11,
            ("Calendar", "calendar-notification@google.example"),
            "Updated invitation: Planning",
            "This event has been updated. Thursday 10:00 – 11:00.",
            60 * 24 * 40,
            true,
            false,
        ),
    ];
    s.offset = 0;
    s.has_more = true;
    s.selected_message = Some(101);
    let body = MessageBody {
        id: 101,
        html: Some(DEMO_HTML.into()),
        text: Some("Your order has shipped.".into()),
        to_addrs: vec!["harry@pluscosmic.dev".into()],
        cc_addrs: Vec::new(),
        attachments: vec![AttachmentInfo {
            id: 1,
            filename: "invoice-114-5521.pdf".into(),
            mime_type: "application/pdf".into(),
            size_bytes: 48_213,
            is_inline: false,
        }],
    };
    s.body_lines = body.html.as_deref().map(html::render).unwrap_or_default();
    s.body = Some(body);
    s.shipments = vec![Shipment {
        id: 1,
        carrier: "ups".into(),
        tracking_number: Some("1Z999AA10123456784".into()),
        tracking_url: Some("https://www.ups.com/track?tracknum=1Z999AA10123456784".into()),
        order_id: Some("114-5521".into()),
        detected_at: ago(4),
    }];
    s
}

fn welcome(s: &mut Ui) {
    s.accounts.clear();
    s.folders.clear();
    s.messages.clear();
    s.selected_message = None;
    s.body = None;
}

fn folder_view(s: &mut Ui) {
    s.unified = false;
    s.selected_account = Some("a1".into());
    s.selected_folder = Some(1);
    s.messages.retain(|m| m.account_id == "a1");
}

fn search(s: &mut Ui) {
    s.search_active = true;
    s.search_query = "invoice".into();
    s.messages
        .retain(|m| m.subject.contains("Invoice") || m.snippet.contains("invoice"));
    s.has_more = false;
    s.selected_message = None;
    s.body = None;
}

fn compose(s: &mut Ui) {
    s.modal = Some(Modal::Compose(Box::new(ComposeForm::new(
        ComposeSeed::new_message("a1".into()),
    ))));
}

fn reply(s: &mut Ui) {
    s.open_reply(false);
}

fn palette(s: &mut Ui) {
    s.open_palette();
}

fn move_picker(s: &mut Ui) {
    s.open_move_picker();
}

fn add_account(s: &mut Ui) {
    let mut form = AddAccountForm::new();
    form.email.replace_text("someone@fastmail.example");
    form.email.focused = false;
    form.email_was_focused = false;
    form.imap_host.replace_text("imap.fastmail.example");
    form.smtp_host.replace_text("smtp.fastmail.example");
    form.autofilled = Some(form.host_fields());
    form.discovered_for = Some("someone@fastmail.example".into());
    form.discovered = Some(crate::autoconfig::DiscoveredConfig {
        kind: crate::autoconfig::DiscoveredKind::Imap,
        imap_host: "imap.fastmail.example".into(),
        imap_port: 993,
        smtp_host: "smtp.fastmail.example".into(),
        smtp_port: 587,
        username: String::new(),
        source: crate::autoconfig::DiscoverySource::Ispdb,
        confident: true,
    });
    s.modal = Some(Modal::AddAccount(Box::new(form)));
}

fn settings(s: &mut Ui) {
    s.reauth_required.insert("a2".into());
    s.modal = Some(Modal::Settings(SettingsState::default()));
}

fn toasts(s: &mut Ui) {
    s.toast("Message sent", false);
    s.toast(
        "Sync error: connection refused (imap.example.com:993)",
        true,
    );
}
