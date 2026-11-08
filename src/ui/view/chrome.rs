//! The frame around the panes: account rail, header, folder column, footer,
//! toasts, and the no-accounts welcome screen.

use rune::{col, el, row, text, text_input, Cursor, El, Key, KeyEvent, Tx};

use super::{button, font, kbd, label, BODY, RADIUS, SMALL};
use crate::accounts::AccountKind;
use crate::sync::SyncState;
use crate::ui::format::{
    avatar_slot, cap_badge, display_folder_name, initials, role_glyph, short_email,
};
use crate::ui::theme::Palette;
use crate::ui::Ui;

/// Shown when no account is configured.
pub fn welcome(p: &Palette) -> El {
    col()
        .key("welcome")
        .w_full()
        .grow(1.0)
        .center()
        .gap(10.0)
        .child(label("Cosmic Mail", font(22.0, 700.0), p.fg))
        .child(label(
            "No accounts yet. Add one to get started.",
            BODY,
            p.muted,
        ))
        .child(
            button("welcome-add", "Add account", p, true, true, |s, _| {
                s.open_add_account()
            })
            .mt(6.0),
        )
}

/// One square rail button: an avatar tile with an optional unread badge.
fn rail_button(
    key: String,
    tip: String,
    tile: El,
    active: bool,
    badge: i64,
    p: &Palette,
    on: impl Fn(&mut Ui, &mut Tx) + 'static,
) -> El {
    let marker = el()
        .w(3.0)
        .h(18.0)
        .radius(2.0)
        .fill(if active {
            p.accent
        } else {
            p.accent.alpha(0.0)
        })
        .absolute(-9.0, 8.0);
    let mut b = row()
        .key(key)
        .role("button")
        .label(tip.clone())
        .help(tip)
        .checked(active)
        .size(34.0, 34.0)
        .center()
        .radius(6.0)
        .fill(if active {
            p.panel_2
        } else {
            p.panel_2.alpha(0.0)
        })
        .hover(p.fg.alpha(0.07))
        .focusable(true)
        .cursor(Cursor::Pointer)
        .on_press_tx(on)
        .child(marker)
        .child(tile);
    let count = cap_badge(badge);
    if !count.is_empty() {
        b = b.child(
            row()
                .absolute(20.0, -3.0)
                .min_w(15.0)
                .h(15.0)
                .px(3.0)
                .center()
                .radius(8.0)
                .fill(p.accent)
                .label(format!("{badge} unread"))
                .child(label(count, font(9.0, 700.0), p.bg)),
        );
    }
    b
}

fn tile(content: El, fill: rune::Color) -> El {
    row()
        .size(26.0, 26.0)
        .center()
        .radius(5.0)
        .fill(fill)
        .child(content)
}

/// The account switcher down the left edge.
pub fn rail(s: &Ui, p: &Palette) -> El {
    let mut rail = col()
        .key("rail")
        .role("navigation")
        .label("Account switcher")
        .w(52.0)
        .min_w(52.0)
        .h_full()
        .py(10.0)
        .gap(6.0)
        .items_center()
        .fill(p.rail)
        .border_sides([0.0, 1.0, 0.0, 0.0], p.border)
        .child(
            row()
                .size(30.0, 30.0)
                .center()
                .radius(6.0)
                .fill(p.accent)
                .mb(6.0)
                .child(label("C", font(15.0, 700.0), p.bg)),
        );

    rail = rail.child(rail_button(
        "rail-unified".into(),
        "Unified inbox".into(),
        tile(label("⇊", font(15.0, 700.0), p.bg), p.c[5]),
        s.unified,
        s.unified_unread(),
        p,
        |s, _| s.select_unified(),
    ));
    for acc in &s.accounts {
        let id = acc.id.clone();
        let kind = match acc.kind {
            AccountKind::Gmail => "gmail",
            AccountKind::Imap => "imap",
        };
        rail = rail.child(rail_button(
            format!("rail-account-{}", acc.id),
            format!("{} — {kind}", acc.email),
            tile(
                label(
                    initials(&acc.display_name, &acc.email),
                    font(12.0, 700.0),
                    p.bg,
                ),
                p.slot(avatar_slot(&acc.email)),
            ),
            !s.unified && s.selected_account.as_deref() == Some(acc.id.as_str()),
            s.account_unread(&acc.id),
            p,
            move |s, _| s.select_account(&id),
        ));
    }

    let action_tile = |glyph: &str| {
        row()
            .size(26.0, 26.0)
            .center()
            .radius(5.0)
            .border(1.0, p.border)
            .child(label(glyph, font(14.0, 400.0), p.muted))
    };
    rail.child(el().grow(1.0))
        .child(rail_button(
            "rail-compose".into(),
            "Compose (c)".into(),
            action_tile("✎"),
            false,
            0,
            p,
            |s, _| s.open_compose(),
        ))
        .child(rail_button(
            "rail-add".into(),
            "Add account".into(),
            action_tile("+"),
            false,
            0,
            p,
            |s, _| s.open_add_account(),
        ))
        .child(rail_button(
            "rail-settings".into(),
            "Settings".into(),
            action_tile("⚙"),
            false,
            0,
            p,
            |s, _| s.open_settings(),
        ))
}

/// Aggregate sync status: error if any account errored, else syncing if any is.
pub fn overall_sync(s: &Ui) -> SyncState {
    let states: Vec<SyncState> = s.accounts.iter().map(|a| s.sync_state(&a.id)).collect();
    if states.contains(&SyncState::Error) {
        SyncState::Error
    } else if states.contains(&SyncState::Syncing) {
        SyncState::Syncing
    } else {
        SyncState::Idle
    }
}

/// The colour of a sync dot.
pub fn sync_color(state: SyncState, p: &Palette) -> rune::Color {
    match state {
        SyncState::Idle => p.ok,
        SyncState::Syncing => p.accent,
        SyncState::Error => p.error,
    }
}

/// Title, search field, and sync status.
pub fn header(s: &Ui, p: &Palette) -> El {
    let title = if s.unified {
        "Unified Inbox".to_string()
    } else if let Some(f) = s.selected_folder() {
        display_folder_name(&f.name).to_string()
    } else if let Some(acc) = s
        .selected_account
        .as_ref()
        .and_then(|id| s.accounts.iter().find(|a| &a.id == id))
    {
        acc.email.clone()
    } else {
        "No folder selected".to_string()
    };
    let n = s.accounts.len();
    let subtitle = format!(
        "{n} account{} · {} unread",
        if n == 1 { "" } else { "s" },
        s.unified_unread()
    );

    let mut head = row()
        .key("header")
        .w_full()
        .px(16.0)
        .py(9.0)
        .gap(14.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .child(
            label(title, font(13.0, 700.0), p.fg)
                .key("header-title")
                .truncate()
                .shrink(),
        )
        .child(label(subtitle, SMALL, p.muted).key("header-subtitle"))
        .child(el().grow(1.0));

    if s.search_active {
        let count = format!(
            "results for “{}” — {}{}",
            s.search_query,
            s.visible_messages().len(),
            if s.has_more { "+" } else { "" }
        );
        head = head.child(
            row()
                .key("search-status")
                .gap(8.0)
                .center_y()
                .child(label(count, SMALL, p.muted).truncate().shrink())
                .child(
                    row()
                        .key("clear-search")
                        .role("button")
                        .label("Clear search")
                        .px(7.0)
                        .h(18.0)
                        .center()
                        .radius(3.0)
                        .border(1.0, p.border)
                        .hover(p.fg.alpha(0.07))
                        .cursor(Cursor::Pointer)
                        .focusable(true)
                        .on_press(|s: &mut Ui| s.clear_search())
                        .child(label("clear", font(10.0, 400.0), p.fg)),
                ),
        );
    }

    let placeholder = if s.search_active {
        "Search all mail…"
    } else {
        "Filter loaded messages…"
    };
    let input = text_input(&s.query, font(12.0, 400.0))
        .key("search-input")
        .label("Search messages")
        .color(p.fg)
        .placeholder(placeholder, p.muted_2)
        .single_line()
        .grow(1.0)
        .min_w(0.0)
        .on_edit(|s: &mut Ui| &mut s.query)
        .on_submit(|s: &mut Ui, tx: &mut Tx| {
            // Search the whole cache, then hand keys back to j/k.
            s.run_search();
            tx.blur();
        })
        .on_input_key(|s: &mut Ui, ev: &KeyEvent, tx: &mut Tx| {
            if ev.key != Key::Escape {
                return false;
            }
            // Escape leaves an active search; either way it clears and blurs.
            s.clear_search();
            s.query.replace_text("");
            tx.blur();
            true
        });
    let search = row()
        .key("search")
        .w(320.0)
        .min_w(200.0)
        .shrink()
        .h(30.0)
        .px(11.0)
        .gap(8.0)
        .center_y()
        .radius(RADIUS)
        .fill(p.panel)
        .border(1.0, if s.query.focused { p.accent } else { p.border })
        .child(label("⌕", font(12.0, 400.0), p.muted))
        .child(input)
        .child(kbd(if s.search_active { "⏎" } else { "/" }, p));
    head = head.child(search);

    let state = overall_sync(s);
    let status = match state {
        SyncState::Syncing => "syncing",
        SyncState::Error => "sync error",
        SyncState::Idle => "synced",
    };
    head.child(
        row()
            .key("sync-status")
            .gap(6.0)
            .center_y()
            .child(el().size(7.0, 7.0).radius(3.5).fill(sync_color(state, p)))
            .child(label(status, SMALL, p.muted).key("sync-label")),
    )
}

/// The selected account's folders (per-account mode only).
pub fn folder_column(s: &Ui, p: &Palette) -> El {
    let (false, Some(account_id)) = (s.unified, s.selected_account.as_ref()) else {
        return el().key("no-folders").w(0.0);
    };
    let folders = s.folders.get(account_id).map(Vec::as_slice).unwrap_or(&[]);
    let mut col = col()
        .key("folders")
        .role("list")
        .label("Folders")
        .w(200.0)
        .min_w(200.0)
        .h_full()
        .min_h(0.0)
        .fill(p.panel)
        .border_sides([0.0, 1.0, 0.0, 0.0], p.border);
    if let Some(acc) = s.accounts.iter().find(|a| &a.id == account_id) {
        let name = if acc.display_name.is_empty() {
            acc.email.clone()
        } else {
            acc.display_name.clone()
        };
        col = col.child(
            row()
                .key("folders-head")
                .px(14.0)
                .py(10.0)
                .help(acc.email.clone())
                .child(label(name, font(11.0, 700.0), p.muted).truncate()),
        );
    }
    let mut list = col_list();
    for folder in folders {
        let id = folder.id;
        let active = s.selected_folder == Some(id);
        let mut b = row()
            .key(format!("folder-{id}"))
            .role("button")
            .label(display_folder_name(&folder.name).to_string())
            .checked(active)
            .w_full()
            .h(28.0)
            .px(14.0)
            .gap(8.0)
            .center_y()
            .fill(if active {
                p.panel_2
            } else {
                p.panel_2.alpha(0.0)
            })
            .hover(p.fg.alpha(0.06))
            .focusable(true)
            .cursor(Cursor::Pointer)
            .on_press(move |s: &mut Ui| s.select_folder(id))
            .child(label(role_glyph(&folder.role), font(12.0, 400.0), p.fg).w(18.0))
            .child(
                label(
                    display_folder_name(&folder.name).to_string(),
                    font(12.5, if active { 700.0 } else { 400.0 }),
                    if active { p.fg } else { p.muted },
                )
                .truncate()
                .grow(1.0)
                .min_w(0.0),
            );
        if folder.unread_count > 0 {
            b = b.child(label(
                folder.unread_count.to_string(),
                font(10.5, 700.0),
                p.accent,
            ));
        }
        list = list.child(b);
    }
    if folders.is_empty() {
        let syncing = s.sync_state(account_id) == SyncState::Syncing;
        list = list.child(row().px(14.0).py(8.0).child(label(
            if syncing {
                "Syncing folders…"
            } else {
                "No folders yet"
            },
            SMALL,
            p.muted,
        )));
    }
    col.child(list)
}

fn col_list() -> El {
    col()
        .key("folder-list")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
}

/// Shortcut hints, per-account sync dots, and the position readout.
pub fn footer(s: &Ui, p: &Palette) -> El {
    let hint = |keys: &[&str], what: &str| {
        let mut r = row().gap(3.0).px(5.0).center_y();
        for k in keys {
            r = r.child(kbd(k, p));
        }
        r.child(label(what.to_string(), SMALL, p.muted).ml(2.0))
    };
    let mut f = row()
        .key("footer")
        .w_full()
        .px(16.0)
        .py(6.0)
        .gap(2.0)
        .center_y()
        .border_sides([1.0, 0.0, 0.0, 0.0], p.border)
        .child(
            row()
                .key("footer-hints")
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .clip_rounded(0.0)
                .gap(2.0)
                .center_y()
                .child(hint(&["j", "k"], "navigate"))
                .child(hint(&["enter"], "open"))
                .child(hint(&["u"], "toggle unread"))
                .child(hint(&["c"], "compose"))
                .child(hint(&["r"], "reply"))
                .child(hint(&["a"], "archive"))
                .child(hint(&["d"], "delete"))
                .child(hint(&["f"], "flag"))
                .child(hint(&["m"], "move"))
                .child(hint(&["/"], "filter"))
                .child(hint(&["g", "g"], "top"))
                .child(hint(&["G"], "bottom"))
                .child(hint(&["1"], "unified"))
                .child(hint(&["2…"], "accounts"))
                .child(hint(&["⌃", "k"], "palette")),
        );
    for acc in &s.accounts {
        let state = s.sync_state(&acc.id);
        let name = match state {
            SyncState::Idle => "idle",
            SyncState::Syncing => "syncing",
            SyncState::Error => "error",
        };
        f = f.child(
            row()
                .key(format!("sync-{}", acc.id))
                .gap(4.0)
                .px(4.0)
                .center_y()
                .help(format!("{}: {name}", acc.email))
                .child(el().size(6.0, 6.0).radius(3.0).fill(sync_color(state, p)))
                .child(label(short_email(&acc.email).to_string(), SMALL, p.muted)),
        );
    }
    let total = s.visible_messages().len();
    let pos = if total > 0 {
        s.selected_index().map_or(0, |i| i + 1)
    } else {
        0
    };
    let scope = if s.unified {
        "unified".to_string()
    } else {
        s.selected_account
            .as_ref()
            .and_then(|id| s.accounts.iter().find(|a| &a.id == id))
            .map(|a| a.email.clone())
            .unwrap_or_else(|| "—".to_string())
    };
    f.child(
        label(
            format!(
                "message {pos} of {total}{} · {scope}",
                if s.has_more { "+" } else { "" }
            ),
            SMALL,
            p.muted,
        )
        .key("footer-meta")
        .ml(10.0),
    )
}

/// Transient notices, bottom-right.
pub fn toasts(s: &Ui, p: &Palette) -> El {
    let mut stack = col()
        .key("toasts")
        .role("status")
        .pin(None, None, Some(16.0), Some(40.0))
        .w(380.0)
        .gap(8.0)
        .items_end()
        .raise();
    for t in &s.toasts {
        let id = t.id;
        let tone = if t.error { p.error } else { p.accent };
        stack = stack.child(
            row()
                .key(format!("toast-{id}"))
                .w_full()
                .px(12.0)
                .py(8.0)
                .gap(10.0)
                .center_y()
                .radius(RADIUS)
                .fill(p.panel_2)
                .border(1.0, tone)
                .child(
                    text(t.message.clone())
                        .font(font(12.0, 400.0))
                        .color(p.fg)
                        .wrap()
                        .grow(1.0)
                        .min_w(0.0),
                )
                .child(
                    row()
                        .key(format!("toast-{id}-dismiss"))
                        .role("button")
                        .label("Dismiss")
                        .size(18.0, 18.0)
                        .center()
                        .cursor(Cursor::Pointer)
                        .on_press(move |s: &mut Ui| s.dismiss_toast(id))
                        .child(label("×", font(14.0, 400.0), p.muted)),
                ),
        );
    }
    stack
}
