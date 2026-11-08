//! The element tree and keyboard handling.
//!
//! Layout: the account rail, then the main column — header, a split of the optional folder
//! column and the list/reader panes (resizable), and the shortcut footer. Modals and toasts
//! float above. The look is the UI foundation's: its tokens (through [`Palette`]), Instrument
//! Sans for text, Fraunces for display, 3 pt corners and small-capital eyebrows.

mod chrome;
pub mod icons;
mod list;
mod modals;
mod reader;

use rune::tok::TextStyle;
use rune::{col, el, icon, row, text, Color, Cursor, Cx, El, Icon, Key, KeyEvent, Phase, Tx};

use super::pane::{self, PANE_DIVIDER_WIDTH};
use super::theme::Palette;
use super::{Action, Command, Modal, PaletteState, Ui};
use crate::ui::format::display_folder_name;

/// Corner radius of controls (the foundation's `--radius`).
pub const RADIUS: f32 = 3.0;
/// Corner radius of cards (the foundation's `--radius-lg`).
pub const RADIUS_LG: f32 = 4.0;

/// The app's text style at `size` and `weight`, in the foundation's sans (Instrument Sans).
pub const fn font(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight)
}

pub const BODY: TextStyle = font(13.0, 400.0);
pub const SMALL: TextStyle = font(11.5, 400.0);
/// Buttons: semibold, as the foundation's.
pub const BUTTON: TextStyle = font(12.5, 600.0);

/// The display face (Fraunces) at `size`, for titles.
pub fn display(size: f32) -> TextStyle {
    rune_theme_foundation::display(size)
        .lh(size * 1.15)
        .tracking(-0.01 * size)
}

/// A one-line label.
pub fn label(s: impl Into<String>, style: TextStyle, color: Color) -> El {
    text(s.into()).font(style).color(color)
}

/// The small capitals over a heading (`ui-eyebrow`).
pub fn eyebrow(s: impl Into<String>, p: &Palette) -> El {
    let s: String = s.into();
    label(s.to_uppercase(), font(10.5, 650.0).tracking(1.2), p.muted)
}

/// An icon in a colour.
pub fn glyph(i: &'static Icon, size: f32, color: Color) -> El {
    icon(i, size).color(color)
}

/// How a button draws: filled (the one main action), bordered, or bare.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Primary,
    Secondary,
    Ghost,
}

/// A button with an optional icon before its caption and a shortcut after it. Disabled
/// buttons dim and drop their handler.
#[allow(clippy::too_many_arguments)]
pub fn btn(
    key: impl Into<String>,
    caption: impl Into<String>,
    leading: Option<&'static Icon>,
    shortcut: Option<&str>,
    kind: Kind,
    enabled: bool,
    p: &Palette,
    on: impl Fn(&mut Ui, &mut Tx) + 'static,
) -> El {
    let caption = caption.into();
    let (fill, edge, ink) = match kind {
        Kind::Primary => (p.fill, p.fill, p.fill_ink),
        Kind::Secondary => (p.panel, p.border, p.fg),
        Kind::Ghost => (p.panel.alpha(0.0), p.border.alpha(0.0), p.muted),
    };
    let mut b = row()
        .key(key.into())
        .role("button")
        .label(caption.clone())
        .h(30.0)
        .px(11.0)
        .gap(7.0)
        .center()
        .radius(RADIUS)
        .border(1.0, edge)
        .fill(fill);
    if let Some(i) = leading {
        b = b.child(glyph(i, 14.0, ink));
    }
    b = b.child(label(caption, BUTTON, ink));
    if let Some(k) = shortcut {
        b = b.child(if kind == Kind::Primary {
            row()
                .px(4.0)
                .h(16.0)
                .min_w(16.0)
                .center()
                .radius(RADIUS)
                .border(1.0, p.fill_ink.alpha(0.4))
                .child(label(k, font(10.0, 600.0), p.fill_ink))
        } else {
            kbd(k, p)
        });
    }
    if enabled {
        b.focusable(true)
            .cursor(Cursor::Pointer)
            .hover(if kind == Kind::Primary {
                p.fill_ink.alpha(0.1)
            } else {
                p.fg.alpha(0.05)
            })
            .on_press_tx(on)
    } else {
        b.disabled(true).opacity(0.5)
    }
}

/// A bordered (or, `primary`, filled) text button.
pub fn button(
    key: impl Into<String>,
    caption: impl Into<String>,
    p: &Palette,
    primary: bool,
    enabled: bool,
    on: impl Fn(&mut Ui, &mut Tx) + 'static,
) -> El {
    let kind = if primary {
        Kind::Primary
    } else {
        Kind::Secondary
    };
    btn(key, caption, None, None, kind, enabled, p, on)
}

/// A square, borderless icon button; `tip` names it for the pointer and screen readers.
pub fn icon_button(
    key: impl Into<String>,
    tip: impl Into<String>,
    i: &'static Icon,
    color: Color,
    p: &Palette,
    on: impl Fn(&mut Ui, &mut Tx) + 'static,
) -> El {
    let tip = tip.into();
    row()
        .key(key.into())
        .role("button")
        .label(tip.clone())
        .help(tip)
        .size(32.0, 32.0)
        .center()
        .radius(RADIUS)
        .hover(p.panel_2)
        .focusable(true)
        .cursor(Cursor::Pointer)
        .on_press_tx(on)
        .child(glyph(i, 16.0, color))
}

/// A keyboard-key cap (`ui-kbd`: a heavier bottom edge).
pub fn kbd(s: &str, p: &Palette) -> El {
    row()
        .px(4.0)
        .h(17.0)
        .min_w(17.0)
        .center()
        .radius(RADIUS)
        .fill(p.panel)
        .border_sides([1.0, 1.0, 2.0, 1.0], p.border)
        .child(label(s, font(10.0, 600.0), p.muted))
}

/// A text field box around an input element (`ui-input`: recessed, accent edge when focused).
pub fn field_box(key: impl Into<String>, focused: bool, p: &Palette, input: El) -> El {
    row()
        .key(key.into())
        .w_full()
        .h(34.0)
        .px(10.0)
        .center_y()
        .radius(RADIUS)
        .fill(p.panel_2)
        .border(1.0, if focused { p.accent } else { p.border })
        .child(input)
}

/// The root view.
pub fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
    let palette = s.palette(cx.system_dark());
    let p = &palette;
    let mut root = col().key("app").w_full().h_full().fill(p.bg);

    if !s.loading_accounts && s.accounts.is_empty() {
        root = root.child(chrome::welcome(p));
    } else {
        let main = col()
            .key("main")
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .min_h(0.0)
            .child(chrome::header(s, p))
            .child(
                row()
                    .key("split")
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .child(chrome::folder_column(s, p))
                    .child(panes(s, cx, p)),
            )
            .child(chrome::footer(s, p));
        root = root.child(
            row()
                .key("shell")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(chrome::rail(s, p))
                .child(main),
        );
    }

    root = root.child(chrome::toasts(s, p));
    if let Some(m) = &s.modal {
        root = root.child(modals::modal(s, m, p));
    }
    root
}

/// The list and reader with the draggable divider between them.
fn panes(s: &Ui, cx: &mut Cx<Ui>, p: &Palette) -> El {
    let available = cx
        .layout_size("panes")
        .map(|[w, _]| (w - PANE_DIVIDER_WIDTH).max(0.0))
        .unwrap_or(900.0);
    let list_width = pane::resolve_list_pane_width(available, s.pane_ratio);
    let limits = pane::pane_width_limits(available);

    let divider_line = el().w(1.0).h_full().fill(if s.dragging_divider {
        p.accent
    } else {
        p.border
    });
    let divider = row()
        .key("pane-divider")
        .role("separator")
        .label("Resize message list and reader")
        .help("Drag to resize · double-click to reset")
        .w(PANE_DIVIDER_WIDTH)
        .h_full()
        .justify_center()
        .focusable(true)
        .cursor(Cursor::ResizeColumn)
        .child(divider_line)
        .on_drag(
            move |s: &mut Ui, d: &mut rune::Drag, _tx: &mut Tx| match d.phase {
                Phase::Down => {
                    s.dragging_divider = true;
                    s.drag_start_width = list_width;
                }
                Phase::Move => {
                    let w = (s.drag_start_width + d.dx).clamp(limits.minimum, limits.maximum);
                    s.pane_ratio = pane::ratio_for_list_pane_width(available, w);
                }
                Phase::Up => {
                    s.dragging_divider = false;
                    s.persist_pane_ratio();
                }
            },
        )
        .on_double_press(|s: &mut Ui| {
            s.pane_ratio = pane::DEFAULT_LIST_PANE_RATIO;
            s.persist_pane_ratio();
        })
        .on_key(move |s: &mut Ui, ev: &KeyEvent, _tx: &mut Tx| {
            let step = if ev.mods.shift { 80.0 } else { 24.0 };
            let requested = match ev.key {
                Key::Left => list_width - step,
                Key::Right => list_width + step,
                Key::Home => limits.minimum,
                Key::End => limits.maximum,
                _ => return false,
            };
            let w = requested.clamp(limits.minimum, limits.maximum);
            s.pane_ratio = pane::ratio_for_list_pane_width(available, w);
            s.persist_pane_ratio();
            true
        });

    row()
        .key("panes")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0)
        .child(list::list_pane(s, p).w(list_width).h_full().min_h(0.0))
        .child(divider)
        .child(
            reader::reader(s, p)
                .grow(1.0)
                .min_w(0.0)
                .h_full()
                .min_h(0.0),
        )
}

impl Ui {
    /// Save the list/reader split for the next launch.
    pub fn persist_pane_ratio(&mut self) {
        if let Some(prefs) = &mut self.prefs {
            if let Err(err) = prefs.set("listPaneRatio", &self.pane_ratio) {
                tracing::debug!(error = %err, "could not persist pane ratio");
            }
        }
    }

    /// The commands the palette offers for the current selection.
    pub fn palette_commands(&self) -> Vec<Command> {
        let mut cmds = Vec::new();
        let mut push =
            |id: String, title: String, hint: Option<&'static str>, kw: &[&str], action| {
                cmds.push(Command {
                    id,
                    title,
                    hint,
                    keywords: kw.iter().map(|k| k.to_string()).collect(),
                    action,
                })
            };
        push(
            "unified".into(),
            "Unified inbox".into(),
            None,
            &["all", "inbox"],
            Action::Unified,
        );
        for acc in &self.accounts {
            push(
                format!("account:{}", acc.id),
                format!("Open inbox — {}", acc.email),
                None,
                &["account", "inbox", &acc.display_name],
                Action::OpenAccount(acc.id.clone()),
            );
        }
        if let (false, Some(account)) = (self.unified, &self.selected_account) {
            for folder in self.folders.get(account).into_iter().flatten() {
                push(
                    format!("folder:{}", folder.id),
                    format!("Go to folder — {}", display_folder_name(&folder.name)),
                    None,
                    &["folder", &folder.role],
                    Action::GoFolder(folder.id),
                );
            }
        }
        push(
            "compose".into(),
            "Compose".into(),
            Some("c"),
            &["new", "write", "mail"],
            Action::Compose,
        );
        let selected = self.selected_message();
        if let Some(m) = selected {
            if self.body.as_ref().is_some_and(|b| b.id == m.id) {
                push(
                    "reply".into(),
                    "Reply".into(),
                    Some("r"),
                    &["respond"],
                    Action::Reply { all: false },
                );
                push(
                    "reply-all".into(),
                    "Reply all".into(),
                    None,
                    &["respond", "everyone"],
                    Action::Reply { all: true },
                );
            }
            let flag_title = if m.flagged {
                "Remove flag"
            } else {
                "Flag message"
            };
            push(
                "toggle-read".into(),
                "Toggle read".into(),
                Some("u"),
                &["unread", "seen", "mark"],
                Action::ToggleRead,
            );
            push(
                "archive-message".into(),
                "Archive message".into(),
                Some("a"),
                &["archive"],
                Action::Archive,
            );
            push(
                "delete-message".into(),
                "Delete message".into(),
                Some("d"),
                &["trash", "remove", "delete"],
                Action::Delete,
            );
            push(
                "toggle-flag".into(),
                flag_title.into(),
                Some("f"),
                &["flag", "star", "important"],
                Action::ToggleFlag,
            );
            push(
                "move-message".into(),
                "Move message…".into(),
                Some("m"),
                &["move", "folder"],
                Action::MovePicker,
            );
        }
        if !self.unified && self.selected_folder.is_some() {
            push(
                "sync-folder".into(),
                "Sync current folder".into(),
                None,
                &["refresh", "fetch"],
                Action::SyncFolder,
            );
        }
        for acc in &self.accounts {
            push(
                format!("sync-account:{}", acc.id),
                format!("Sync account — {}", acc.email),
                None,
                &["refresh", "fetch"],
                Action::SyncAccount(acc.id.clone()),
            );
        }
        push(
            "add-account".into(),
            "Add account".into(),
            None,
            &["new", "imap", "gmail"],
            Action::AddAccount,
        );
        push(
            "open-settings".into(),
            "Open settings".into(),
            None,
            &["preferences", "config", "options", "remote images"],
            Action::OpenSettings,
        );
        push(
            "focus-search".into(),
            "Search all mail".into(),
            Some("/"),
            &["filter", "find", "search", "query"],
            Action::FocusSearch,
        );
        cmds
    }

    pub fn open_palette(&mut self) {
        let cmds = self.palette_commands();
        self.modal = Some(Modal::Palette(Box::new(PaletteState::new(
            cmds,
            "Run a command…",
        ))));
    }

    /// The palette as a folder picker for moving the selected message.
    pub fn open_move_picker(&mut self) {
        let Some(m) = self.selected_message().cloned() else {
            return;
        };
        let cmds: Vec<Command> = self
            .folders
            .get(&m.account_id)
            .into_iter()
            .flatten()
            .filter(|f| f.id != m.folder_id)
            .map(|f| Command {
                id: format!("move:{}", f.id),
                title: format!("Move to — {}", display_folder_name(&f.name)),
                hint: None,
                keywords: vec!["move".into(), f.role.clone(), f.name.clone()],
                action: Action::MoveTo {
                    message_id: m.id,
                    folder_id: f.id,
                },
            })
            .collect();
        if !cmds.is_empty() {
            self.modal = Some(Modal::Palette(Box::new(PaletteState::new(
                cmds,
                "Move to folder…",
            ))));
        }
    }

    /// Run a palette command (the palette has already closed).
    pub fn run_action(&mut self, action: Action, tx: &mut Tx) {
        let selected = self.selected_message;
        match action {
            Action::Unified => self.select_unified(),
            Action::OpenAccount(id) => self.select_account(&id),
            Action::GoFolder(id) => self.select_folder(id),
            Action::Compose => self.open_compose(),
            Action::Reply { all } => self.open_reply(all),
            Action::ToggleRead => selected.into_iter().for_each(|id| self.toggle_read(id)),
            Action::Archive => selected.into_iter().for_each(|id| self.archive_message(id)),
            Action::Delete => selected.into_iter().for_each(|id| self.delete_message(id)),
            Action::ToggleFlag => selected.into_iter().for_each(|id| self.toggle_flagged(id)),
            Action::MovePicker => self.open_move_picker(),
            Action::MoveTo {
                message_id,
                folder_id,
            } => self.move_message(message_id, folder_id),
            Action::SyncFolder => self.sync_selected_folder(),
            Action::SyncAccount(id) => self.sync_account(id),
            Action::AddAccount => self.open_add_account(),
            Action::OpenSettings => self.open_settings(),
            Action::FocusSearch => self.query.focused = true,
        }
        let _ = tx;
    }
}

/// Keys that reach the window (nothing focused, or the focused control passed
/// them on). Mirrors the web UI's global keydown handler.
pub fn on_key(s: &mut Ui, ev: &KeyEvent, tx: &mut Tx) -> bool {
    // Ctrl+K opens the palette from anywhere, but never over another modal.
    if ev.is_command('k') {
        if s.modal.is_none() {
            s.open_palette();
        }
        return true;
    }
    if s.modal.is_some() || ev.mods.cmd || ev.mods.ctrl || ev.mods.alt {
        return false;
    }
    // A focused text field keeps its own keys.
    if s.query.focused {
        return false;
    }
    let Key::Text(t) = &ev.key else {
        return match ev.key {
            Key::Enter => {
                if let Some(id) = s.selected_message {
                    tx.scroll_to_y("reader-body", 0.0);
                    s.reveal = Some(format!("msg-{id}"));
                }
                true
            }
            Key::Escape => {
                tx.blur();
                true
            }
            Key::Down => {
                move_selection(s, 1, tx);
                true
            }
            Key::Up => {
                move_selection(s, -1, tx);
                true
            }
            _ => false,
        };
    };
    let is_g = t == "g";
    let handled = match t.as_str() {
        "/" => {
            s.query.focused = true;
            true
        }
        "j" => {
            move_selection(s, 1, tx);
            true
        }
        "k" => {
            move_selection(s, -1, tx);
            true
        }
        "G" => {
            s.jump_to_bottom(tx);
            true
        }
        "g" => {
            let now = tx.input_time().unwrap_or_else(rune::time::Instant::now);
            match s.last_g.take() {
                Some(prev) if now.duration_since(prev).as_secs_f64() < 0.5 => {
                    s.select_index(0, tx);
                }
                _ => s.last_g = Some(now),
            }
            true
        }
        " " => {
            tx.scroll_page("reader-body", if ev.mods.shift { -1 } else { 1 });
            true
        }
        "u" => {
            if let Some(id) = s.selected_message {
                s.toggle_read(id);
            }
            true
        }
        "c" => {
            s.open_compose();
            true
        }
        "r" => {
            s.open_reply(false);
            true
        }
        "a" => {
            if let Some(id) = s.selected_message {
                s.archive_message(id);
            }
            true
        }
        "d" => {
            if let Some(id) = s.selected_message {
                s.delete_message(id);
            }
            true
        }
        "f" => {
            if let Some(id) = s.selected_message {
                s.toggle_flagged(id);
            }
            true
        }
        "m" => {
            s.open_move_picker();
            true
        }
        "1" => {
            s.select_unified();
            true
        }
        d if d.len() == 1 && ('2'..='9').contains(&d.chars().next().unwrap_or(' ')) => {
            let idx = d.parse::<usize>().unwrap_or(2) - 2;
            if let Some(id) = s.accounts.get(idx).map(|a| a.id.clone()) {
                s.select_account(&id);
            }
            true
        }
        _ => false,
    };
    if !is_g {
        s.last_g = None;
    }
    handled
}

/// `j`/`k`: move the selection by `delta` rows (the first row when none).
fn move_selection(s: &mut Ui, delta: isize, tx: &mut Tx) {
    let next = match s.selected_index() {
        None => 0,
        Some(i) => (i as isize + delta).max(0) as usize,
    };
    s.select_index(next, tx);
}
