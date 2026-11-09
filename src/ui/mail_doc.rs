//! HTML mail laid out by Blitz (Stylo CSS + Taffy + Parley), flattened to a
//! display list the reader draws as Rune elements (`view::reader`).
//!
//! Blitz parses, runs the cascade (the mail's `<style>` and inline styles) and
//! lays out; nothing here paints, runs script or fetches. The walk mirrors
//! `blitz-paint`'s `render_element` in reduced form: element boxes (background,
//! per-side borders, corner radius), images (by `src`, drawn by the reader's own
//! loader, with consent for remote ones), list markers, and text — one item per
//! line segment, so Blitz's line breaks and positions hold exactly.
//!
//! Text is laid out in the app's own fonts (a UA `!important` rule), because
//! Rune can only draw fonts the app embeds: measuring in Arial and drawing in
//! Instrument Sans would make runs overlap. See GOTCHAS ("Blitz").

use blitz_dom::util::ToColorColor;
use blitz_dom::UserAgentStylesheet;
use blitz_dom::{local_name, BaseDocument, DocumentConfig, FontContext, Node, NodeId};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use parley::PositionedLayoutItem;

/// An sRGB colour, 8 bits per channel.
pub type Rgba = [u8; 4];

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// An element's box: background and/or border.
    Box {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<Rgba>,
        /// Border widths (top, right, bottom, left) and one colour: the first visible side's.
        border: Option<([f32; 4], Rgba)>,
        radius: f32,
    },
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        src: String,
        alt: String,
        link: Option<String>,
    },
    /// Text on one line at one font size, its box `line_height` tall from `y`
    /// with the baseline `line_height - descent` down; styled per span.
    Text {
        x: f32,
        y: f32,
        /// The width Blitz measured (so the reader never re-wraps the line).
        w: f32,
        line_height: f32,
        size: f32,
        text: String,
        spans: Vec<TextSpan>,
    },
}

/// The style of a byte range of an [`Item::Text`].
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpan {
    pub range: std::ops::Range<usize>,
    pub weight: f32,
    pub italic: bool,
    pub color: Rgba,
    pub underline: bool,
    /// The link target as written in the mail (validate before opening).
    pub link: Option<String>,
}

impl TextSpan {
    fn same_style(&self, other: &TextSpan) -> bool {
        self.weight == other.weight
            && self.italic == other.italic
            && self.color == other.color
            && self.underline == other.underline
            && self.link == other.link
    }
}

#[derive(Debug, Clone, Default)]
pub struct MailLayout {
    /// The width laid out at.
    pub width: f32,
    /// How wide the content really is: more than `width` when the mail has fixed-width
    /// tables wider than the pane (the reader scales it down to fit).
    pub content_width: f32,
    pub height: f32,
    /// The page background (`<body>`/`<html>`), when the mail sets one.
    pub background: Option<Rgba>,
    pub items: Vec<Item>,
}

/// The fonts mail is laid out in: the app's own faces (the only ones Rune draws),
/// with the system's as fallback for what they lack (`•`, CJK, emoji), which Rune
/// falls back to as well. Built once — the system scan is slow — and shared, so
/// each layout's clone is cheap.
static FONTS: std::sync::LazyLock<FontContext> = std::sync::LazyLock::new(|| {
    use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
    let mut ctx = FontContext {
        collection: Collection::new(CollectionOptions {
            shared: true,
            system_fonts: true,
        }),
        source_cache: SourceCache::new_shared(),
    };
    for face in rune_theme_foundation::FONTS {
        ctx.collection
            .register_fonts(Blob::new(std::sync::Arc::new(*face)), None);
    }
    ctx
});

/// Lay out `html` at `width` CSS px and flatten it to a display list.
///
/// `image_sizes` gives the pixel size of each `<img src>` the reader has already
/// decoded: Blitz loads no resources itself, and an image with no data sizes
/// from its attributes alone. Lay out again as images arrive.
pub fn layout(
    html: &str,
    width: f32,
    image_sizes: &std::collections::HashMap<String, (u32, u32)>,
) -> MailLayout {
    // Stylo and Blitz recurse deeply; Windows' main thread has 1 MB of stack.
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn_scoped(scope, || {
                layout_inner(html, width, |src| image_sizes.get(src).copied())
            })
            .ok()
            .and_then(|h| h.join().ok())
            .unwrap_or_default()
    })
}

fn layout_inner(
    html: &str,
    width: f32,
    image_size: impl Fn(&str) -> Option<(u32, u32)>,
) -> MailLayout {
    let font_ctx = FONTS.clone();
    let css = format!(
        "* {{ font-family: \"{}\" !important; }}",
        rune_theme_foundation::FAMILIES.0.replace('"', "")
    );
    let mut doc = HtmlDocument::from_html(
        html,
        DocumentConfig {
            viewport: Some(Viewport::new(
                width.ceil() as u32,
                1,
                1.0,
                ColorScheme::Light,
            )),
            font_ctx: Some(font_ctx),
            ua_stylesheets: vec![
                UserAgentStylesheet::default_sheet(),
                UserAgentStylesheet::parse(&css),
            ],
            ..Default::default()
        },
    );
    size_images(&mut doc, &image_size);
    doc.resolve(0.0);
    let doc: &BaseDocument = &doc;

    let mut out = MailLayout {
        width,
        ..Default::default()
    };
    let Some(root) = doc.try_root_element() else {
        return out;
    };
    out.height = root.final_layout().size.height;
    out.background = page_background(doc, root);
    walk(doc, root.id, 0.0, 0.0, &mut out.items);
    out.content_width = out
        .items
        .iter()
        .map(|i| match i {
            Item::Box { x, w, .. } | Item::Image { x, w, .. } | Item::Text { x, w, .. } => x + w,
        })
        .fold(width, f32::max);
    out
}

/// Give each `<img>` the reader has decoded its intrinsic size, as a browser
/// would once the image loads. Blitz fetches nothing itself, and an image with
/// no data sizes from its attributes alone — so `width:100%;height:auto` in a
/// shrink-to-fit cell collapses to 0×0. Layout only reads the dimensions, so
/// the pixel data stays empty.
fn size_images(doc: &mut BaseDocument, image_size: &impl Fn(&str) -> Option<(u32, u32)>) {
    use blitz_dom::node::{ImageData, RasterImageData, SpecialElementData};
    let mut imgs = Vec::new();
    let mut stack = vec![doc.root_node().id];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get_node(id) else {
            continue;
        };
        stack.extend(node.children.iter().copied());
        if !node.data.is_element_with_tag_name(&local_name!("img")) {
            continue;
        }
        if let Some((w, h)) = node
            .attr(local_name!("src"))
            .and_then(image_size)
            .filter(|(w, h)| *w > 0 && *h > 0)
        {
            imgs.push((id, w, h));
        }
    }
    for (id, w, h) in imgs {
        if let Some(el) = doc.get_node_mut(id).and_then(|n| n.element_data_mut()) {
            el.special_data = SpecialElementData::Image(Box::new(ImageData::Raster(
                RasterImageData::new(w, h, std::sync::Arc::new(Vec::new())),
            )));
        }
    }
}

fn page_background(doc: &BaseDocument, root: &Node) -> Option<Rgba> {
    let own = root.primary_styles().and_then(|s| fill_of(&s));
    own.or_else(|| {
        root.children
            .iter()
            .filter_map(|id| doc.get_node(*id))
            .find(|n| n.data.is_element_with_tag_name(&local_name!("body")))
            .and_then(|body| body.primary_styles().and_then(|s| fill_of(&s)))
    })
}

fn rgba(c: blitz_dom::util::Color) -> Rgba {
    c.to_rgba8().to_u8_array()
}

fn fill_of(s: &style::properties::ComputedValues) -> Option<Rgba> {
    let current = s.clone_color();
    let c = rgba(
        s.clone_background_color()
            .resolve_to_absolute(&current)
            .as_color_color(),
    );
    (c[3] > 0).then_some(c)
}

fn walk(doc: &BaseDocument, id: NodeId, ox: f32, oy: f32, out: &mut Vec<Item>) {
    let node = &doc.tree()[id];
    if matches!(node.taffy_display(), taffy::Display::None) {
        return;
    }
    let Some(styles) = node.primary_styles() else {
        return;
    };
    if styles.get_inherited_box().visibility != style::computed_values::visibility::T::Visible
        || styles.get_effects().opacity == 0.0
    {
        return;
    }
    let l = node.final_layout();
    let (x, y) = (ox + l.location.x, oy + l.location.y);
    let (w, h) = (l.size.width, l.size.height);

    // Box: background and border.
    let fill = fill_of(&styles);
    let border = {
        // Stylo computes a `none`/`hidden` side's width as 0, so the layout's widths say
        // which sides draw.
        let widths = [l.border.top, l.border.right, l.border.bottom, l.border.left];
        let b = styles.get_border();
        let current = styles.clone_color();
        let colors = [
            &b.border_top_color,
            &b.border_right_color,
            &b.border_bottom_color,
            &b.border_left_color,
        ];
        widths
            .iter()
            .zip(colors)
            .find(|(w, _)| **w > 0.0)
            .map(|(_, c)| rgba(c.resolve_to_absolute(&current).as_color_color()))
            .filter(|c| c[3] > 0)
            .map(|c| (widths, c))
    };
    if (fill.is_some() || border.is_some()) && w > 0.0 && h > 0.0 {
        let radius = styles
            .get_border()
            .border_top_left_radius
            .0
            .width
            .0
            .resolve(style::values::computed::Length::new(w.min(h)))
            .px();
        let item = Item::Box {
            x,
            y,
            w,
            h,
            fill,
            border,
            radius,
        };
        // Mail nests same-coloured tables in cells of the same size; one box is enough.
        if out.last() != Some(&item) {
            out.push(item);
        }
    }

    // Images: the reader loads and draws them by `src`.
    if node.data.is_element_with_tag_name(&local_name!("img")) {
        if let Some(src) = node.attr(local_name!("src")) {
            let pb = l.padding.left + l.border.left;
            let pt = l.padding.top + l.border.top;
            out.push(Item::Image {
                x: x + pb,
                y: y + pt,
                w: l.content_box_width(),
                h: l.content_box_height(),
                src: src.to_string(),
                alt: node.attr(local_name!("alt")).unwrap_or("").to_string(),
                link: ancestor_link(doc, id).map(str::to_string),
            });
        }
    }

    // Text laid out by this inline root.
    if node.flags.is_inline_root() {
        if let Some(text) = node
            .element_data()
            .and_then(|e| e.inline_layout_data.as_ref())
        {
            let cx = x + l.padding.left + l.border.left;
            let cy = y + l.padding.top + l.border.top + text.block_offset;
            text_items(doc, &text.text, &text.layout, cx, cy, out);
        }
    }

    // An outside list marker ("•", "1."), right-aligned before the item's border box and
    // on its first line's baseline, as blitz-paint's `draw_marker` places it.
    if let Some(blitz_dom::node::ListItemLayout {
        marker,
        position: blitz_dom::node::ListItemLayoutPosition::Outside(marker_layout),
    }) = node
        .element_data()
        .and_then(|e| e.list_item_data.as_deref())
    {
        let (source, pad) = match marker {
            blitz_dom::node::Marker::Char(c) => (format!("{c} "), 8.0),
            blitz_dom::node::Marker::String(s) => (s.clone(), 0.0),
        };
        let content_x = x + l.padding.left + l.border.left;
        let content_y = y + l.padding.top + l.border.top;
        let dx = -(marker_layout.full_width() + pad + l.padding.left + l.border.left);
        let dy = node
            .element_data()
            .and_then(|e| e.inline_layout_data.as_ref())
            .and_then(|t| {
                let first = t.layout.lines().next()?.metrics().baseline;
                let own = marker_layout.lines().next()?.metrics().baseline;
                Some(first - own + t.block_offset)
            })
            .unwrap_or(0.0);
        text_items(
            doc,
            &source,
            marker_layout,
            content_x + dx,
            content_y + dy,
            out,
        );
    }

    if let Some(children) = &*node.paint_children.borrow() {
        for child in children {
            walk(doc, *child, x, y, out);
        }
    }
}

fn text_items(
    doc: &BaseDocument,
    source: &str,
    layout: &parley::Layout<blitz_dom::node::TextBrush>,
    cx: f32,
    cy: f32,
    out: &mut Vec<Item>,
) {
    for line in layout.lines() {
        // Glyph runs come in visual order; those of one shaping run share its clusters,
        // which we consume in order to recover each glyph run's text.
        let mut group: Option<Group> = None;
        let mut current_run = usize::MAX;
        let mut clusters: Vec<parley::Cluster<'_, blitz_dom::node::TextBrush>> = Vec::new();
        let mut next = 0;
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(gr) = item else {
                continue;
            };
            let run = gr.run();
            if run.index() != current_run {
                current_run = run.index();
                clusters = run.visual_clusters().collect();
                next = 0;
            }
            let mut text = String::new();
            while let Some(c) = clusters.get(next) {
                if c.style_index() != gr.style_index() {
                    break;
                }
                text.push_str(source.get(c.text_range()).unwrap_or(""));
                next += 1;
            }
            // Drop bidi/format controls (U+200B–U+200F, U+202A–U+202E, U+2060–U+206F, U+FEFF):
            // invisible in a browser, but a run of only these would still be an item.
            let text: String = text
                .trim_end_matches(['\n', '\r'])
                .chars()
                .filter(|c| {
                    !matches!(*c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
                })
                .collect();
            if text.is_empty() {
                continue;
            }
            let id = gr.style().brush.id;
            let Some(s) = doc.get_node(id).and_then(|n| n.primary_styles()) else {
                continue;
            };
            let font = s.get_font();
            let fm = run.font_metrics();
            let piece = Piece {
                x: cx + gr.offset(),
                end_x: cx + gr.offset() + gr.advance(),
                baseline: cy + gr.baseline(),
                ascent: fm.ascent,
                descent: fm.descent.abs(),
                size: run.font_size(),
                text,
                span: TextSpan {
                    range: 0..0,
                    weight: font.font_weight.value(),
                    italic: font.font_style != style::values::computed::font::FontStyle::NORMAL,
                    color: rgba(s.clone_color().as_color_color()),
                    underline: underlined(doc, id),
                    link: ancestor_link(doc, id).map(str::to_string),
                },
            };
            match group.as_mut() {
                Some(g) if g.continues(&piece) => g.push(piece),
                _ => {
                    if let Some(g) = group.take() {
                        g.flush(out);
                    }
                    // A line never starts with a whitespace-only run.
                    if !piece.text.trim().is_empty() {
                        group = Some(Group::new(piece));
                    }
                }
            }
        }
        if let Some(g) = group.take() {
            g.flush(out);
        }
    }
}

/// One glyph run's text and style, before merging.
struct Piece {
    x: f32,
    end_x: f32,
    baseline: f32,
    ascent: f32,
    descent: f32,
    size: f32,
    text: String,
    span: TextSpan,
}

/// Runs on one line that touch, share a baseline and share a font size: drawn as one
/// text element (selectable as a unit) with a span per run. Anything else — padding
/// between inline boxes, a raised `<sup>` — starts a new item, so Blitz's positions hold.
struct Group {
    x: f32,
    end_x: f32,
    baseline: f32,
    ascent: f32,
    descent: f32,
    size: f32,
    text: String,
    spans: Vec<TextSpan>,
}

impl Group {
    fn new(piece: Piece) -> Group {
        let mut g = Group {
            x: piece.x,
            end_x: piece.x,
            baseline: piece.baseline,
            ascent: piece.ascent,
            descent: piece.descent,
            size: piece.size,
            text: String::new(),
            spans: Vec::new(),
        };
        g.push(piece);
        g
    }

    fn continues(&self, p: &Piece) -> bool {
        (p.baseline - self.baseline).abs() < 0.01
            && (p.size - self.size).abs() < 0.01
            && (p.x - self.end_x).abs() < 0.5
    }

    fn push(&mut self, mut p: Piece) {
        let start = self.text.len();
        self.text.push_str(&p.text);
        p.span.range = start..self.text.len();
        self.end_x = p.end_x;
        self.ascent = self.ascent.max(p.ascent);
        self.descent = self.descent.max(p.descent);
        match self.spans.last_mut() {
            Some(last) if last.same_style(&p.span) && last.range.end == start => {
                last.range.end = p.span.range.end;
            }
            _ => self.spans.push(p.span),
        }
    }

    /// Emit the group, its trailing whitespace trimmed. The box is ascent+descent tall
    /// with its top `ascent` above the baseline, so Rune's baseline lands on Blitz's.
    fn flush(mut self, out: &mut Vec<Item>) {
        let trimmed = self.text.trim_end().len();
        self.text.truncate(trimmed);
        for s in &mut self.spans {
            s.range.end = s.range.end.min(trimmed);
            s.range.start = s.range.start.min(s.range.end);
        }
        self.spans.retain(|s| !s.range.is_empty());
        if self.text.is_empty() {
            return;
        }
        out.push(Item::Text {
            x: self.x,
            y: self.baseline - self.ascent,
            w: self.end_x - self.x,
            line_height: self.ascent + self.descent,
            size: self.size,
            text: self.text,
            spans: self.spans,
        });
    }
}

fn ancestor_link(doc: &BaseDocument, mut id: NodeId) -> Option<&str> {
    loop {
        let node = doc.get_node(id)?;
        if node.data.is_element_with_tag_name(&local_name!("a")) {
            if let Some(h) = node.attr(local_name!("href")) {
                return Some(h);
            }
        }
        id = node.parent?;
    }
}

fn underlined(doc: &BaseDocument, mut id: NodeId) -> bool {
    use style::values::specified::TextDecorationLine;
    loop {
        let Some(node) = doc.get_node(id) else {
            return false;
        };
        if let Some(s) = node.primary_styles() {
            if s.get_text()
                .text_decoration_line
                .contains(TextDecorationLine::UNDERLINE)
            {
                return true;
            }
        }
        if node.flags.is_inline_root() {
            return false;
        }
        let Some(p) = node.parent else { return false };
        id = p;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_cells_sit_side_by_side() {
        let l = layout(
            r#"<table width="600"><tr><td>Left</td><td style="background:#232f3e;color:#fff">Right</td></tr></table>"#,
            800.0,
            &Default::default(),
        );
        let pos = |want: &str| {
            l.items.iter().find_map(|i| match i {
                Item::Text { x, y, text, .. } if text == want => Some((*x, *y)),
                _ => None,
            })
        };
        let (lx, ly) = pos("Left").expect("left");
        let (rx, ry) = pos("Right").expect("right");
        assert!(rx > lx + 20.0, "{lx} {rx}");
        assert!((ly - ry).abs() < 1.0);
        assert!(l.items.iter().any(|i| matches!(
            i,
            Item::Box {
                fill: Some([0x23, 0x2f, 0x3e, 255]),
                ..
            }
        )));
    }

    fn lay(html: &str) -> MailLayout {
        layout(html, 600.0, &Default::default())
    }

    fn texts(l: &MailLayout) -> Vec<&str> {
        l.items
            .iter()
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn text_item<'a>(l: &'a MailLayout, want: &str) -> &'a Item {
        l.items
            .iter()
            .find(|i| matches!(i, Item::Text { text, .. } if text == want))
            .unwrap_or_else(|| panic!("no text {want:?} in {:?}", texts(l)))
    }

    #[test]
    fn hidden_scripts_styles_and_preheaders_never_show() {
        // Minified CSS without a trailing `;`, as mail sends it.
        let l = lay(
            r#"<style>.pre{display:none}</style><script>alert(1)</script>
               <div class="pre">Preheader</div><span style="display:none">Hidden</span>
               <p>Shown</p>"#,
        );
        assert_eq!(texts(&l), vec!["Shown"]);
    }

    #[test]
    fn a_line_is_one_item_with_a_span_per_style() {
        let l = lay(r#"<p>Track <b>now</b> or <a href="https://ups.example/t">here</a></p>"#);
        let Item::Text { text, spans, .. } = text_item(&l, "Track now or here") else {
            unreachable!()
        };
        let span_of = |s: &str| {
            spans
                .iter()
                .find(|sp| &text[sp.range.clone()] == s)
                .unwrap_or_else(|| panic!("no span {s:?} in {spans:?}"))
        };
        assert!(span_of("now").weight >= 600.0);
        assert_eq!(
            span_of("here").link.as_deref(),
            Some("https://ups.example/t")
        );
        assert!(span_of("here").underline);
    }

    #[test]
    fn superscripts_keep_their_raised_baseline() {
        let l = lay("<p>&pound;34<sup>99</sup></p>");
        let top = |s| match text_item(&l, s) {
            Item::Text { y, .. } => *y,
            _ => unreachable!(),
        };
        assert!(top("99") < top("£34"));
    }

    #[test]
    fn images_size_from_decoded_pixels() {
        let html = r#"<table><tr><td><img src="cid:p" style="width:100%;height:auto"></td>
                      <td>Charger</td></tr></table>"#;
        let size = |l: &MailLayout| {
            l.items.iter().find_map(|i| match i {
                Item::Image { w, h, src, .. } if src == "cid:p" => Some((*w, *h)),
                _ => None,
            })
        };
        // Without the image a shrink-to-fit cell collapses it, as in a browser before load.
        assert_eq!(size(&lay(html)), Some((0.0, 0.0)));
        let sizes = [("cid:p".to_string(), (90, 45))].into_iter().collect();
        let l = layout(html, 600.0, &sizes);
        assert_eq!(size(&l), Some((90.0, 45.0)));
    }

    #[test]
    fn linked_images_keep_their_link() {
        let l = lay(
            r#"<a href="https://shop.example/p"><img src="cid:p" width="40" height="40" alt="Item"></a>"#,
        );
        assert!(l.items.iter().any(|i| matches!(
            i,
            Item::Image { link: Some(url), alt, .. } if url == "https://shop.example/p" && alt == "Item"
        )));
    }

    #[test]
    fn list_items_get_a_marker_before_them() {
        let l = lay("<ul><li>Keyboard</li></ul>");
        let x_of = |s: &str| match text_item(&l, s) {
            Item::Text { x, .. } => *x,
            _ => unreachable!(),
        };
        assert!(x_of("•") < x_of("Keyboard"));
    }

    #[test]
    fn bidi_controls_alone_make_no_item() {
        let l = lay("<p>Order # \u{202b}026-1</p>");
        assert!(texts(&l).iter().all(|t| !t.contains('\u{202b}')));
        assert!(texts(&l).iter().any(|t| t.contains("026-1")));
    }

    #[test]
    fn page_takes_the_body_background() {
        let l = lay(r#"<body style="background:#f0f2f2"><p>x</p></body>"#);
        assert_eq!(l.background, Some([0xf0, 0xf2, 0xf2, 255]));
        assert!(lay("<p>x</p>").background.is_none());
    }
}
