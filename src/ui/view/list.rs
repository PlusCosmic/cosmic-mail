//! The message list: filter chips, rows, and "load more".

use rune::{col, el, memo, row, Color, Cursor, El, ScrollSpec};

use super::chrome::account_tag;
use super::{btn, font, glyph, icons, label, Kind, RADIUS, SMALL};
use crate::settings::AccountMarker;
use crate::sync::SyncState;
use crate::ui::format::{format_count, relative_date};
use crate::ui::theme::Palette;
use crate::ui::{Filter, Ui};
use crate::wire::MessageSummary;

/// A filter chip (`ui-chip`): the accent tint when on, with its count.
fn chip(
    key: &'static str,
    caption: &'static str,
    count: usize,
    on: bool,
    filter: Filter,
    p: &Palette,
) -> El {
    row()
        .key(key)
        .role("button")
        .label(caption)
        .checked(on)
        .px(10.0)
        .h(26.0)
        .gap(6.0)
        .center()
        .radius(RADIUS)
        .border(1.0, if on { p.accent_dim } else { p.border })
        .fill(if on {
            p.accent_soft
        } else {
            p.accent_soft.alpha(0.0)
        })
        .hover(p.fg.alpha(0.05))
        .focusable(true)
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut Ui| s.filter = filter)
        .child(label(
            caption,
            font(12.0, 600.0),
            if on { p.accent } else { p.muted },
        ))
        .child(label(count.to_string(), font(12.0, 400.0), p.muted))
}

/// The list pane (its width is set by the caller).
pub fn list_pane(s: &Ui, p: &Palette) -> El {
    let visible = s.visible_messages();
    let loaded = s
        .messages
        .iter()
        .filter(|m| !s.pending_removals.contains_key(&m.id));
    let (all, unread, flagged) = loaded.fold((0, 0, 0), |(a, u, f), m| {
        (a + 1, u + usize::from(!m.seen), f + usize::from(m.flagged))
    });
    let head = row()
        .key("list-head")
        .w_full()
        .px(16.0)
        .py(10.0)
        .gap(8.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .child(chip(
            "filter-all",
            "All",
            all,
            s.filter == Filter::All,
            Filter::All,
            p,
        ))
        .child(chip(
            "filter-unread",
            "Unread",
            unread,
            s.filter == Filter::Unread,
            Filter::Unread,
            p,
        ))
        .child(chip(
            "filter-flagged",
            "Flagged",
            flagged,
            s.filter == Filter::Flagged,
            Filter::Flagged,
            p,
        ))
        .child(el().grow(1.0))
        .child(
            label(
                format!(
                    "{}{} shown",
                    visible.len(),
                    if s.can_load_more() { "+" } else { "" }
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
            .child(label(msg.to_string(), font(13.0, 400.0), p.muted))
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
                    .map(|a| (account_tag(s, &a.id), s.account_slot(&a.id)))
            } else {
                None
            };
            let palette = *p;
            let msg = (*m).clone();
            let when = relative_date(&m.date);
            let marker = s.settings.account_marker;
            rows = rows.child(memo(
                format!("msg-{}", m.id),
                (&msg, selected, &account, &palette, &when, marker),
                || message_row(&msg, selected, account.clone(), marker, &when, &palette),
            ));
        }
        if s.can_load_more() || s.loading_older {
            let busy = s.loading_messages || s.loading_older;
            let caption = if s.loading_older {
                "Loading older messages…"
            } else if s.loading_messages {
                "Loading…"
            } else if s.has_more {
                "Load more"
            } else {
                "Load older messages"
            };
            let mut footer = col().w_full().py(14.0).gap(6.0).items_center().child(btn(
                "load-more",
                caption,
                None,
                None,
                Kind::Secondary,
                !busy,
                p,
                |s, _| s.load_more(),
            ));
            let remaining = s.remaining_server_count();
            if !s.has_more && remaining > 0 {
                footer = footer.child(
                    label(
                        format!("{} older on server", format_count(remaining)),
                        SMALL,
                        p.muted,
                    )
                    .key("load-more-hint"),
                );
            }
            rows = rows.child(footer);
        }
    }

    col()
        .key("list")
        .min_w(0.0)
        .child(head)
        .child(super::scroll_bar(
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
            p,
        ))
}

/// One message row: unread dot; sender, account label and date; subject; snippet. In the
/// unified inbox (`account` is set) the row shows its account per `marker`.
fn message_row(
    m: &MessageSummary,
    selected: bool,
    account: Option<(String, usize)>,
    marker: AccountMarker,
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
    let dot = el()
        .size(7.0, 7.0)
        .radius(3.5)
        .fill(if unread {
            p.accent
        } else {
            p.accent.alpha(0.0)
        })
        .mt(6.0);

    let mut top = row().w_full().gap(8.0).center_y().child(
        label(from, font(13.5, if unread { 650.0 } else { 500.0 }), p.fg)
            .truncate()
            .shrink()
            .min_w(0.0),
    );
    if let (Some((name, slot)), AccountMarker::Label) = (&account, marker) {
        let (name, slot) = (name.clone(), *slot);
        top = top.child(
            row()
                .px(6.0)
                .h(18.0)
                .gap(5.0)
                .center_y()
                .radius(RADIUS)
                .border(1.0, p.border)
                .child(el().size(6.0, 6.0).radius(3.0).fill(p.slot(slot)))
                .child(label(name, font(10.5, 600.0), p.muted)),
        );
    }
    top = top.child(el().grow(1.0));
    if m.has_attachments {
        top = top.child(glyph(&icons::CLIP, 13.0, p.muted).label("Has attachments"));
    }
    if m.flagged {
        top = top.child(glyph(&icons::STAR, 13.0, p.warn).label("Flagged"));
    }
    top = top.child(label(
        when.to_string(),
        font(11.5, if unread { 600.0 } else { 400.0 }),
        if selected || unread { p.fg } else { p.muted },
    ));

    let subject = if m.subject.is_empty() {
        "(no subject)".to_string()
    } else {
        m.subject.clone()
    };
    let text_col = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(3.0)
        .child(top)
        .child(
            label(
                subject,
                font(13.5, if unread { 650.0 } else { 400.0 }),
                p.fg,
            )
            .truncate()
            .w_full(),
        )
        .child(
            label(m.snippet.clone(), font(12.5, 400.0), p.muted)
                .truncate()
                .w_full(),
        );

    let mut r = row()
        .key(format!("msg-{id}"))
        .role("option")
        .label(format!("{} — {}", m.from_name, m.subject))
        .checked(selected)
        .w_full()
        .pl(12.0)
        .pr(18.0)
        .py(11.0)
        .gap(9.0)
        .fill(if selected { p.panel } else { p.bg.alpha(0.0) })
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .hover(p.fg.alpha(0.035))
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut Ui| s.select_message(id))
        .child(dot)
        .child(text_col);
    let bar = account
        .as_ref()
        .and_then(|(name, slot)| account_bar(marker, selected, p.slot(*slot), name));
    let left_bar = matches!(marker, AccountMarker::LeftLine | AccountMarker::LeftInset);
    if let Some(bar) = bar {
        r = r.child(bar);
    }
    if selected && !(left_bar && account.is_some()) {
        // The selection's accent edge, inside the row.
        r = r.child(
            el().pin(Some(0.0), Some(0.0), None, Some(0.0))
                .w(3.0)
                .fill(p.accent),
        );
    }
    r
}

/// The account's colour down one edge of a row: full height, or a short rounded bar inset
/// from the row's ends. A left bar takes the selection edge's place, so it thickens when
/// the row is selected. `None` for the label marker.
fn account_bar(marker: AccountMarker, selected: bool, color: Color, name: &str) -> Option<El> {
    let (left, inset) = match marker {
        AccountMarker::Label => return None,
        AccountMarker::LeftLine => (true, false),
        AccountMarker::LeftInset => (true, true),
        AccountMarker::RightLine => (false, false),
        AccountMarker::RightInset => (false, true),
    };
    let w = if left && selected { 5.0 } else { 3.0 };
    let (end, gap) = if inset { (12.0, 4.0) } else { (0.0, 0.0) };
    let (l, r) = if left {
        (Some(gap), None)
    } else {
        (None, Some(gap))
    };
    let mut bar = el()
        .pin(l, Some(end), r, Some(end))
        .w(w)
        .fill(color)
        .help(name.to_string());
    if inset {
        bar = bar.radius(w / 2.0);
    }
    Some(bar)
}
