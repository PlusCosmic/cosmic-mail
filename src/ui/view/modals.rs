//! Dialogs: compose, add account, settings, and the command palette.

use rune::{col, el, row, text, text_input, Cursor, El, Key, KeyEvent, ScrollSpec, TextEdit, Tx};

use super::{button, field_box, font, kbd, label, BODY, RADIUS, SMALL};
use crate::autoconfig::{DiscoveredKind, DiscoverySource};
use crate::ui::theme::{mix, scheme_words, Appearance, Palette};
use crate::ui::{AddAccountForm, AddTab, ComposeForm, Modal, PaletteState, SettingsState, Ui};

/// The open modal over a dimming scrim.
pub fn modal(s: &Ui, m: &Modal, p: &Palette) -> El {
    let (dialog, top) = match m {
        Modal::AddAccount(f) => (add_account(s, f, p), false),
        Modal::Settings(st) => (settings(s, st, p), false),
        Modal::Compose(f) => (compose(s, f, p), false),
        Modal::Palette(st) => (command_palette(st, p), true),
    };
    let dialog = dialog
        .fill(p.bg)
        .border(1.0, p.border_strong)
        .radius(RADIUS)
        .modal(true, |s: &mut Ui| s.close_modal());
    let scrim = col()
        .key("scrim")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .fill(p.bg.alpha(0.6))
        .items_center()
        .raise();
    if top {
        scrim.pt(80.0).child(dialog)
    } else {
        scrim.justify_center().child(dialog)
    }
}

fn dialog_head(title: &str, p: &Palette, enabled: bool) -> El {
    let mut close = row()
        .key("dialog-close")
        .role("button")
        .label("Close")
        .size(24.0, 24.0)
        .center()
        .radius(RADIUS)
        .child(label("×", font(16.0, 400.0), p.muted));
    if enabled {
        close = close
            .cursor(Cursor::Pointer)
            .hover(p.fg.alpha(0.07))
            .on_press(|s: &mut Ui| s.close_modal());
    }
    row()
        .key("dialog-head")
        .w_full()
        .px(16.0)
        .py(10.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .child(label(title.to_string(), font(13.0, 700.0), p.fg).grow(1.0))
        .child(close)
}

/// A labelled single-line field.
#[allow(clippy::too_many_arguments)]
fn field(
    key: &'static str,
    caption: &str,
    edit: &TextEdit,
    placeholder: &str,
    p: &Palette,
    enabled: bool,
    access: impl for<'a> Fn(&'a mut Ui) -> &'a mut TextEdit + 'static,
    extra: impl FnOnce(El) -> El,
) -> El {
    let mut input = text_input(edit, BODY)
        .key(key)
        .label(caption.to_string())
        .color(p.fg)
        .placeholder(placeholder.to_string(), p.muted_2)
        .single_line()
        .grow(1.0)
        .min_w(0.0)
        .on_edit(access);
    if !enabled {
        input = input.read_only(true);
    }
    input = extra(input);
    col()
        .w_full()
        .gap(4.0)
        .child(label(caption.to_string(), SMALL, p.muted))
        .child(field_box(format!("{key}-box"), edit.focused, p, input))
}

/// The open compose dialog's field. When the dialog closed under a pending
/// event, a scratch edit takes it (and the edit goes nowhere).
macro_rules! compose_field {
    ($name:ident) => {
        |s: &mut Ui| match &mut s.modal {
            Some(Modal::Compose(f)) => &mut f.$name,
            _ => &mut s.scratch_edit,
        }
    };
}

/// The open add-account dialog's field (scratch when it closed).
macro_rules! account_field {
    ($name:ident) => {
        |s: &mut Ui| match &mut s.modal {
            Some(Modal::AddAccount(f)) => &mut f.$name,
            _ => &mut s.scratch_edit,
        }
    };
}

/// Ctrl+Enter sends from any compose field.
fn send_on_ctrl_enter(e: El) -> El {
    e.on_input_key(|s: &mut Ui, ev: &KeyEvent, _tx: &mut Tx| {
        if ev.key == Key::Enter && (ev.mods.cmd || ev.mods.ctrl) {
            s.send_message();
            return true;
        }
        false
    })
}

fn compose(s: &Ui, f: &ComposeForm, p: &Palette) -> El {
    let enabled = !f.busy;
    let title = if f.seed.reply_to_message_id.is_none() {
        "New message"
    } else {
        "Reply"
    };

    // From: one chip per account (fixed for replies).
    let mut from = row().key("compose-from").gap(6.0).flex_wrap();
    for acc in &s.accounts {
        let on = acc.id == f.account_id;
        let id = acc.id.clone();
        let name = if acc.display_name.is_empty() {
            acc.email.clone()
        } else {
            format!("{} <{}>", acc.display_name, acc.email)
        };
        let mut chip = row()
            .key(format!("from-{}", acc.id))
            .role("radio")
            .label(name.clone())
            .checked(on)
            .px(9.0)
            .h(24.0)
            .center()
            .radius(RADIUS)
            .border(1.0, if on { p.accent_dim } else { p.border })
            .fill(if on {
                p.accent_soft
            } else {
                p.accent_soft.alpha(0.0)
            })
            .child(label(
                name,
                font(12.0, 400.0),
                if on { p.fg } else { p.muted },
            ));
        if enabled && f.seed.reply_to_message_id.is_none() {
            chip = chip
                .focusable(true)
                .cursor(Cursor::Pointer)
                .hover(p.fg.alpha(0.06))
                .on_press(move |s: &mut Ui| {
                    if let Some(Modal::Compose(f)) = &mut s.modal {
                        f.account_id = id.clone();
                    }
                });
        }
        from = from.child(chip);
    }

    let body_input = send_on_ctrl_enter(
        text_input(&f.body, BODY.lh(13.0 * 1.45))
            .key("compose-body")
            .label("Message body")
            .color(p.fg)
            .grow(1.0)
            .min_w(0.0)
            .wrap()
            .on_edit(compose_field!(body)),
    );
    let body_input = if enabled {
        body_input
    } else {
        body_input.read_only(true)
    };
    let body_box = col()
        .key("compose-body-box")
        .w_full()
        .h(260.0)
        .p_box(p, f.body.focused)
        .child(body_input.w_full())
        .scroll(ScrollSpec {
            stick_bottom: false,
        });

    let mut c = col()
        .key("compose")
        .role("dialog")
        .label("Compose message")
        .w(640.0)
        .max_w(900.0)
        .child(dialog_head(title, p, enabled));
    let mut fields = col()
        .w_full()
        .px(16.0)
        .py(12.0)
        .gap(10.0)
        .child(
            col()
                .gap(4.0)
                .child(label("From", SMALL, p.muted))
                .child(from),
        )
        .child(field(
            "compose-to",
            "To",
            &f.to,
            "",
            p,
            enabled,
            compose_field!(to),
            send_on_ctrl_enter,
        ))
        .child(
            row()
                .w_full()
                .gap(10.0)
                .child(
                    field(
                        "compose-cc",
                        "Cc",
                        &f.cc,
                        "",
                        p,
                        enabled,
                        compose_field!(cc),
                        send_on_ctrl_enter,
                    )
                    .grow(1.0)
                    .min_w(0.0),
                )
                .child(
                    field(
                        "compose-bcc",
                        "Bcc",
                        &f.bcc,
                        "",
                        p,
                        enabled,
                        compose_field!(bcc),
                        send_on_ctrl_enter,
                    )
                    .grow(1.0)
                    .min_w(0.0),
                ),
        )
        .child(field(
            "compose-subject",
            "Subject",
            &f.subject,
            "",
            p,
            enabled,
            compose_field!(subject),
            send_on_ctrl_enter,
        ))
        .child(
            col()
                .w_full()
                .gap(4.0)
                .child(label("Message", SMALL, p.muted))
                .child(body_box),
        );
    if let Some(err) = &f.error {
        fields = fields.child(error_line(err, p));
    }
    c = c.child(fields);
    c.child(
        row()
            .key("compose-actions")
            .w_full()
            .px(16.0)
            .py(10.0)
            .gap(8.0)
            .center_y()
            .border_sides([1.0, 0.0, 0.0, 0.0], p.border)
            .child(
                row()
                    .gap(3.0)
                    .center_y()
                    .child(kbd("ctrl", p))
                    .child(label("+", SMALL, p.muted))
                    .child(kbd("enter", p))
                    .child(label("send", SMALL, p.muted).ml(3.0)),
            )
            .child(el().grow(1.0))
            .child(button(
                "compose-cancel",
                "Cancel",
                p,
                false,
                enabled,
                |s, _| s.close_modal(),
            ))
            .child(button(
                "compose-send",
                if f.busy { "Sending…" } else { "Send" },
                p,
                true,
                enabled,
                |s, _| s.send_message(),
            )),
    )
}

/// A bordered multi-line box (padding plus a focus-coloured border).
trait BoxExt {
    fn p_box(self, p: &Palette, focused: bool) -> El;
}

impl BoxExt for El {
    fn p_box(self, p: &Palette, focused: bool) -> El {
        self.padding(8.0)
            .radius(RADIUS)
            .fill(p.bg)
            .border(1.0, if focused { p.accent } else { p.border })
    }
}

fn error_line(err: &str, p: &Palette) -> El {
    text(err.to_string())
        .key("dialog-error")
        .font(font(12.0, 400.0))
        .color(p.error)
        .wrap()
        .w_full()
}

fn tab(key: &'static str, caption: &'static str, on: bool, which: AddTab, p: &Palette) -> El {
    row()
        .key(key)
        .role("tab")
        .label(caption)
        .checked(on)
        .px(14.0)
        .h(30.0)
        .center()
        .border_sides(
            [0.0, 0.0, 2.0, 0.0],
            if on { p.accent } else { p.accent.alpha(0.0) },
        )
        .focusable(true)
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut Ui| s.set_add_tab(which))
        .child(label(caption, BODY, if on { p.fg } else { p.muted }))
}

fn add_account(_s: &Ui, f: &AddAccountForm, p: &Palette) -> El {
    let enabled = !f.busy;
    let tabs = row()
        .key("add-tabs")
        .role("tablist")
        .w_full()
        .px(16.0)
        .gap(4.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .child(tab(
            "tab-imap",
            "IMAP",
            f.tab == AddTab::Imap,
            AddTab::Imap,
            p,
        ))
        .child(tab(
            "tab-gmail",
            "Gmail",
            f.tab == AddTab::Gmail,
            AddTab::Gmail,
            p,
        ));

    let mut body = col().key("add-body").w_full().px(16.0).py(14.0).gap(10.0);
    match f.tab {
        AddTab::Imap => {
            let pair = |a: El, b: El, narrow_b: bool| {
                let b = if narrow_b {
                    b.w(110.0)
                } else {
                    b.grow(1.0).min_w(0.0)
                };
                row()
                    .w_full()
                    .gap(10.0)
                    .child(a.grow(1.0).min_w(0.0))
                    .child(b)
            };
            body = body.child(pair(
                field(
                    "add-email",
                    "Email",
                    &f.email,
                    "you@example.com",
                    p,
                    enabled,
                    account_field!(email),
                    |e| {
                        e.on_input_change(|s: &mut Ui, _tx: &mut Tx| s.email_edited())
                            .on_submit(|s: &mut Ui, _tx: &mut Tx| s.run_discovery())
                    },
                ),
                field(
                    "add-display-name",
                    "Display name",
                    &f.display_name,
                    "",
                    p,
                    enabled,
                    account_field!(display_name),
                    |e| e,
                ),
                false,
            ));
            if let Some(status) = discovery_status(f, p) {
                body = body.child(status);
            }
            body = body
                .child(pair(
                    field(
                        "add-imap-host",
                        "IMAP host",
                        &f.imap_host,
                        "imap.example.com",
                        p,
                        enabled,
                        account_field!(imap_host),
                        |e| e,
                    ),
                    field(
                        "add-imap-port",
                        "IMAP port",
                        &f.imap_port,
                        "993",
                        p,
                        enabled,
                        account_field!(imap_port),
                        |e| e,
                    ),
                    true,
                ))
                .child(pair(
                    field(
                        "add-smtp-host",
                        "SMTP host",
                        &f.smtp_host,
                        "smtp.example.com",
                        p,
                        enabled,
                        account_field!(smtp_host),
                        |e| e,
                    ),
                    field(
                        "add-smtp-port",
                        "SMTP port",
                        &f.smtp_port,
                        "587",
                        p,
                        enabled,
                        account_field!(smtp_port),
                        |e| e,
                    ),
                    true,
                ))
                .child(pair(
                    field(
                        "add-username",
                        "Username",
                        &f.username,
                        "defaults to email",
                        p,
                        enabled,
                        account_field!(username),
                        |e| e,
                    ),
                    field(
                        "add-password",
                        "Password",
                        &f.password,
                        "",
                        p,
                        enabled,
                        account_field!(password),
                        |e| {
                            e.secure()
                                .on_submit(|s: &mut Ui, _tx: &mut Tx| s.submit_imap_account())
                        },
                    ),
                    false,
                ));
            if let Some(err) = &f.error {
                body = body.child(error_line(err, p));
            }
            body = body.child(
                row()
                    .w_full()
                    .gap(8.0)
                    .child(el().grow(1.0))
                    .child(button("add-cancel", "Cancel", p, false, enabled, |s, _| {
                        s.close_modal()
                    }))
                    .child(button(
                        "add-submit",
                        if f.busy {
                            "Connecting…"
                        } else {
                            "Add account"
                        },
                        p,
                        true,
                        enabled,
                        |s, _| s.submit_imap_account(),
                    )),
            );
        }
        AddTab::Gmail => {
            body = body.child(
                text(
                    "Sign in with Google to add a Gmail account. A browser window will open \
                     for consent; this window waits until you finish.",
                )
                .font(font(12.5, 400.0))
                .color(p.muted)
                .wrap()
                .w_full(),
            );
            if let Some(err) = &f.error {
                body = body.child(error_line(err, p));
            }
            body = body.child(row().w_full().justify_center().child(button(
                "gmail-signin",
                if f.busy {
                    "Waiting for Google…"
                } else {
                    "Sign in with Google"
                },
                p,
                true,
                enabled,
                |s, _| s.start_gmail_oauth(),
            )));
        }
    }

    col()
        .key("add-account")
        .role("dialog")
        .label("Add account")
        .w(560.0)
        .child(dialog_head("Add account", p, enabled))
        .child(tabs)
        .child(body)
}

/// The inline autodiscovery status line under the email field.
fn discovery_status(f: &AddAccountForm, p: &Palette) -> Option<El> {
    let line = |msg: &str, color| {
        row()
            .key("discovery")
            .w_full()
            .px(10.0)
            .py(6.0)
            .gap(8.0)
            .center_y()
            .radius(RADIUS)
            .fill(p.panel)
            .border(1.0, p.border)
            .child(
                text(msg.to_string())
                    .font(font(12.0, 400.0))
                    .color(color)
                    .wrap()
                    .grow(1.0)
                    .min_w(0.0),
            )
    };
    if f.detecting {
        return Some(line("Detecting settings…", p.muted));
    }
    let d = f.discovered.as_ref()?;
    if d.kind == DiscoveredKind::Gmail {
        return Some(
            line(
                "This address is hosted by Google — use Google sign-in instead",
                p.fg,
            )
            .child(button("use-gmail", "Use Gmail", p, true, true, |s, _| {
                s.set_add_tab(AddTab::Gmail)
            })),
        );
    }
    let (msg, color) = match d.source {
        DiscoverySource::Ispdb => ("Settings found (Thunderbird ISPDB)", p.ok),
        DiscoverySource::Autoconfig => ("Settings found (provider autoconfig)", p.ok),
        DiscoverySource::Mx => ("Settings found (via MX lookup)", p.ok),
        DiscoverySource::Srv => ("Settings found (DNS SRV)", p.ok),
        DiscoverySource::Guess => (
            "Could not verify — guessed common defaults, please check",
            p.warn,
        ),
    };
    Some(line(msg, color))
}

/// A labelled checkbox row in the settings dialog.
fn setting_toggle(
    key: &'static str,
    title: &'static str,
    description: &'static str,
    checked: bool,
    p: &Palette,
    toggle: fn(&mut crate::settings::Settings),
) -> El {
    let check = row()
        .size(16.0, 16.0)
        .center()
        .radius(3.0)
        .border(1.0, if checked { p.accent } else { p.border_strong })
        .fill(if checked { p.accent } else { p.bg })
        .child(label(
            if checked { "✓" } else { "" },
            font(11.0, 700.0),
            p.bg,
        ));
    row()
        .key(key)
        .role("checkbox")
        .label(title)
        .checked(checked)
        .w_full()
        .gap(10.0)
        .focusable(true)
        .cursor(Cursor::Pointer)
        .on_press(move |s: &mut Ui| {
            let mut next = s.settings.clone();
            toggle(&mut next);
            s.update_settings(next);
        })
        .child(check.mt(2.0))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(3.0)
                .child(label(title, BODY, p.fg))
                .child(
                    text(description)
                        .font(font(11.5, 400.0))
                        .color(p.muted)
                        .wrap()
                        .w_full(),
                ),
        )
}

/// The foundation's colour schemes as cards, and light, dark or the desktop's choice.
fn appearance(s: &Ui, p: &Palette) -> El {
    let mut modes = row()
        .key("appearance")
        .self_start()
        .role("radiogroup")
        .label("Appearance")
        .gap(2.0)
        .px(3.0)
        .py(3.0)
        .radius(RADIUS)
        .fill(p.panel_2)
        .border(1.0, p.border);
    for a in Appearance::ALL {
        let chosen = a == s.appearance;
        modes = modes.child(
            row()
                .key(format!("appearance-{}", a.name()))
                .role("radio")
                .label(a.label())
                .checked(chosen)
                .h(26.0)
                .px(14.0)
                .center()
                .radius(RADIUS)
                .fill(if chosen { p.panel } else { p.panel_2 })
                .focusable(true)
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut Ui| s.set_appearance(a))
                .child(label(
                    a.label(),
                    font(12.5, 600.0),
                    if chosen { p.fg } else { p.muted },
                )),
        );
    }
    let schemes = rune_theme_foundation::parts::scheme_picker(
        s.scheme,
        p.dark,
        scheme_words,
        |s: &mut Ui, scheme, _tx: &mut Tx| s.set_scheme(scheme),
    );
    col().w_full().gap(12.0).child(modes).child(schemes)
}

fn settings(s: &Ui, st: &SettingsState, p: &Palette) -> El {
    let remote_row = setting_toggle(
        "setting-remote-images",
        "Always download remote images",
        "Load HTTP(S) images in HTML messages without asking each time. \
         Messages still render as text only; no scripts, forms, or other \
         remote resources are ever loaded — only images are affected.",
        s.settings.always_download_remote_images,
        p,
        |st| st.always_download_remote_images = !st.always_download_remote_images,
    );
    let title_bar_row = setting_toggle(
        "setting-title-bar",
        "Show the window title bar",
        "The system title bar and borders, to move, minimise and close the window. \
         Turn off under a tiling window manager such as Hyprland, which draws its \
         own. Takes effect the next time Cosmic Mail starts.",
        s.settings.window_title_bar,
        p,
        |st| st.window_title_bar = !st.window_title_bar,
    );

    let mut accounts = col().key("settings-accounts").w_full().gap(6.0);
    for acc in &s.accounts {
        let id = acc.id.clone();
        let expired = s.reauth_required.contains(&acc.id);
        let kind = if s.is_gmail(&acc.id) { "gmail" } else { "imap" };
        let mut info = row().gap(0.0).child(label(kind, SMALL, p.muted));
        if expired {
            info = info.child(label(" · sign-in expired", SMALL, p.error));
        }
        let mut r = row()
            .key(format!("settings-account-{}", acc.id))
            .w_full()
            .px(10.0)
            .py(8.0)
            .gap(10.0)
            .center_y()
            .radius(RADIUS)
            .border(1.0, p.border)
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(2.0)
                    .child(label(acc.email.clone(), BODY, p.fg).truncate())
                    .child(info),
            );
        let armed = st.pending_removal.as_ref().map(|(i, _)| i) == Some(&acc.id);
        if armed {
            let confirm_id = id.clone();
            r = r
                .child(
                    button(
                        format!("confirm-remove-{id}"),
                        "Confirm remove",
                        p,
                        false,
                        true,
                        move |s, _| s.request_remove_account(confirm_id.clone()),
                    )
                    .border(1.0, p.error),
                )
                .child(button(
                    format!("cancel-remove-{id}"),
                    "Cancel",
                    p,
                    false,
                    true,
                    |s, _| {
                        if let Some(Modal::Settings(st)) = &mut s.modal {
                            st.pending_removal = None;
                        }
                    },
                ));
        } else {
            if s.is_gmail(&acc.id) {
                let reauth_id = id.clone();
                let busy = st.reauthing.is_some();
                let caption = if st.reauthing.as_deref() == Some(acc.id.as_str()) {
                    "Waiting for Google…"
                } else {
                    "Reconnect"
                };
                r = r.child(button(
                    format!("reconnect-{id}"),
                    caption,
                    p,
                    expired,
                    !busy,
                    move |s, _| s.reauth_account(reauth_id.clone()),
                ));
            }
            let remove_id = id.clone();
            r = r.child(button(
                format!("remove-{id}"),
                "Remove",
                p,
                false,
                true,
                move |s, _| s.request_remove_account(remove_id.clone()),
            ));
        }
        accounts = accounts.child(r);
    }
    if s.accounts.is_empty() {
        accounts = accounts.child(label("No accounts configured.", font(12.0, 400.0), p.muted));
    }

    let section = |title: &str, content: El| {
        col()
            .w_full()
            .gap(8.0)
            .child(label(title.to_uppercase(), font(10.5, 700.0), p.muted))
            .child(content)
    };
    col()
        .key("settings")
        .role("dialog")
        .label("Settings")
        .w(680.0)
        .child(dialog_head("Settings", p, true))
        .child(
            col()
                .w_full()
                .px(16.0)
                .py(14.0)
                .gap(18.0)
                .child(section("Privacy", remote_row))
                .child(section("Window", title_bar_row))
                .child(section("Appearance", appearance(s, p)))
                .child(section("Accounts", accounts))
                .child(section(
                    "About",
                    label(
                        format!("Cosmic Mail {}", crate::VERSION),
                        font(12.0, 400.0),
                        p.muted,
                    )
                    .key("about-version"),
                )),
        )
}

fn command_palette(st: &PaletteState, p: &Palette) -> El {
    let results = st.results();
    let selected = st.selected.min(results.len().saturating_sub(1));
    let input = text_input(&st.query, BODY)
        .key("palette-input")
        .label("Command palette")
        .role("combobox")
        .color(p.fg)
        .placeholder(st.placeholder, p.muted_2)
        .single_line()
        .grow(1.0)
        .min_w(0.0)
        .on_edit(|s: &mut Ui| match &mut s.modal {
            Some(Modal::Palette(st)) => &mut st.query,
            _ => &mut s.scratch_edit,
        })
        .on_input_change(|s: &mut Ui, _tx: &mut Tx| {
            if let Some(Modal::Palette(st)) = &mut s.modal {
                st.selected = 0;
            }
        })
        .on_input_key(|s: &mut Ui, ev: &KeyEvent, tx: &mut Tx| palette_key(s, ev, tx));

    let mut list = col()
        .key("palette-list")
        .role("listbox")
        .label("Commands")
        .w_full()
        .max_h(360.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    if results.is_empty() {
        list = list.child(row().px(14.0).py(10.0).child(label(
            "No matching commands",
            font(12.0, 400.0),
            p.muted,
        )));
    }
    for (pos, &i) in results.iter().enumerate() {
        let cmd = &st.commands[i];
        let on = pos == selected;
        let action = cmd.action.clone();
        let mut item = row()
            .key(format!("pal-{}", cmd.id))
            .role("option")
            .label(cmd.title.clone())
            .checked(on)
            .w_full()
            .px(14.0)
            .h(30.0)
            .gap(10.0)
            .center_y()
            .fill(if on {
                p.accent_soft
            } else {
                p.accent_soft.alpha(0.0)
            })
            .border_sides(
                [0.0, 0.0, 0.0, 2.0],
                if on { p.accent } else { p.accent.alpha(0.0) },
            )
            .cursor(Cursor::Pointer)
            .on_hover(move |s: &mut Ui| {
                if let Some(Modal::Palette(st)) = &mut s.modal {
                    st.selected = pos;
                }
            })
            .on_press_tx(move |s: &mut Ui, tx: &mut Tx| {
                s.modal = None;
                s.run_action(action.clone(), tx);
            })
            .child(
                label(cmd.title.clone(), BODY, p.fg)
                    .truncate()
                    .grow(1.0)
                    .min_w(0.0),
            );
        if let Some(h) = cmd.hint {
            item = item.child(kbd(h, p));
        }
        list = list.child(item);
    }

    col()
        .key("palette")
        .role("dialog")
        .label("Command palette")
        .w(560.0)
        .child(
            row()
                .key("palette-head")
                .w_full()
                .px(14.0)
                .h(42.0)
                .gap(10.0)
                .center_y()
                .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
                .child(label("›", font(15.0, 700.0), p.accent))
                .child(input)
                .child(label(
                    format!("{} / {}", results.len(), st.commands.len()),
                    SMALL,
                    p.muted,
                )),
        )
        .child(list)
        .child(
            row()
                .w_full()
                .px(14.0)
                .py(6.0)
                .gap(14.0)
                .fill(mix(p.fg, 0.03, p.bg))
                .border_sides([1.0, 0.0, 0.0, 0.0], p.border)
                .child(label("↑↓ navigate", SMALL, p.muted))
                .child(label("enter run", SMALL, p.muted))
                .child(label("esc close", SMALL, p.muted)),
        )
}

/// Palette keys: arrows / Ctrl-N/P move, Enter runs, Escape closes.
fn palette_key(s: &mut Ui, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(Modal::Palette(st)) = &mut s.modal else {
        return false;
    };
    let n = st.results().len();
    let step = |st: &mut PaletteState, d: isize| {
        if n > 0 {
            st.selected = ((st.selected.min(n - 1) as isize + d).rem_euclid(n as isize)) as usize;
        }
    };
    match &ev.key {
        Key::Down => step(st, 1),
        Key::Up => step(st, -1),
        Key::Command('n') if ev.mods.ctrl => step(st, 1),
        Key::Command('p') if ev.mods.ctrl => step(st, -1),
        Key::Enter => {
            let results = st.results();
            let action = results
                .get(st.selected.min(n.saturating_sub(1)))
                .map(|&i| st.commands[i].action.clone());
            // Close first so focus a command requests (search, a modal) wins.
            s.modal = None;
            if let Some(action) = action {
                s.run_action(action, tx);
            }
        }
        Key::Escape => s.modal = None,
        _ => return false,
    }
    if let Some(Modal::Palette(st)) = &s.modal {
        if let Some(&i) = st.results().get(st.selected) {
            tx.scroll_into_view(format!("pal-{}", st.commands[i].id));
        }
    }
    true
}
