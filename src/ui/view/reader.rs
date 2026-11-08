//! The reader: header, actions, attachment and shipment chips, and the body.

use rune::text::Span;
use rune::{col, el, rich, row, text, Cursor, El, Link, ScrollSpec, Tx};

use super::{button, font, label, BODY, RADIUS, SMALL};
use crate::ui::format::{
    avatar_slot, carrier_glyph, carrier_label, format_bytes, full_date, initials,
};
use crate::ui::html::{Line, Style};
use crate::ui::theme::Palette;
use crate::ui::{BodyImage, Ui};

/// The reader pane (its width is set by the caller).
pub fn reader(s: &Ui, p: &Palette) -> El {
    let pane = col().key("reader").role("region").label("Message");
    let Some(msg) = s.selected_message().cloned() else {
        return pane.child(
            row()
                .key("reader-empty")
                .w_full()
                .grow(1.0)
                .center()
                .child(label("No message selected", font(12.0, 400.0), p.muted)),
        );
    };
    let id = msg.id;
    let body = s.body.as_ref().filter(|b| b.id == id);
    let loaded = body.is_some();

    let who = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(2.0)
        .child(
            label(
                if msg.from_name.is_empty() {
                    msg.from_addr.clone()
                } else {
                    msg.from_name.clone()
                },
                font(13.0, 700.0),
                p.fg,
            )
            .truncate(),
        )
        .child({
            let to = body
                .and_then(|b| b.to_addrs.first().cloned())
                .or_else(|| {
                    s.accounts
                        .iter()
                        .find(|a| a.id == msg.account_id)
                        .map(|a| a.email.clone())
                })
                .unwrap_or_default();
            let line = if to.is_empty() {
                msg.from_addr.clone()
            } else {
                format!("{} → {to}", msg.from_addr)
            };
            label(line, font(11.5, 400.0), p.muted).truncate()
        });
    let seed = if msg.from_addr.is_empty() {
        &msg.from_name
    } else {
        &msg.from_addr
    };
    let sender = row()
        .w_full()
        .gap(12.0)
        .center_y()
        .child(
            row()
                .size(38.0, 38.0)
                .center()
                .radius(5.0)
                .fill(p.slot(avatar_slot(seed)))
                .child(label(
                    initials(&msg.from_name, &msg.from_addr),
                    font(14.0, 700.0),
                    p.bg,
                )),
        )
        .child(who)
        .child(label(full_date(&msg.date), font(11.5, 400.0), p.muted).key("reader-date"));

    let actions = row()
        .key("reader-actions")
        .w_full()
        .gap(6.0)
        .flex_wrap()
        .child(button("act-reply", "↩ Reply", p, true, loaded, |s, _| {
            s.open_reply(false)
        }))
        .child(button(
            "act-reply-all",
            "↩↩ Reply all",
            p,
            false,
            loaded,
            |s, _| s.open_reply(true),
        ))
        .child(button("act-forward", "→ Forward", p, false, false, |_, _| {}).help("Coming soon"))
        .child(button(
            "act-archive",
            "📦 Archive",
            p,
            false,
            true,
            move |s, _| s.archive_message(id),
        ))
        .child(button(
            "act-delete",
            "🗑 Delete",
            p,
            false,
            true,
            move |s, _| s.delete_message(id),
        ))
        .child(button(
            "act-flag",
            if msg.flagged {
                "★ Unflag"
            } else {
                "★ Flag"
            },
            p,
            msg.flagged,
            true,
            move |s, _| s.toggle_flagged(id),
        ))
        .child(button("act-move", "↦ Move", p, false, true, |s, _| {
            s.open_move_picker()
        }))
        .child(button(
            "act-read",
            if msg.seen {
                "◎ Mark unread"
            } else {
                "● Mark read"
            },
            p,
            false,
            true,
            move |s, _| s.toggle_read(id),
        ));

    let mut head = col()
        .key("reader-head")
        .w_full()
        .px(26.0)
        .pt(20.0)
        .pb(14.0)
        .gap(12.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .child(
            text(if msg.subject.is_empty() {
                "(no subject)".to_string()
            } else {
                msg.subject.clone()
            })
            .key("reader-subject")
            .font(font(17.0, 700.0).lh(17.0 * 1.4))
            .color(p.fg)
            .wrap()
            .selectable(true),
        )
        .child(sender)
        .child(actions);

    // Listed (non-inline) attachments: inline cid images already render.
    if let Some(b) = body {
        let listed: Vec<_> = b.attachments.iter().filter(|a| !a.is_inline).collect();
        if !listed.is_empty() {
            let mut atts = row()
                .key("attachments")
                .role("list")
                .label("Attachments")
                .gap(6.0)
                .flex_wrap();
            for att in listed {
                let a = att.clone();
                let name = if att.filename.is_empty() {
                    "attachment".to_string()
                } else {
                    att.filename.clone()
                };
                let size = format_bytes(att.size_bytes as f64);
                atts = atts.child(
                    row()
                        .key(format!("att-{}", att.id))
                        .role("button")
                        .label(format!("Save {name} ({size})"))
                        .help(format!("Save {name} ({size})"))
                        .px(11.0)
                        .h(28.0)
                        .gap(7.0)
                        .center_y()
                        .max_w(360.0)
                        .radius(RADIUS)
                        .fill(p.panel)
                        .border(1.0, p.border)
                        .hover(p.fg.alpha(0.06))
                        .focusable(true)
                        .cursor(Cursor::Pointer)
                        .on_press(move |s: &mut Ui| s.save_attachment(&a))
                        .child(label("📎", font(12.0, 400.0), p.muted))
                        .child(
                            label(name, font(12.0, 400.0), p.fg)
                                .truncate()
                                .shrink()
                                .min_w(0.0),
                        )
                        .child(label(size, SMALL, p.muted)),
                );
            }
            head = head.child(atts);
        }
    }

    if !s.shipments.is_empty() {
        let mut ships = row()
            .key("shipments")
            .role("list")
            .label("Shipments")
            .gap(6.0)
            .flex_wrap();
        for sh in &s.shipments {
            let carrier = carrier_label(&sh.carrier).to_string();
            let mut info =
                col()
                    .gap(1.0)
                    .min_w(0.0)
                    .child(label(carrier.clone(), font(12.0, 700.0), p.fg));
            if let Some(n) = &sh.tracking_number {
                info = info.child(label(n.clone(), SMALL, p.muted).truncate());
            }
            if let Some(o) = &sh.order_id {
                info = info.child(label(format!("Order {o}"), SMALL, p.muted).truncate());
            }
            let mut chip = row()
                .key(format!("ship-{}", sh.id))
                .role("button")
                .label(format!("Track on {carrier}"))
                .px(12.0)
                .py(6.0)
                .gap(9.0)
                .center_y()
                .max_w(360.0)
                .radius(RADIUS)
                .fill(p.panel)
                .border(1.0, p.border)
                .child(label(carrier_glyph(&sh.carrier), font(14.0, 400.0), p.fg))
                .child(info);
            if let Some(url) = sh.tracking_url.clone() {
                chip = chip
                    .help(format!("Track on {carrier}"))
                    .hover(p.fg.alpha(0.06))
                    .focusable(true)
                    .cursor(Cursor::Pointer)
                    .on_press_tx(move |s: &mut Ui, _tx: &mut Tx| s.open_link(&url))
                    .child(label("↗", font(12.0, 400.0), p.accent));
            } else {
                chip = chip.disabled(true).opacity(0.6);
            }
            ships = ships.child(chip);
        }
        head = head.child(ships);
    }

    let mut content = col()
        .key("reader-content")
        .w_full()
        .px(26.0)
        .py(16.0)
        .gap(4.0);
    if s.loading_body {
        content = content.child(label("Loading…", font(12.0, 400.0), p.muted));
    } else if let Some(b) = body {
        if b.html.is_some() && !s.body_lines.is_empty() {
            if s.body_has_remote_images() && !s.settings.always_download_remote_images {
                content = content.child(remote_banner(s, id, p));
            }
            for (i, line) in s.body_lines.iter().enumerate() {
                content = content.child(html_line(s, i, line, p));
            }
        } else if let Some(t) = b.text.as_ref().filter(|t| !t.trim().is_empty()) {
            content = content.child(
                text(t.clone())
                    .key("body-text")
                    .font(BODY.lh(13.0 * 1.5))
                    .color(p.fg)
                    .wrap()
                    .wrap_anywhere()
                    .selectable(true),
            );
        } else {
            content = content.child(label("(empty message)", font(12.0, 400.0), p.muted));
        }
    }

    pane.child(head).child(
        col()
            .key("reader-body")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(content),
    )
}

/// The remote-images consent bar.
fn remote_banner(s: &Ui, id: i64, p: &Palette) -> El {
    let allowed = s.remote_allowed.contains(&id);
    row()
        .key("remote-content")
        .role("status")
        .w_full()
        .px(10.0)
        .py(6.0)
        .mb(10.0)
        .gap(10.0)
        .center_y()
        .radius(RADIUS)
        .fill(p.panel)
        .border(1.0, p.border)
        .child(
            label(
                if allowed {
                    "Remote images are allowed for this message."
                } else {
                    "Remote images are blocked to protect your privacy."
                },
                font(12.0, 400.0),
                p.muted,
            )
            .grow(1.0)
            .min_w(0.0)
            .wrap(),
        )
        .child(button(
            "remote-toggle",
            if allowed {
                "Block remote images"
            } else {
                "Load remote images"
            },
            p,
            false,
            true,
            move |s, _| s.toggle_remote_images(id),
        ))
}

/// One rendered HTML line: styled text with links, plus its images.
fn html_line(s: &Ui, index: usize, line: &Line, p: &Palette) -> El {
    if line.is_blank() {
        return el().key(format!("line-{index}")).h(8.0);
    }
    let mut block = col().key(format!("line-{index}")).w_full().gap(4.0);
    for img in &line.images {
        if let Some(e) = image_el(s, &img.src, &img.alt, p) {
            block = block.child(e);
        }
    }
    let shown: &str = line.text.as_str();
    if shown.trim().is_empty() {
        return block;
    }
    let mut spans = Vec::new();
    let mut links = Vec::new();
    for (range, style) in &line.styles {
        let span = Span::new(range.start, range.end);
        let span = match style {
            Style::Strong => span.bold(),
            Style::Emphasis => span.italic(),
            Style::Code => span.mono().bg(p.panel_2, 2.0),
            Style::Strike => span.color(p.muted),
            Style::Link(url) => {
                links.push(Link::new(range.clone(), url.clone()));
                span.color(p.accent).underline(p.accent.alpha(0.6))
            }
        };
        spans.push(span);
    }
    let mut t = rich(shown.to_string(), spans)
        .font(if line.pre {
            BODY.lh(13.0 * 1.4)
        } else {
            font(14.0, 400.0).lh(14.0 * 1.5)
        })
        .color(p.fg)
        .wrap()
        .wrap_anywhere()
        .selectable(true);
    for link in links {
        t = t.link(link);
    }
    t = t.on_link(|s: &mut Ui, url: &str, _tx: &mut Tx| s.open_link(url));
    block.child(t)
}

/// An inline image: decoded `data:` images and allowed remote ones show; a
/// blocked remote image shows its alt text as a placeholder.
fn image_el(s: &Ui, src: &str, alt: &str, p: &Palette) -> Option<El> {
    match s.body_images.get(src) {
        Some(BodyImage::Ready(img)) => {
            let [w, h] = img.size()?;
            // Tracking pixels and spacers draw nothing worth showing.
            if w <= 2 || h <= 2 {
                return None;
            }
            Some(
                el().key(format!("img-{}", crate::ui::short_hash(src)))
                    .pixels(img.pixels())
                    .w((w as f32).min(560.0))
                    .label(alt.to_string()),
            )
        }
        Some(BodyImage::Loading) => None,
        Some(BodyImage::Failed) | None => {
            if alt.is_empty() {
                return None;
            }
            Some(
                row()
                    .px(6.0)
                    .h(20.0)
                    .center_y()
                    .radius(3.0)
                    .border(1.0, p.border)
                    .child(label(format!("🖼 {alt}"), SMALL, p.muted).truncate())
                    .max_w(360.0),
            )
        }
    }
}
