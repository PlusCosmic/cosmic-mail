//! The app's line icons: 24-unit outlines with round caps, drawn in their element's colour
//! (the same set the design mock-ups use).

use rune::Icon;

macro_rules! icon {
    ($id:ident, $name:literal, $body:literal) => {
        pub static $id: Icon = Icon {
            name: concat!("cosmic-", $name),
            view: 24.0,
            body: concat!(
                r#"<g fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</g>"
            ),
        };
    };
}

icon!(
    INBOX,
    "inbox",
    r#"<path d="M3 13h5l1.5 2.5h5L16 13h5"/><path d="M5.5 5h13L21 13v6H3v-6z"/>"#
);
icon!(
    PEN,
    "pen",
    r#"<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="M13.5 6.5l4 4"/>"#
);
icon!(PLUS, "plus", r#"<path d="M12 5v14M5 12h14"/>"#);
icon!(
    SLIDERS,
    "sliders",
    r#"<path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>"#
);
icon!(
    SEARCH,
    "search",
    r#"<circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4.2-4.2"/>"#
);
icon!(
    REPLY,
    "reply",
    r#"<path d="M9 7l-5 5 5 5"/><path d="M4 12h10a6 6 0 0 1 6 6"/>"#
);
icon!(
    REPLY_ALL,
    "reply-all",
    r#"<path d="M8 7l-5 5 5 5"/><path d="M13 7l-5 5 5 5"/><path d="M8 12h7a6 6 0 0 1 6 6"/>"#
);
icon!(
    FORWARD,
    "forward",
    r#"<path d="M15 7l5 5-5 5"/><path d="M20 12H10a6 6 0 0 0-6 6"/>"#
);
icon!(FLAG, "flag", r#"<path d="M5 21V4h12l-2.5 4.5L17 13H5"/>"#);
icon!(
    MOVE,
    "move",
    r#"<path d="M3 6h6l2 2h10v11H3z"/><path d="M11 13.5h6M14.5 11l2.5 2.5-2.5 2.5"/>"#
);
icon!(
    ARCHIVE,
    "archive",
    r#"<rect x="3" y="4" width="18" height="5" rx="1"/><path d="M5 9v11h14V9M10 13h4"/>"#
);
icon!(
    TRASH,
    "trash",
    r#"<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>"#
);
icon!(
    MAIL,
    "mail",
    r#"<rect x="3" y="5" width="18" height="14" rx="1.5"/><path d="M3.5 6l8.5 7 8.5-7"/>"#
);
icon!(
    MAIL_OPEN,
    "mail-open",
    r#"<path d="M3 10l9-6 9 6v10H3z"/><path d="M3.5 10.5l8.5 6 8.5-6"/>"#
);
icon!(
    CLIP,
    "clip",
    r#"<path d="M20 11l-8.5 8.5a5 5 0 0 1-7-7L13 4a3.5 3.5 0 0 1 5 5l-8.5 8.5a2 2 0 0 1-3-3L14 7"/>"#
);
icon!(
    PACKAGE,
    "package",
    r#"<path d="M3 7.5l9-4.5 9 4.5v9L12 21l-9-4.5z"/><path d="M3 7.5l9 4.5 9-4.5M12 12v9"/>"#
);
icon!(SEND, "send", r#"<path d="M4 12l16-8-6 16-3-7z"/>"#);
icon!(CLOSE, "close", r#"<path d="M6 6l12 12M18 6L6 18"/>"#);
icon!(CHEVRON, "chevron", r#"<path d="M9 6l6 6-6 6"/>"#);
icon!(FOLDER, "folder", r#"<path d="M3 6h6l2 2h10v11H3z"/>"#);
icon!(
    DRAFT,
    "draft",
    r#"<path d="M6 3h9l4 4v14H6z"/><path d="M14 3v5h5"/>"#
);
icon!(
    SPAM,
    "spam",
    r#"<path d="M12 3l8 3v6c0 4.5-3.4 8-8 9-4.6-1-8-4.5-8-9V6z"/><path d="M12 8v5M12 16v.5"/>"#
);
icon!(CHECK, "check", r#"<path d="M5 12.5l4.5 4.5L19 7.5"/>"#);
icon!(
    ALERT,
    "alert",
    r#"<circle cx="12" cy="12" r="9"/><path d="M12 7.5v5.5M12 16.5v.5"/>"#
);
icon!(
    LOCK,
    "lock",
    r#"<rect x="5" y="10" width="14" height="10" rx="1.5"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/>"#
);
icon!(
    EXTERNAL,
    "external",
    r#"<path d="M14 4h6v6M20 4l-9 9"/><path d="M18 14v6H4V6h6"/>"#
);
icon!(
    IMAGE,
    "image",
    r#"<rect x="3" y="4" width="18" height="16" rx="1.5"/><circle cx="9" cy="10" r="1.6"/><path d="M21 16l-5-5-8 9"/>"#
);
icon!(
    SYNC,
    "sync",
    r#"<path d="M20 11a8 8 0 0 0-14.3-4.9L4 8"/><path d="M4 13a8 8 0 0 0 14.3 4.9L20 16"/><path d="M4 4v4h4M20 20v-4h-4"/>"#
);

/// A filled star, for a flagged message.
pub static STAR: Icon = Icon {
    name: "cosmic-star",
    view: 24.0,
    body: r#"<path d="M12 3l2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z" fill="currentColor" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/>"#,
};

/// The icon for a folder role (`inbox`, `sent`, ...).
pub fn for_role(role: &str) -> &'static Icon {
    match role {
        "inbox" => &INBOX,
        "sent" => &SEND,
        "drafts" => &DRAFT,
        "archive" => &ARCHIVE,
        "trash" => &TRASH,
        "spam" => &SPAM,
        _ => &FOLDER,
    }
}
