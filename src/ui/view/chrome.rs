//! The frame around the panes: account rail, header, folder column, footer,
//! toasts, and the no-accounts welcome screen.

use rune::text::Span;
use rune::{col, el, rich, row, text, text_input, ChromeInsets, Cursor, El, Key, KeyEvent, Tx};

use super::icons;
use super::{btn, display, eyebrow, font, glyph, kbd, label, Kind, BODY, RADIUS, RADIUS_LG, SMALL};
use crate::accounts::AccountKind;
use crate::sync::SyncState;
use crate::ui::format::{avatar_slot, cap_badge, folder_title};
use crate::ui::theme::Palette;
use crate::ui::{AddTab, Ui};

/// Shown when no account is configured: what the app is, and the way in.
pub fn welcome(p: &Palette) -> El {
    let headline = "Your mail, kept close.";
    let accent_from = headline.find("kept").unwrap_or(0);
    let feature = |keys: El, words: &str| {
        row()
            .gap(12.0)
            .center_y()
            .child(row().w(76.0).gap(4.0).center_y().child(keys))
            .child(label(words.to_string(), BODY, p.fg))
    };
    let caps = |ks: &[&str]| {
        let mut r = row().gap(4.0);
        for k in ks {
            r = r.child(kbd(k, p));
        }
        r
    };
    let hero = col()
        .key("welcome-hero")
        .w(440.0)
        .shrink()
        .min_w(0.0)
        .gap(18.0)
        .child(eyebrow("Cosmic Mail", p))
        .child(
            rich(
                headline,
                vec![Span::new(accent_from, headline.len() - 1).color(p.accent)],
            )
            .font(display(52.0).lh(54.0))
            .color(p.fg)
            .wrap()
            .w_full(),
        )
        .child(
            text("A keyboard-driven client that keeps your mail on this computer, searches it offline and hears about new mail the moment it lands.")
                .font(font(15.0, 400.0).lh(23.0))
                .color(p.muted)
                .wrap()
                .w_full(),
        )
        .child(
            col()
                .gap(12.0)
                .mt(6.0)
                .child(feature(caps(&["j", "k", "c"]), "Move, read and compose without the mouse"))
                .child(feature(caps(&["ctrl", "k"]), "Every command, one search away"))
                .child(feature(
                    glyph(&icons::LOCK, 16.0, p.accent),
                    "Passwords in the keyring; remote images blocked",
                )),
        );

    let card = col()
        .key("welcome-card")
        .w(380.0)
        .shrink()
        .min_w(0.0)
        .radius(RADIUS_LG)
        .fill(p.panel)
        .border(1.0, p.border)
        .shadow(14.0, 48.0, p.shadow.alpha(if p.dark { 0.45 } else { 0.16 }))
        .child(
            col()
                .w_full()
                .px(22.0)
                .pt(20.0)
                .pb(16.0)
                .gap(6.0)
                .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
                .child(label("Add an account", display(24.0), p.fg))
                .child(label(
                    "Gmail signs in with Google. Anything else uses IMAP.",
                    SMALL,
                    p.muted,
                )),
        )
        .child(
            col()
                .w_full()
                .px(22.0)
                .py(20.0)
                .gap(12.0)
                .child(
                    btn(
                        "welcome-gmail",
                        "Sign in with Google",
                        Some(&icons::MAIL),
                        None,
                        Kind::Secondary,
                        true,
                        p,
                        |s, _| {
                            s.open_add_account();
                            s.set_add_tab(AddTab::Gmail);
                        },
                    )
                    .w_full(),
                )
                .child(
                    row()
                        .w_full()
                        .gap(10.0)
                        .center_y()
                        .child(el().grow(1.0).h(1.0).fill(p.border))
                        .child(label("or", SMALL, p.muted))
                        .child(el().grow(1.0).h(1.0).fill(p.border)),
                )
                .child(
                    btn(
                        "welcome-add",
                        "Add an IMAP account",
                        Some(&icons::PLUS),
                        None,
                        Kind::Primary,
                        true,
                        p,
                        |s, _| s.open_add_account(),
                    )
                    .w_full(),
                )
                .child(
                    text("Cosmic Mail looks up your provider's settings from the address.")
                        .font(SMALL)
                        .color(p.muted)
                        .wrap()
                        .w_full(),
                ),
        );

    col()
        .key("welcome")
        .w_full()
        .grow(1.0)
        .child(
            row()
                .px(32.0)
                .py(20.0)
                .gap(10.0)
                .center_y()
                .child(mark(p, 28.0))
                .child(
                    rich("Cosmic Mail", vec![Span::new(7, 11).color(p.accent)])
                        .font(display(19.0))
                        .color(p.fg),
                ),
        )
        .child(
            row()
                .w_full()
                .grow(1.0)
                .px(32.0)
                .pb(48.0)
                .gap(80.0)
                .center()
                .flex_wrap()
                .child(hero)
                .child(card),
        )
}

/// The app's mark: a lower-case c in the display face on the fill.
fn mark(p: &Palette, size: f32) -> El {
    row()
        .size(size, size)
        .center()
        .radius(RADIUS)
        .fill(p.fill)
        .a11y_hidden()
        .child(label("c", display(size * 0.68).lh(size * 0.7), p.fill_ink))
}

/// One square rail button with an optional unread count.
fn rail_button(
    key: String,
    tip: String,
    content: El,
    active: bool,
    badge: i64,
    p: &Palette,
    on: impl Fn(&mut Ui, &mut Tx) + 'static,
) -> El {
    let mut b = row()
        .key(key)
        .role("button")
        .label(tip.clone())
        .help(tip)
        .checked(active)
        .size(40.0, 40.0)
        .center()
        .radius(RADIUS)
        .border(
            1.0,
            if active {
                p.border
            } else {
                p.border.alpha(0.0)
            },
        )
        .fill(if active { p.panel } else { p.panel.alpha(0.0) })
        .hover(p.fg.alpha(0.06))
        .focusable(true)
        .cursor(Cursor::Pointer)
        .on_press_tx(on)
        .child(content);
    let count = cap_badge(badge);
    if !count.is_empty() {
        b = b.child(
            row()
                .absolute(26.0, -5.0)
                .min_w(17.0)
                .h(17.0)
                .px(4.0)
                .center()
                .radius(8.5)
                .fill(p.fill)
                .label(format!("{badge} unread"))
                .child(label(count, font(9.5, 700.0), p.fill_ink)),
        );
    }
    b
}

/// The account switcher down the left edge.
pub fn rail(s: &Ui, p: &Palette) -> El {
    let mut rail = col()
        .key("rail")
        .role("navigation")
        .label("Account switcher")
        .w(60.0)
        .min_w(60.0)
        .h_full()
        .pt(12.0)
        .pb(10.0)
        .gap(8.0)
        .items_center()
        .fill(p.rail)
        .border_sides([0.0, 1.0, 0.0, 0.0], p.border)
        .child(mark(p, 32.0))
        .child(el().w(26.0).h(1.0).my(4.0).fill(p.border));

    rail = rail.child(rail_button(
        "rail-unified".into(),
        "Unified inbox (1)".into(),
        glyph(&icons::INBOX, 19.0, if s.unified { p.fg } else { p.muted }),
        s.unified,
        s.unified_unread(),
        p,
        |s, _| s.select_unified(),
    ));
    for (i, acc) in s.accounts.iter().enumerate() {
        let id = acc.id.clone();
        let kind = match acc.kind {
            AccountKind::Gmail => "Gmail",
            AccountKind::Imap => "IMAP",
        };
        // The account's tag ("Gmail", "pluscosmic"), as one letter: two accounts of one
        // person share initials, not tags.
        let letter = account_tag(s, &acc.id)
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default();
        rail = rail.child(rail_button(
            format!("rail-account-{}", acc.id),
            format!("{} — {kind} ({})", acc.email, i + 2),
            row()
                .size(28.0, 28.0)
                .center()
                .radius(14.0)
                .fill(p.slot(avatar_slot(&acc.email)))
                .child(label(letter, font(12.0, 700.0), p.bg)),
            !s.unified && s.selected_account.as_deref() == Some(acc.id.as_str()),
            s.account_unread(&acc.id),
            p,
            move |s, _| s.select_account(&id),
        ));
    }

    rail.child(el().grow(1.0))
        .child(
            row()
                .key("rail-compose")
                .role("button")
                .label("Compose (c)")
                .help("Compose (c)")
                .size(40.0, 40.0)
                .center()
                .radius(RADIUS)
                .fill(p.fill)
                .hover(p.fill_ink.alpha(0.1))
                .focusable(true)
                .cursor(Cursor::Pointer)
                .on_press(|s: &mut Ui| s.open_compose())
                .child(glyph(&icons::PEN, 17.0, p.fill_ink)),
        )
        .child(rail_button(
            "rail-add".into(),
            "Add account".into(),
            glyph(&icons::PLUS, 17.0, p.muted),
            false,
            0,
            p,
            |s, _| s.open_add_account(),
        ))
        .child(rail_button(
            "rail-settings".into(),
            "Settings".into(),
            glyph(&icons::SLIDERS, 17.0, p.muted),
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
        SyncState::Syncing => p.warn,
        SyncState::Error => p.error,
    }
}

/// A short name for an account: "Gmail", or the first part of its domain.
pub fn account_tag(s: &Ui, account_id: &str) -> String {
    if s.is_gmail(account_id) {
        return "Gmail".into();
    }
    s.accounts
        .iter()
        .find(|a| a.id == account_id)
        .map(|a| {
            let domain = a.email.split('@').nth(1).unwrap_or(&a.email);
            domain.split('.').next().unwrap_or(domain).to_string()
        })
        .unwrap_or_default()
}

/// Title, search field (`with_search`: unless the titlebar has it), and sync status.
pub fn header(s: &Ui, p: &Palette, with_search: bool) -> El {
    let title = if s.unified {
        "Unified inbox".to_string()
    } else if let Some(f) = s.selected_folder() {
        folder_title(&f.name, &f.role).to_string()
    } else if let Some(acc) = s
        .selected_account
        .as_ref()
        .and_then(|id| s.accounts.iter().find(|a| &a.id == id))
    {
        acc.email.clone()
    } else {
        "No folder selected".to_string()
    };
    let subtitle = if s.unified {
        let n = s.accounts.len();
        format!(
            "{n} account{} · {} unread",
            if n == 1 { "" } else { "s" },
            s.unified_unread()
        )
    } else {
        let tag = s
            .selected_account
            .as_deref()
            .map(|id| account_tag(s, id))
            .unwrap_or_default();
        match s.selected_folder() {
            Some(f) => format!("{tag} · {} unread of {}", f.unread_count, f.total_count),
            None => tag,
        }
    };
    let title_el = if s.unified {
        rich(title, vec![Span::new(8, 13).color(p.accent)])
    } else {
        text(title)
    };

    let mut head = row()
        .key("header")
        .w_full()
        .px(22.0)
        .py(12.0)
        .gap(18.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .child(
            col()
                .gap(2.0)
                .shrink()
                .min_w(0.0)
                .child(eyebrow(subtitle, p).key("header-subtitle"))
                .child(
                    title_el
                        .font(display(25.0))
                        .color(p.fg)
                        .key("header-title")
                        .truncate(),
                ),
        )
        .child(el().grow(1.0));

    if s.search_active {
        let count = format!(
            "{}{} for “{}”",
            s.visible_messages().len(),
            if s.has_more { "+" } else { "" },
            s.search_query,
        );
        head = head.child(
            row()
                .key("search-status")
                .gap(8.0)
                .center_y()
                .child(label(count, SMALL, p.muted).truncate().shrink())
                .child(
                    btn(
                        "clear-search",
                        "Clear",
                        Some(&icons::CLOSE),
                        Some("esc"),
                        Kind::Ghost,
                        true,
                        p,
                        |s, _| s.clear_search(),
                    )
                    .label("Clear search"),
                ),
        );
    }

    if with_search {
        head = head.child(search_field(s, p, false));
    }

    let state = overall_sync(s);
    let status = match state {
        SyncState::Syncing => {
            let names: Vec<String> = s
                .accounts
                .iter()
                .filter(|a| s.sync_state(&a.id) == SyncState::Syncing)
                .map(|a| account_tag(s, &a.id))
                .collect();
            if names.len() == 1 {
                format!("Syncing {}…", names[0])
            } else {
                "Syncing…".to_string()
            }
        }
        SyncState::Error => "Sync error".to_string(),
        SyncState::Idle => "Up to date".to_string(),
    };
    head.child(
        row()
            .key("sync-status")
            .gap(7.0)
            .center_y()
            .child(el().size(7.0, 7.0).radius(3.5).fill(sync_color(state, p)))
            .child(label(status, font(12.5, 400.0), p.muted).key("sync-label")),
    )
}

/// The search field: in the header, or (`in_titlebar`) shorter and on the page colour in the
/// titlebar band, where it stands out from the band's rail colour.
fn search_field(s: &Ui, p: &Palette, in_titlebar: bool) -> El {
    let placeholder = match (s.search_active, in_titlebar) {
        (true, _) => "Search all mail…",
        (false, true) => "Search mail…",
        (false, false) => "Filter loaded messages…",
    };
    let input = text_input(&s.query, font(13.0, 400.0))
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
    let focused = s.query.focused || s.search_active;
    let (width, height, fill) = if in_titlebar {
        (TITLEBAR_SEARCH_WIDTH, 28.0, p.bg)
    } else {
        (340.0, 34.0, p.panel_2)
    };
    row()
        .key("search")
        .w(width)
        .min_w(160.0)
        .shrink()
        .h(height)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .radius(RADIUS)
        .fill(fill)
        .border(1.0, if focused { p.accent_dim } else { p.border })
        .child(glyph(
            &icons::SEARCH,
            if in_titlebar { 14.0 } else { 15.0 },
            p.muted,
        ))
        .child(input)
        .child(kbd(if s.search_active { "⏎" } else { "/" }, p))
}

/// The search field's width in the titlebar band.
const TITLEBAR_SEARCH_WIDTH: f32 = 360.0;

/// The band across the top of the window on Windows and macOS (`App::titlebar`): it moves the
/// window, carries the app's mark and name after the macOS traffic lights, and centres the
/// search field (`with_search`, once there are accounts) between them and the Windows caption
/// buttons, which `insets` keeps it clear of.
pub fn titlebar(s: &Ui, p: &Palette, insets: ChromeInsets, with_search: bool) -> El {
    let brand = row()
        .key("titlebar-brand")
        .grow(1.0)
        .min_w(0.0)
        .gap(8.0)
        .center_y()
        .child(mark(p, 16.0))
        .child(
            label("Cosmic Mail", font(12.5, 600.0), p.muted)
                .truncate()
                .shrink(),
        );
    row()
        .key("titlebar")
        .w_full()
        .h(insets.height)
        .min_h(insets.height)
        .pl(insets.leading + 12.0)
        .pr(insets.trailing + 12.0)
        .gap(12.0)
        .center_y()
        .fill(p.rail)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .window_drag()
        .child(brand)
        .children(with_search.then(|| search_field(s, p, true)))
        .child(el().grow(1.0).min_w(0.0))
}

/// The selected account's folders (per-account mode only).
pub fn folder_column(s: &Ui, p: &Palette) -> El {
    let (false, Some(account_id)) = (s.unified, s.selected_account.as_ref()) else {
        return el().key("no-folders").w(0.0);
    };
    let folders = s.folders.get(account_id).map(Vec::as_slice).unwrap_or(&[]);
    let mut column = col()
        .key("folders")
        .role("list")
        .label("Folders")
        .w(210.0)
        .min_w(210.0)
        .h_full()
        .min_h(0.0)
        .px(10.0)
        .py(16.0)
        .gap(2.0)
        .border_sides([0.0, 1.0, 0.0, 0.0], p.border);
    if let Some(acc) = s.accounts.iter().find(|a| &a.id == account_id) {
        column = column.child(
            col()
                .key("folders-head")
                .px(8.0)
                .pb(12.0)
                .gap(3.0)
                .help(acc.email.clone())
                .child(eyebrow("Account", p))
                .child(label(acc.email.clone(), font(13.0, 650.0), p.fg).truncate()),
        );
    }
    let mut list = col_list();
    for folder in folders {
        let id = folder.id;
        let active = s.selected_folder == Some(id);
        let name = folder_title(&folder.name, &folder.role).to_string();
        let mut b = row()
            .key(format!("folder-{id}"))
            .role("button")
            .label(name.clone())
            .checked(active)
            .w_full()
            .h(32.0)
            .px(8.0)
            .gap(10.0)
            .center_y()
            .radius(RADIUS)
            .border(
                1.0,
                if active {
                    p.border
                } else {
                    p.border.alpha(0.0)
                },
            )
            .fill(if active { p.panel } else { p.panel.alpha(0.0) })
            .hover(p.fg.alpha(0.05))
            .focusable(true)
            .cursor(Cursor::Pointer)
            .on_press(move |s: &mut Ui| s.select_folder(id))
            .child(glyph(
                icons::for_role(&folder.role),
                15.0,
                if active { p.fg } else { p.muted },
            ))
            .child(
                label(name, font(13.0, 600.0), if active { p.fg } else { p.muted })
                    .truncate()
                    .grow(1.0)
                    .min_w(0.0),
            );
        if folder.unread_count > 0 {
            b = b.child(label(
                folder.unread_count.to_string(),
                font(11.5, 650.0),
                p.accent,
            ));
        }
        list = list.child(b);
    }
    if folders.is_empty() {
        let syncing = s.sync_state(account_id) == SyncState::Syncing;
        list = list.child(row().px(8.0).py(8.0).child(label(
            if syncing {
                "Syncing folders…"
            } else {
                "No folders yet"
            },
            SMALL,
            p.muted,
        )));
    }
    let state = s.sync_state(account_id);
    column.child(list).child(
        row()
            .px(8.0)
            .pt(8.0)
            .gap(7.0)
            .center_y()
            .child(el().size(6.0, 6.0).radius(3.0).fill(sync_color(state, p)))
            .child(label(
                match state {
                    SyncState::Idle => "Up to date",
                    SyncState::Syncing => "Syncing…",
                    SyncState::Error => "Sync error",
                },
                SMALL,
                p.muted,
            )),
    )
}

fn col_list() -> El {
    col()
        .key("folder-list")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(2.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
}

/// Shortcut hints, per-account sync dots, and the position readout.
pub fn footer(s: &Ui, p: &Palette) -> El {
    let hint = |keys: &[&str], what: &str| {
        let mut r = row().gap(3.0).pr(12.0).center_y();
        for k in keys {
            r = r.child(kbd(k, p));
        }
        r.child(label(what.to_string(), SMALL, p.muted).ml(3.0))
    };
    let mut f = row()
        .key("footer")
        .w_full()
        .px(18.0)
        .py(6.0)
        .gap(14.0)
        .center_y()
        .fill(p.panel_2)
        .border_sides([1.0, 0.0, 0.0, 0.0], p.border)
        .child(
            row()
                .key("footer-hints")
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .clip_rounded(0.0)
                .center_y()
                .child(hint(&["j", "k"], "navigate"))
                .child(hint(&["c"], "compose"))
                .child(hint(&["r"], "reply"))
                .child(hint(&["a"], "archive"))
                .child(hint(&["f"], "flag"))
                .child(hint(&["m"], "move"))
                .child(hint(&["u"], "unread"))
                .child(hint(&["/"], "filter"))
                .child(hint(&["ctrl", "k"], "commands")),
        );
    for acc in &s.accounts {
        let state = s.sync_state(&acc.id);
        let name = match state {
            SyncState::Idle => "up to date",
            SyncState::Syncing => "syncing",
            SyncState::Error => "sync error",
        };
        f = f.child(
            row()
                .key(format!("sync-{}", acc.id))
                .gap(6.0)
                .center_y()
                .help(format!("{}: {name}", acc.email))
                .child(el().size(6.0, 6.0).radius(3.0).fill(sync_color(state, p)))
                .child(label(account_tag(s, &acc.id), SMALL, p.muted)),
        );
    }
    let total = s.visible_messages().len();
    let pos = if total > 0 {
        s.selected_index().map_or(0, |i| i + 1)
    } else {
        0
    };
    f.child(
        label(
            format!("{pos} of {total}{}", if s.has_more { "+" } else { "" }),
            SMALL,
            p.muted,
        )
        .key("footer-meta"),
    )
}

/// Transient notices, bottom-right: the tooltip colours, or a danger tint for errors.
pub fn toasts(s: &Ui, p: &Palette) -> El {
    let mut stack = col()
        .key("toasts")
        .role("status")
        .pin(None, None, Some(20.0), Some(48.0))
        .w(400.0)
        .gap(8.0)
        .items_end()
        .raise();
    for t in &s.toasts {
        let id = t.id;
        let (fill, edge, ink, mark) = if t.error {
            (
                rune_theme_foundation::mix(p.error, 0.1, p.bg),
                rune_theme_foundation::mix(p.error, 0.45, p.border),
                p.fg,
                glyph(&icons::ALERT, 16.0, p.error),
            )
        } else {
            (
                p.tip_bg,
                p.tip_bg,
                p.tip_ink,
                glyph(&icons::CHECK, 16.0, p.tip_ink),
            )
        };
        stack = stack.child(
            row()
                .key(format!("toast-{id}"))
                .w_full()
                .pl(14.0)
                .pr(8.0)
                .py(9.0)
                .gap(10.0)
                .center_y()
                .radius(RADIUS)
                .fill(fill)
                .border(1.0, edge)
                .shadow(10.0, 32.0, p.shadow.alpha(0.25))
                .child(mark)
                .child(
                    text(t.message.clone())
                        .font(font(12.5, 400.0).lh(18.0))
                        .color(ink)
                        .wrap()
                        .grow(1.0)
                        .min_w(0.0),
                )
                .child(
                    row()
                        .key(format!("toast-{id}-dismiss"))
                        .role("button")
                        .label("Dismiss")
                        .size(26.0, 26.0)
                        .center()
                        .radius(RADIUS)
                        .hover(ink.alpha(0.1))
                        .cursor(Cursor::Pointer)
                        .on_press(move |s: &mut Ui| s.dismiss_toast(id))
                        .child(glyph(&icons::CLOSE, 13.0, ink)),
                ),
        );
    }
    stack
}
