//! The message list: filter chips, rows, and "load more".

use rune::{col, el, memo, row, Cursor, El, ScrollSpec};

use super::{font, label, SMALL};
use crate::sync::SyncState;
use crate::ui::format::{avatar_slot, initials, relative_date, short_email};
use crate::ui::theme::Palette;
use crate::ui::{Filter, Ui};
use crate::wire::MessageSummary;

fn chip(key: &'static str, caption: &'static str, on: bool, filter: Filter, p: &Palette) -> El {
    row()
        .key(key)
        .role("button")
        .label(caption)
        .checked(on)
        .px(9.0)
        .h(20.0)
        .center()
        .radius(10.0)
        .border(1.0, if on { p.accent_dim } else { p.border })
        .fill(if on {
            p.accent_soft
        } else {
            p.accent_soft.alpha(0.0)
        })
        .hover(p.fg.alpha(0.06))
        .focusable(true)
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut Ui| s.filter = filter)
        .child(label(caption, SMALL, if on { p.fg } else { p.muted }))
}

/// The list pane (its width is set by the caller).
pub fn list_pane(s: &Ui, p: &Palette) -> El {
    let visible = s.visible_messages();
    let head = row()
        .key("list-head")
        .w_full()
        .px(14.0)
        .py(8.0)
        .gap(8.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .child(
            row()
                .gap(4.0)
                .child(chip(
                    "filter-all",
                    "All",
                    s.filter == Filter::All,
                    Filter::All,
                    p,
                ))
                .child(chip(
                    "filter-unread",
                    "Unread",
                    s.filter == Filter::Unread,
                    Filter::Unread,
                    p,
                ))
                .child(chip(
                    "filter-flagged",
                    "Flagged",
                    s.filter == Filter::Flagged,
                    Filter::Flagged,
                    p,
                )),
        )
        .child(el().grow(1.0))
        .child(
            label(
                format!(
                    "{}{} messages",
                    visible.len(),
                    if s.has_more { "+" } else { "" }
                ),
                SMALL,
                p.muted,
            )
            .key("list-count"),
        );

    let syncing = if s.unified {
        s.accounts
            .iter()
            .any(|a| s.sync_state(&a.id) == SyncState::Syncing)
    } else {
        s.selected_account
            .as_ref()
            .is_some_and(|id| s.sync_state(id) == SyncState::Syncing)
    };
    let empty = |msg: &str| {
        row()
            .key("list-empty")
            .w_full()
            .h(120.0)
            .center()
            .child(label(msg.to_string(), font(12.0, 400.0), p.muted))
    };

    let mut rows = col().key("rows").role("listbox").label("Messages").w_full();
    if !s.unified && s.selected_folder.is_none() {
        rows = rows.child(empty("Select a folder"));
    } else if visible.is_empty() && (s.loading_messages || syncing) {
        rows = rows.child(empty("Syncing messages…"));
    } else if visible.is_empty() {
        rows = rows.child(empty(if s.messages.is_empty() {
            "No messages"
        } else {
            "No messages match the filter"
        }));
    } else {
        for m in &visible {
            let selected = s.selected_message == Some(m.id);
            let account = if s.unified {
                s.accounts
                    .iter()
                    .find(|a| a.id == m.account_id)
                    .map(|a| (short_email(&a.email).to_string(), avatar_slot(&a.email)))
            } else {
                None
            };
            let palette = *p;
            let msg = (*m).clone();
            let when = relative_date(&m.date);
            rows = rows.child(memo(
                format!("msg-{}", m.id),
                (&msg, selected, &account, &palette, &when),
                || message_row(&msg, selected, account.clone(), &when, &palette),
            ));
        }
        if s.has_more {
            let caption = if s.loading_messages {
                "Loading…"
            } else {
                "Load more"
            };
            rows = rows.child(
                row()
                    .key("load-more")
                    .role("button")
                    .label(caption)
                    .w_full()
                    .h(34.0)
                    .center()
                    .hover(p.fg.alpha(0.06))
                    .cursor(Cursor::Pointer)
                    .focusable(true)
                    .on_press(|s: &mut Ui| s.load_more())
                    .child(label(caption, font(12.0, 400.0), p.muted)),
            );
        }
    }

    col().key("list").min_w(0.0).child(head).child(
        col()
            .key("list-scroll")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .scroll_margin(8.0)
            .child(rows),
    )
}

/// One message row: unread dot, avatar, sender + date, subject, snippet, meta.
fn message_row(
    m: &MessageSummary,
    selected: bool,
    account: Option<(String, usize)>,
    when: &str,
    p: &Palette,
) -> El {
    let id = m.id;
    let unread = !m.seen;
    let from = if !m.from_name.is_empty() {
        m.from_name.clone()
    } else if !m.from_addr.is_empty() {
        m.from_addr.clone()
    } else {
        "(unknown)".to_string()
    };
    let seed = if m.from_addr.is_empty() {
        &m.from_name
    } else {
        &m.from_addr
    };
    let dot = el()
        .size(7.0, 7.0)
        .radius(3.5)
        .fill(if unread {
            p.accent
        } else {
            p.accent.alpha(0.0)
        })
        .mt(5.0);
    let avatar = row()
        .size(24.0, 24.0)
        .center()
        .radius(4.0)
        .fill(p.slot(avatar_slot(seed)))
        .child(label(
            initials(&m.from_name, &m.from_addr),
            font(11.0, 700.0),
            p.bg,
        ));

    let top = row()
        .w_full()
        .gap(10.0)
        .center_y()
        .child(
            label(from, font(13.0, if unread { 700.0 } else { 500.0 }), p.fg)
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(label(
            when.to_string(),
            SMALL,
            if selected { p.fg } else { p.muted },
        ));
    let subject = if m.subject.is_empty() {
        "(no subject)".to_string()
    } else {
        m.subject.clone()
    };
    let mut text_col = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(3.0)
        .child(top)
        .child(
            label(
                subject,
                font(12.5, if unread { 600.0 } else { 400.0 }),
                p.fg,
            )
            .truncate()
            .w_full(),
        )
        .child(
            label(
                m.snippet.clone(),
                font(11.5, 400.0),
                if selected { p.muted } else { p.faint },
            )
            .truncate()
            .w_full(),
        );
    if account.is_some() || m.flagged {
        let mut meta = row().gap(7.0).center_y();
        if let Some((name, slot)) = account {
            meta = meta.child(
                row()
                    .gap(4.0)
                    .center_y()
                    .child(el().size(6.0, 6.0).radius(3.0).fill(p.slot(slot)))
                    .child(label(name, font(10.0, 400.0), p.muted)),
            );
        }
        if m.flagged {
            meta = meta.child(label("★", font(10.0, 400.0), p.c[3]).label("Flagged"));
        }
        text_col = text_col.child(meta);
    }

    row()
        .key(format!("msg-{id}"))
        .role("option")
        .label(format!("{} — {}", m.from_name, m.subject))
        .checked(selected)
        .w_full()
        .pl(12.0)
        .pr(26.0)
        .py(9.0)
        .gap(10.0)
        .fill(if selected {
            p.sel_bg.alpha(0.55)
        } else {
            p.bg.alpha(0.0)
        })
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border_soft)
        .hover(p.fg.alpha(0.05))
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut Ui| s.select_message(id))
        .child(dot)
        .child(avatar)
        .child(text_col)
}
