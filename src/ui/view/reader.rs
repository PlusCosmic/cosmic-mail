//! The reader: its action bar, then (scrolling) the headers, shipment and attachment cards,
//! the remote-images notice and the body.

use rune::text::Span;
use rune::{col, el, rich, row, text, Cursor, El, Link, ScrollSpec, Tx};
use rune_theme_foundation::parts::{Notice, Tone};

use super::chrome::account_tag;
use super::{
    btn, display, eyebrow, font, glyph, icon_button, icons, label, Kind, BODY, RADIUS, RADIUS_LG,
    SMALL,
};
use crate::ui::format::{carrier_label, folder_title, format_bytes, full_date, initials};
use crate::ui::html::{Line, Style};
use crate::ui::mail_doc::{Item as MailItem, MailLayout};
use crate::ui::theme::Palette;
use crate::ui::{BodyImage, Ui};

/// The widest a line of body text runs, for reading.
const MEASURE: f32 = 680.0;

/// The widest the reader column runs (headers and HTML mail). Mail is laid out at the
/// column's full width, so fixed-width newsletters centre themselves in it as they do in
/// other clients, and fluid ones spread out.
const COLUMN: f32 = 960.0;

/// The reader pane (its width is set by the caller).
/// `width` is the pane's (the mail body lays out to fit it).
pub fn reader(s: &Ui, p: &Palette, width: f32) -> El {
    let pane = col()
        .key("reader")
        .role("region")
        .label("Message")
        .fill(rune_theme_foundation::mix(p.panel, 0.45, p.bg));
    let Some(msg) = s.selected_message().cloned() else {
        return pane.child(
            col()
                .key("reader-empty")
                .w_full()
                .grow(1.0)
                .center()
                .gap(10.0)
                .child(glyph(&icons::MAIL, 28.0, p.faint))
                .child(label("No message selected", font(13.0, 400.0), p.muted)),
        );
    };
    let id = msg.id;
    let body = s.body.as_ref().filter(|b| b.id == id);
    let loaded = body.is_some();

    let actions = row()
        .key("reader-actions")
        .w_full()
        .px(24.0)
        .py(9.0)
        .gap(6.0)
        .center_y()
        .flex_wrap()
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .child(btn(
            "act-reply",
            "Reply",
            Some(&icons::REPLY),
            Some("r"),
            Kind::Secondary,
            loaded,
            p,
            |s, _| s.open_reply(false),
        ))
        .child(btn(
            "act-reply-all",
            "Reply all",
            Some(&icons::REPLY_ALL),
            None,
            Kind::Secondary,
            loaded,
            p,
            |s, _| s.open_reply(true),
        ))
        .child(
            btn(
                "act-forward",
                "Forward",
                Some(&icons::FORWARD),
                None,
                Kind::Ghost,
                false,
                p,
                |_, _| {},
            )
            .help("Coming soon"),
        )
        .child(el().grow(1.0))
        .child(icon_button(
            "act-read",
            if msg.seen {
                "Mark unread (u)"
            } else {
                "Mark read (u)"
            },
            if msg.seen {
                &icons::MAIL
            } else {
                &icons::MAIL_OPEN
            },
            p.muted,
            p,
            move |s, _| s.toggle_read(id),
        ))
        .child(
            icon_button(
                "act-flag",
                if msg.flagged {
                    "Unflag (f)"
                } else {
                    "Flag (f)"
                },
                if msg.flagged {
                    &icons::STAR
                } else {
                    &icons::FLAG
                },
                if msg.flagged { p.warn } else { p.muted },
                p,
                move |s, _| s.toggle_flagged(id),
            )
            .checked(msg.flagged),
        )
        .child(icon_button(
            "act-move",
            "Move (m)",
            &icons::MOVE,
            p.muted,
            p,
            |s, _| s.open_move_picker(),
        ))
        .child(icon_button(
            "act-archive",
            "Archive (a)",
            &icons::ARCHIVE,
            p.muted,
            p,
            move |s, _| s.archive_message(id),
        ))
        .child(icon_button(
            "act-delete",
            "Delete (d)",
            &icons::TRASH,
            p.muted,
            p,
            move |s, _| s.delete_message(id),
        ));

    // Where the message lives, over its subject.
    let folder = s
        .folders
        .get(&msg.account_id)
        .and_then(|fs| fs.iter().find(|f| f.id == msg.folder_id))
        .map(|f| folder_title(&f.name, &f.role).to_string());
    let place = match folder {
        Some(f) => format!("{} · {f}", account_tag(s, &msg.account_id)),
        None => account_tag(s, &msg.account_id),
    };

    let to = body
        .and_then(|b| b.to_addrs.first().cloned())
        .or_else(|| {
            s.accounts
                .iter()
                .find(|a| a.id == msg.account_id)
                .map(|a| a.email.clone())
        })
        .unwrap_or_default();
    let name = if msg.from_name.is_empty() {
        msg.from_addr.clone()
    } else {
        msg.from_name.clone()
    };
    let from_line = if msg.from_name.is_empty() || msg.from_addr.is_empty() {
        rich(name.clone(), vec![Span::new(0, name.len()).bold()])
    } else {
        let line = format!("{name}  <{}>", msg.from_addr);
        rich(
            line.clone(),
            vec![
                Span::new(0, name.len()).bold(),
                Span::new(name.len(), line.len()).color(p.muted),
            ],
        )
    };
    let mut who = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(2.0)
        .child(from_line.font(font(13.5, 400.0)).color(p.fg).truncate());
    if !to.is_empty() {
        who = who.child(label(format!("To {to}"), font(12.5, 400.0), p.muted).truncate());
    }
    let sender = row()
        .w_full()
        .gap(12.0)
        .center_y()
        .child(
            row()
                .size(40.0, 40.0)
                .center()
                .radius(20.0)
                .fill(p.fill)
                .child(label(
                    initials(&msg.from_name, &msg.from_addr),
                    font(14.0, 700.0),
                    p.fill_ink,
                )),
        )
        .child(who)
        .child(label(full_date(&msg.date), font(12.5, 400.0), p.muted).key("reader-date"));

    let head = col()
        .key("reader-head")
        .w_full()
        .gap(10.0)
        .child(eyebrow(place, p))
        .child(
            text(if msg.subject.is_empty() {
                "(no subject)".to_string()
            } else {
                msg.subject.clone()
            })
            .key("reader-subject")
            .font(display(30.0).lh(35.0))
            .color(p.fg)
            .wrap()
            .selectable(true),
        )
        .child(sender.mt(4.0));

    // Shipments and listed (non-inline) attachments, as cards; inline cid images already render.
    let mut cards = row().key("reader-cards").w_full().gap(10.0).flex_wrap();
    let mut any_card = false;
    if !s.shipments.is_empty() {
        let mut ships = row()
            .key("shipments")
            .role("list")
            .label("Shipments")
            .gap(10.0)
            .flex_wrap();
        for sh in &s.shipments {
            ships = ships.child(shipment_card(sh, p));
        }
        cards = cards.child(ships);
        any_card = true;
    }
    if let Some(b) = body {
        let listed: Vec<_> = b.attachments.iter().filter(|a| !a.is_inline).collect();
        if !listed.is_empty() {
            let mut atts = row()
                .key("attachments")
                .role("list")
                .label("Attachments")
                .gap(10.0)
                .flex_wrap();
            for att in listed {
                let a = att.clone();
                let name = if att.filename.is_empty() {
                    "attachment".to_string()
                } else {
                    att.filename.clone()
                };
                let size = format_bytes(att.size_bytes as f64);
                let kind = att
                    .mime_type
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_uppercase();
                atts = atts.child(
                    row()
                        .key(format!("att-{}", att.id))
                        .role("button")
                        .label(format!("Save {name} ({size})"))
                        .help(format!("Save {name} ({size})"))
                        .h(54.0)
                        .px(14.0)
                        .gap(10.0)
                        .center_y()
                        .max_w(320.0)
                        .radius(RADIUS_LG)
                        .fill(p.panel)
                        .border(1.0, p.border)
                        .hover(p.fg.alpha(0.04))
                        .focusable(true)
                        .cursor(Cursor::Pointer)
                        .on_press(move |s: &mut Ui| s.save_attachment(&a))
                        .child(glyph(&icons::CLIP, 16.0, p.muted))
                        .child(
                            col()
                                .gap(1.0)
                                .shrink()
                                .min_w(0.0)
                                .child(label(name, font(13.0, 600.0), p.fg).truncate())
                                .child(label(
                                    if kind.is_empty() {
                                        size
                                    } else {
                                        format!("{size} · {kind}")
                                    },
                                    SMALL,
                                    p.muted,
                                )),
                        ),
                );
            }
            cards = cards.child(atts);
            any_card = true;
        }
    }

    let mut content = col()
        .key("reader-content")
        .w_full()
        .max_w(COLUMN + 80.0)
        .self_center()
        .px(40.0)
        .pt(26.0)
        .pb(40.0)
        .gap(18.0)
        .child(head);
    if any_card {
        content = content.child(cards);
    }

    let mut text_body = col().key("reader-text").w_full().max_w(MEASURE).gap(4.0);
    if s.loading_body {
        text_body = text_body.child(label("Loading…", font(13.0, 400.0), p.muted));
    } else if let Some(b) = body {
        // HTML is laid out by Blitz at the content column's width (40px padding each side).
        // `None` means the first layout is still being computed: draw nothing rather than
        // flash another rendering. An empty layout (Blitz produced nothing) falls back to
        // the styled-text lines.
        let mail_width = (width - 80.0).min(COLUMN);
        let doc = b.html.as_ref().map(|_| s.mail_layout(mail_width));
        let remote_blocked =
            s.body_has_remote_images() && !s.settings.always_download_remote_images;
        if let Some(Some(doc)) = doc
            .as_ref()
            .filter(|d| d.as_ref().is_some_and(|d| !d.items.is_empty()))
        {
            if remote_blocked {
                content = content.child(remote_notice(s, id, p));
            }
            text_body = text_body.max_w(COLUMN).child(mail_doc_view(s, doc));
        } else if matches!(doc, Some(None)) {
            // Laying out.
        } else if b.html.is_some() && !s.body_lines.is_empty() {
            if s.body_has_remote_images() && !s.settings.always_download_remote_images {
                content = content.child(remote_notice(s, id, p));
            }
            for (i, line) in s.body_lines.iter().enumerate() {
                text_body = text_body.child(html_line(s, i, line, p));
            }
        } else if let Some(t) = b.text.as_ref().filter(|t| !t.trim().is_empty()) {
            text_body = text_body.child(
                text(t.clone())
                    .key("body-text")
                    .font(font(14.5, 400.0).lh(23.0))
                    .color(p.fg)
                    .wrap()
                    .wrap_anywhere()
                    .selectable(true),
            );
        } else {
            text_body = text_body.child(label("(empty message)", font(13.0, 400.0), p.muted));
        }
    }
    content = content.child(text_body);

    pane.child(actions).child(super::scroll_bar(
        col()
            .key("reader-body")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(content),
        p,
    ))
}

/// A parcel detected in the mail: carrier, tracking number and order, and a way to track it.
fn shipment_card(sh: &crate::wire::Shipment, p: &Palette) -> El {
    let carrier = carrier_label(&sh.carrier).to_string();
    let mut detail = vec![carrier.clone()];
    if let Some(n) = &sh.tracking_number {
        detail.push(n.clone());
    }
    if let Some(o) = &sh.order_id {
        detail.push(format!("Order {o}"));
    }
    let mut card = row()
        .key(format!("ship-{}", sh.id))
        .h(54.0)
        .pl(10.0)
        .pr(10.0)
        .gap(12.0)
        .center_y()
        .max_w(520.0)
        .radius(RADIUS_LG)
        .fill(p.panel)
        .border(1.0, p.border)
        .child(
            row()
                .size(34.0, 34.0)
                .center()
                .radius(RADIUS)
                .fill(p.accent_soft)
                .child(glyph(&icons::PACKAGE, 18.0, p.accent)),
        )
        .child(
            col()
                .gap(1.0)
                .shrink()
                .min_w(0.0)
                .child(label("Parcel on its way", font(13.0, 650.0), p.fg))
                .child(label(detail.join(" · "), SMALL, p.muted).truncate()),
        );
    if let Some(url) = sh.tracking_url.clone() {
        card = card.child(
            btn(
                format!("track-{}", sh.id),
                format!("Track on {carrier}"),
                Some(&icons::EXTERNAL),
                None,
                Kind::Primary,
                true,
                p,
                move |s: &mut Ui, _tx: &mut Tx| s.open_link(&url),
            )
            .ml(6.0),
        );
    }
    card
}

/// The remote-images consent notice (the foundation's compact info notice).
fn remote_notice(s: &Ui, id: i64, p: &Palette) -> El {
    let allowed = s.remote_allowed.contains(&id);
    let look = rune_theme_foundation::look(s.scheme, p.dark);
    let words = if allowed {
        "Remote images are allowed for this message."
    } else {
        "Remote images are blocked to protect your privacy."
    };
    let toggle = btn(
        "remote-toggle",
        if allowed {
            "Block remote images"
        } else {
            "Load remote images"
        },
        None,
        None,
        Kind::Ghost,
        true,
        p,
        move |s, _| s.toggle_remote_images(id),
    );
    col().key("remote-content").role("status").w_full().child(
        Notice::new(Tone::Info)
            .body(words)
            .action(toggle)
            .build(look),
    )
}

/// One rendered HTML line: styled text with links, plus its images.
fn html_line(s: &Ui, index: usize, line: &Line, p: &Palette) -> El {
    if line.is_blank() {
        return el().key(format!("line-{index}")).h(10.0);
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
    // html2text marks list items with "* " and quotes with "> "; draw them as a bullet and a
    // quote block instead, cutting the marker off the text and its styles.
    let (kind, cut) = if line.pre {
        (Block::Plain, 0)
    } else if shown.starts_with("* ") || shown.starts_with("- ") {
        (Block::Item, 2)
    } else if shown.starts_with('>') {
        let rest = shown.trim_start_matches(['>', ' ']);
        (Block::Quote, shown.len() - rest.len())
    } else {
        (Block::Plain, 0)
    };
    let shown = &shown[cut..];
    let mut spans = Vec::new();
    let mut links = Vec::new();
    for (range, style) in &line.styles {
        if range.end <= cut {
            continue;
        }
        let range = range.start.saturating_sub(cut)..range.end - cut;
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
            BODY.lh(13.0 * 1.45)
        } else {
            font(14.5, 400.0).lh(23.0)
        })
        .color(p.fg)
        .wrap()
        .wrap_anywhere()
        .selectable(true);
    for link in links {
        t = t.link(link);
    }
    t = t.on_link(|s: &mut Ui, url: &str, _tx: &mut Tx| s.open_link(url));
    match kind {
        Block::Plain => block.child(t),
        Block::Item => block.child(
            row()
                .w_full()
                .gap(10.0)
                .pl(6.0)
                .child(el().size(5.0, 5.0).radius(2.5).fill(p.muted).mt(9.0))
                .child(t.grow(1.0).min_w(0.0)),
        ),
        Block::Quote => block.child(
            col()
                .w_full()
                .px(14.0)
                .py(9.0)
                .radius(RADIUS)
                .fill(p.panel_2)
                .child(t.color(p.muted)),
        ),
    }
}

/// An HTML mail laid out by Blitz (`ui::mail_doc`), drawn as a page in the mail's
/// own colours — white unless it sets a background, in either theme, as other
/// clients do — with every box, image and line of text where Blitz put it.
fn mail_doc_view(s: &Ui, doc: &MailLayout) -> El {
    let color = |c: [u8; 4]| {
        rune::color::hex(&format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            c[0], c[1], c[2], c[3]
        ))
        .unwrap_or(rune::Color::BLACK)
    };
    let mut page = el()
        .key("mail-doc")
        .w(doc.width)
        .h(doc.height)
        .radius(RADIUS)
        .fill(color(doc.background.unwrap_or([255, 255, 255, 255])));
    for (i, item) in doc.items.iter().enumerate() {
        let child = match item {
            MailItem::Box {
                x,
                y,
                w,
                h,
                fill,
                border,
                radius,
            } => {
                let mut b = el().w(*w).h(*h).radius(*radius);
                if let Some(f) = fill {
                    b = b.fill(color(*f));
                }
                if let Some((bw, bc)) = border {
                    b = b.border_sides(*bw, color(*bc));
                }
                b.absolute(*x, *y)
            }
            MailItem::Image {
                x,
                y,
                w,
                h,
                src,
                alt,
                link,
            } => {
                // Tracking pixels and spacers draw nothing worth showing.
                if *w <= 2.0 || *h <= 2.0 {
                    continue;
                }
                let mut e = match s.body_images.get(src) {
                    Some(BodyImage::Ready(img)) if img.size().is_some() => {
                        el().pixels(img.pixels()).label(alt.clone())
                    }
                    // Blocked, loading or failed: a faint frame keeps the layout readable.
                    _ => el().border(1.0, color([0, 0, 0, 24])),
                };
                if let Some(url) = link.clone() {
                    e = e
                        .cursor(Cursor::Pointer)
                        .on_press(move |s: &mut Ui| s.open_link(&url));
                }
                e.w(*w).h(*h).absolute(*x, *y)
            }
            MailItem::Text {
                x,
                y,
                w,
                line_height,
                size,
                text,
                spans,
            } => {
                let mut rune_spans = Vec::with_capacity(spans.len());
                let mut links = Vec::new();
                for sp in spans {
                    let fg = color(sp.color);
                    // The foundation ships 400 and 600 faces; Blitz matched the same ones.
                    let mut span = Span::new(sp.range.start, sp.range.end)
                        .color(fg)
                        .weight(if sp.weight >= 600.0 { 600.0 } else { 400.0 });
                    if sp.italic {
                        span = span.italic();
                    }
                    if sp.underline {
                        span = span.underline(fg);
                    }
                    rune_spans.push(span);
                    if let Some(url) = &sp.link {
                        links.push(Link::new(sp.range.clone(), url.clone()));
                    }
                }
                let mut t = rich(text.clone(), rune_spans)
                    .font(font(*size, 400.0).lh(*line_height))
                    .selectable(true);
                if !links.is_empty() {
                    for link in links {
                        t = t.link(link);
                    }
                    t = t.on_link(|s: &mut Ui, url: &str, _tx: &mut Tx| s.open_link(url));
                }
                // Blitz broke the line; at its measured width (plus rounding slack) Rune
                // never breaks it again, even where it runs past a narrow page.
                t.w(*w + 1.0).absolute(*x, *y)
            }
        };
        page = page.child(child.key(format!("md-{i}")));
    }
    // Mail with fixed-width tables wider than the pane is drawn scaled down to fit,
    // as other clients do, rather than clipped. Layout keeps the unscaled size, so
    // the wrapper takes the scaled one and the page sits in it out of flow.
    let k = (doc.width / doc.content_width).min(1.0);
    if k >= 1.0 {
        return page;
    }
    el().key("mail-doc-fit")
        .w(doc.content_width * k)
        .h(doc.height * k)
        .child(
            page.w(doc.content_width)
                .origin(0.0, 0.0)
                .scale(k)
                .absolute(0.0, 0.0),
        )
}

/// How an HTML line is laid out.
#[derive(Clone, Copy)]
enum Block {
    Plain,
    Item,
    Quote,
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
                    .w((w as f32).min(MEASURE))
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
                    .px(8.0)
                    .h(24.0)
                    .gap(6.0)
                    .center_y()
                    .radius(RADIUS)
                    .border(1.0, p.border)
                    .child(glyph(&icons::IMAGE, 13.0, p.muted))
                    .child(label(alt.to_string(), SMALL, p.muted).truncate())
                    .max_w(360.0),
            )
        }
    }
}
