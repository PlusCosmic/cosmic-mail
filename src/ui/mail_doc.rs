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
use blitz_traits::shell::{ColorScheme, Viewport};
use parley::PositionedLayoutItem;

mod clip;
mod hints;
mod paint;
mod parse;
pub use paint::{background_tile, BgSize, Border, Gradient, LineStyle, Shadow};

/// An sRGB colour, 8 bits per channel.
pub type Rgba = [u8; 4];

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// An element's box: background, border and shadow.
    Box {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<Rgba>,
        border: Option<Border>,
        /// Corner radii: top-left, top-right, bottom-right, bottom-left.
        radius: [f32; 4],
        /// A linear gradient painted over the fill.
        gradient: Option<Gradient>,
        shadows: Vec<Shadow>,
    },
    /// A background image (`background-image: url()`, `background=`) in an element's
    /// padding box `x, y, w, h`, clipped to it. Its size and place in the box depend on
    /// the image's own size: see [`background_tile`].
    Background {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: [f32; 4],
        src: String,
        size: BgSize,
        /// Horizontal and vertical position, each `(fraction, px)`: the image's edge sits
        /// at `px + fraction × (box − image)` (`center` is `(0.5, 0.0)`).
        position: [(f32, f32); 2],
        /// Whether it repeats horizontally and vertically.
        repeat: [bool; 2],
    },
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        src: String,
        alt: String,
        link: Option<String>,
        /// Corner radius (`border-radius:50%` makes a round avatar).
        radius: f32,
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
        /// Letter spacing in px (Blitz measured with it).
        tracking: f32,
        text: String,
        spans: Vec<TextSpan>,
    },
    /// The next `len` items (nested clips included) draw only inside this rectangle:
    /// the padding box of an `overflow:hidden` (`clip`, `scroll`, `auto`) ancestor
    /// they cross the edge of. See `clip`.
    Clip {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        len: usize,
    },
}

/// The style of a byte range of an [`Item::Text`].
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpan {
    pub range: std::ops::Range<usize>,
    pub weight: f32,
    pub italic: bool,
    pub color: Rgba,
    /// Underlined, in this colour (the decorating element's, not always the text's).
    pub underline: Option<Rgba>,
    /// Struck through (`<s>`, `<del>`, `line-through`), in this colour.
    pub strike: Option<Rgba>,
    /// In the foundation's code face (`<code>`, `<pre>`, monospace inline styles).
    pub mono: bool,
    /// The link target as written in the mail (validate before opening).
    pub link: Option<String>,
}

impl TextSpan {
    fn same_style(&self, other: &TextSpan) -> bool {
        self.weight == other.weight
            && self.italic == other.italic
            && self.color == other.color
            && self.underline == other.underline
            && self.strike == other.strike
            && self.mono == other.mono
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
    // Blitz lays out `•`/`◦`/`▪` list markers in `monospace` regardless of the item's font;
    // make that the code face, which has them and which the reader draws markers in.
    if let Some(mono) = ctx.collection.family_id(rune_theme_foundation::FAMILIES.1) {
        ctx.collection.set_generic_families(
            parley::fontique::GenericFamily::Monospace,
            [mono].into_iter(),
        );
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
            .map_err(|e| tracing::warn!("mail layout thread not started: {e}"))
            .ok()
            .and_then(|h| h.join().ok())
            .unwrap_or_else(|| {
                // An empty layout makes the reader show the mail as text. Blitz's panic message
                // went to stderr; it can quote the mail, so it is not logged here.
                tracing::warn!(bytes = html.len(), "mail layout failed; showing it as text");
                MailLayout::default()
            })
    })
}

fn layout_inner(
    html: &str,
    width: f32,
    image_size: impl Fn(&str) -> Option<(u32, u32)>,
) -> MailLayout {
    let font_ctx = FONTS.clone();
    let (sans, mono) = rune_theme_foundation::FAMILIES;
    // Everything in the app's sans, and code — by tag, or by a monospace inline style or
    // `<font face>` — in its code face. Descendants are listed too because `*` matches
    // them directly, so they would not inherit the code face.
    let code = "code, pre, kbd, samp, tt, [style*=monospace i], [style*=courier i], \
                [style*=consolas i], [style*=menlo i], font[face*=mono i], font[face*=courier i]";
    let code_desc = code
        .split(", ")
        .map(|sel| format!("{sel} *"))
        .collect::<Vec<_>>()
        .join(", ");
    let css = format!(
        "* {{ font-family: \"{sans}\" !important; }}\n\
         {code}, {code_desc} {{ font-family: \"{mono}\" !important; }}"
    );
    let mut doc = parse::document(
        html,
        DocumentConfig {
            // Without a base, Blitz panics resolving `//host/x.png` or `x.png`. Nothing is
            // ever fetched from it.
            base_url: Some(BASE_URL.to_string()),
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
    hints::apply(&mut doc);
    table_heights_as_minimums(&mut doc);
    background_attributes(&mut doc);
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
    // The viewport cuts off what lies above or left of the page (`text-indent:-9999px`).
    clip::clip_subtree(
        &mut out.items,
        0,
        [0.0, 0.0, f32::INFINITY, f32::INFINITY],
        0.0,
    );
    // Only what is drawn counts: an off-canvas preheader or a ticker clipped to its box
    // neither scales the page down nor makes it taller.
    let (right, bottom) = clip::visible_extent(&out.items);
    out.content_width = right.max(width);
    // `html,body{height:100%}` makes the root box the viewport's height (1px) with the mail
    // overflowing it: the page is at least as tall as what it draws.
    out.height = out.height.max(bottom);
    out
}

/// The base URL relative `src`s resolve against (inert: Blitz loads nothing).
const BASE_URL: &str = "https://mail.invalid/";

/// Give each `<img>` the reader has decoded its intrinsic size, as a browser
/// would once the image loads. Blitz fetches nothing itself, and an image with
/// no data sizes from its attributes alone — so `width:100%;height:auto` in a
/// shrink-to-fit cell collapses to 0×0. Layout only reads the dimensions, so
/// the pixel data stays empty. Images not decoded keep the box their attributes
/// describe (`unloaded_image_size`, `alt_text_lines`).
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
            .or_else(|| unloaded_image_size(node))
        {
            imgs.push((id, w, h));
        }
    }
    alt_text_lines(doc, image_size);
    for (id, w, h) in imgs {
        if let Some(el) = doc.get_node_mut(id).and_then(|n| n.element_data_mut()) {
            el.special_data = SpecialElementData::Image(Box::new(ImageData::Raster(
                RasterImageData::new(w, h, std::sync::Arc::new(Vec::new())),
            )));
        }
    }
}

/// The box an image that has not loaded (blocked, still fetching, failed, or in a format
/// the reader cannot decode) keeps, so that loading it does not reflow the mail. Browsers
/// map `width` and `height` attributes to `aspect-ratio: auto w / h` (HTML "attributes for
/// embedded content"), which holds the box at the CSS width — `height:auto` included —
/// before the image arrives; Blitz has no such mapping. An image with both, as non-zero
/// non-percentage lengths, is laid out as if it had loaded at that size: the same ratio,
/// and an intrinsic width that is right for every image drawn at its declared size.
fn unloaded_image_size(node: &Node) -> Option<(u32, u32)> {
    let w = dimension_attr(node.attr(local_name!("width"))?)?;
    let h = dimension_attr(node.attr(local_name!("height"))?)?;
    Some((w.round().max(1.0) as u32, h.round().max(1.0) as u32))
}

/// An HTML dimension attribute ("rules for parsing dimension values") as a length in px:
/// leading digits and an optional fraction, anything after ignored (`600px`), `None` for a
/// percentage, zero or no number at all.
fn dimension_attr(value: &str) -> Option<f32> {
    let v = value.trim_start();
    let end = v
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(v.len());
    let n: f32 = v[..end].trim_end_matches('.').parse().ok()?;
    let percent = v[end..].trim_start().starts_with('%');
    (n > 0.0 && n.is_finite() && !percent).then_some(n)
}

/// How tall an unloaded image with only a `width` is held, when it has alt text to show.
const ALT_LINE: f32 = 20.0;

/// An unloaded image with a `width` but no `height` has no height until it arrives. When
/// it has alt text, hold one line for it (the reader draws the text in the frame), as a
/// browser shows a broken image's alt text; without, it stays collapsed — any other guess
/// at the height would still reflow when the image loads. The mail's own `min-height`
/// follows in the `style` attribute, so it still wins.
fn alt_text_lines(doc: &mut BaseDocument, image_size: &impl Fn(&str) -> Option<(u32, u32)>) {
    use blitz_dom::{ns, QualName};
    let mut found = Vec::new();
    let mut stack = vec![doc.root_node().id];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get_node(id) else {
            continue;
        };
        stack.extend(node.children.iter().copied());
        if !node.data.is_element_with_tag_name(&local_name!("img"))
            || node.attr(local_name!("src")).and_then(image_size).is_some()
            || node
                .attr(local_name!("alt"))
                .is_none_or(|a| a.trim().is_empty())
            || node
                .attr(local_name!("height"))
                .and_then(dimension_attr)
                .is_some()
            || node
                .attr(local_name!("width"))
                .and_then(dimension_attr)
                .is_none_or(|w| w <= 2.0)
        {
            continue;
        }
        let style = node.attr(local_name!("style")).unwrap_or("").to_string();
        found.push((id, style));
    }
    let mut m = doc.mutate();
    for (id, style) in found {
        m.set_attribute(
            id,
            QualName::new(None, ns!(), local_name!("style")),
            &format!("min-height:{ALT_LINE}px;{style}"),
        );
    }
}

/// Browsers treat a table's (or a row's) `height` as a minimum and grow it to fit
/// its content; Blitz applies it as a fixed height, so mail that sets
/// `<table height=1>` around a block of text overflows it and the next table is
/// drawn over the text. Rewrite those attributes as `min-height` before styles
/// resolve. The element's own inline style comes after, so it still wins.
fn table_heights_as_minimums(doc: &mut BaseDocument) {
    use blitz_dom::{ns, QualName};
    let tags = [
        local_name!("table"),
        local_name!("thead"),
        local_name!("tbody"),
        local_name!("tfoot"),
        local_name!("tr"),
    ];
    let mut found = Vec::new();
    let mut stack = vec![doc.root_node().id];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get_node(id) else {
            continue;
        };
        stack.extend(node.children.iter().copied());
        if !tags.iter().any(|t| node.data.is_element_with_tag_name(t)) {
            continue;
        }
        if let Some(height) = node.attr(local_name!("height")) {
            let style = node.attr(local_name!("style")).unwrap_or("").to_string();
            found.push((id, height.trim().to_string(), style));
        }
    }
    let mut m = doc.mutate();
    for (id, height, style) in found {
        m.clear_attribute(id, QualName::new(None, ns!(), local_name!("height")));
        let min = if height.ends_with('%') {
            height
        } else if height.parse::<f32>().is_ok_and(|h| h > 0.0) {
            format!("{height}px")
        } else {
            continue;
        };
        m.set_attribute(
            id,
            QualName::new(None, ns!(), local_name!("style")),
            &format!("min-height:{min};{style}"),
        );
    }
}

/// The legacy `background="url"` attribute on `<body>`, `<table>`, `<tr>`, `<td>` and
/// `<th>`, which Blitz does not map: a background image under the element's own styles,
/// which come after it in the `style` attribute and so still win.
fn background_attributes(doc: &mut BaseDocument) {
    use blitz_dom::{ns, QualName};
    let tags = [
        local_name!("body"),
        local_name!("table"),
        local_name!("tr"),
        local_name!("td"),
        local_name!("th"),
    ];
    let mut found = Vec::new();
    let mut stack = vec![doc.root_node().id];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get_node(id) else {
            continue;
        };
        stack.extend(node.children.iter().copied());
        if !tags.iter().any(|t| node.data.is_element_with_tag_name(t)) {
            continue;
        }
        let Some(url) = node
            .attr(local_name!("background"))
            .map(str::trim)
            .filter(|u| !u.is_empty())
        else {
            continue;
        };
        let style = node.attr(local_name!("style")).unwrap_or("").to_string();
        let url = url.replace('\\', "\\\\").replace('"', "\\\"");
        found.push((id, format!("background-image:url(\"{url}\");{style}")));
    }
    let mut m = doc.mutate();
    for (id, style) in found {
        m.set_attribute(id, QualName::new(None, ns!(), local_name!("style")), &style);
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

    // Box: background, border, shadow; then background images over it.
    if let Some(item) = paint::element_box(
        &styles,
        Some([l.border.top, l.border.right, l.border.bottom, l.border.left]),
        (x, y, w, h),
    ) {
        // Mail nests same-coloured tables in cells of the same size; one box is enough.
        if out.last() != Some(&item) {
            out.push(item);
        }
    }
    paint::collapsed_cell_border(doc, node, &styles, (x, y, w, h), out);
    paint::background_images(&styles, l, (x, y, w, h), out);
    paint::row_backgrounds(doc, node, x, y, out);
    let radius = paint::top_left_radius(&styles, w, h);

    // Everything after the box itself is clipped by its `overflow`, outside list markers too.
    let clip_start = out.len();

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
                radius,
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
            paint::inline_boxes(doc, id, &text.layout, cx, cy, out);
            text_items(doc, &text.text, &text.layout, cx, cy, false, out);
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
        // Bullets are laid out in `monospace` (the code face, see `FONTS`); numbers in the
        // item's own font.
        let (source, pad, bullet) = match marker {
            blitz_dom::node::Marker::Char(c) => (format!("{c} "), 8.0, true),
            blitz_dom::node::Marker::String(s) => (s.clone(), 0.0, false),
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
            bullet,
            out,
        );
    }

    if let Some(children) = &*node.paint_children.borrow() {
        for child in children {
            walk(doc, *child, x, y, out);
        }
    }
    if let Some((rect, radius)) = clip::clip_of(node, x, y, radius) {
        clip::clip_subtree(out, clip_start, rect, radius);
    }
}

fn text_items(
    doc: &BaseDocument,
    source: &str,
    layout: &parley::Layout<blitz_dom::node::TextBrush>,
    cx: f32,
    cy: f32,
    force_mono: bool,
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
            // Text a browser lays out but nobody sees: `font-size:0` wrappers, `color:transparent`.
            let color = rgba(s.clone_color().as_color_color());
            if run.font_size() < 1.0 || color[3] == 0 {
                continue;
            }
            let font = s.get_font();
            let fm = run.font_metrics();
            let tracking = s
                .get_inherited_text()
                .letter_spacing
                .0
                .resolve(style::values::computed::Length::new(run.font_size()))
                .px();
            let piece = Piece {
                x: cx + gr.offset(),
                end_x: cx + gr.offset() + gr.advance(),
                baseline: cy + gr.baseline(),
                ascent: fm.ascent,
                descent: fm.descent.abs(),
                size: run.font_size(),
                tracking,
                text,
                span: TextSpan {
                    range: 0..0,
                    weight: font.font_weight.value(),
                    italic: font.font_style != style::values::computed::font::FontStyle::NORMAL,
                    color,
                    underline: decoration(doc, id, Decoration::UNDERLINE),
                    strike: decoration(doc, id, Decoration::LINE_THROUGH),
                    mono: force_mono || is_mono(font),
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
    tracking: f32,
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
    tracking: f32,
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
            tracking: piece.tracking,
            text: String::new(),
            spans: Vec::new(),
        };
        g.push(piece);
        g
    }

    fn continues(&self, p: &Piece) -> bool {
        (p.baseline - self.baseline).abs() < 0.01
            && (p.size - self.size).abs() < 0.01
            && (p.tracking - self.tracking).abs() < 0.01
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
            tracking: self.tracking,
            text: self.text,
            spans: self.spans,
        });
    }
}

/// Whether a run's computed font is the foundation's code face (see the UA rule in
/// `layout_inner`).
fn is_mono(font: &style::properties::style_structs::Font) -> bool {
    use style::values::computed::font::SingleFontFamily;
    matches!(
        font.font_family.families.iter().next(),
        Some(SingleFontFamily::FamilyName(f)) if &*f.name == rune_theme_foundation::FAMILIES.1
    )
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

use style::values::specified::TextDecorationLine as Decoration;

/// The colour of the `line` decoration (underline, line-through) a run's text carries,
/// if any. Decorations propagate from the element that sets them to all its in-flow
/// descendants, in that element's `text-decoration-color` (its `color` by default), and
/// a descendant's `text-decoration: none` cannot remove them; they stop at inline blocks,
/// floats and positioned boxes.
fn decoration(doc: &BaseDocument, mut id: NodeId, line: Decoration) -> Option<Rgba> {
    use style::values::specified::box_::{DisplayInside, DisplayOutside};
    loop {
        let node = doc.get_node(id)?;
        if let Some(s) = node.primary_styles() {
            let text = s.get_text();
            if text.text_decoration_line.contains(line) {
                let current = s.clone_color();
                let c = rgba(
                    text.text_decoration_color
                        .resolve_to_absolute(&current)
                        .as_color_color(),
                );
                return (c[3] > 0).then_some(c);
            }
            let display = s.get_box().display;
            let atomic_inline = display.outside() == DisplayOutside::Inline
                && display.inside() != DisplayInside::Flow;
            if atomic_inline
                || s.get_box().float != style::computed_values::float::T::None
                || s.get_box().position.is_absolutely_positioned()
            {
                return None;
            }
        }
        id = node.parent?;
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
    fn a_short_table_height_grows_to_fit_its_content() {
        // Twitch's notices: `height=0`/`height=1` tables around the body text,
        // then a footer table that browsers draw below it, not over it.
        let l = lay(
            r#"<table height=1 width=100%><tr><td><div style="padding:20px 0;line-height:1.5">One</div></td></tr>
               <tr><td><div style="padding:20px 0;line-height:1.5">Two</div></td></tr></table>
               <table><tr height=2><td>Footer</td></tr></table>"#,
        );
        let y = |s| match text_item(&l, s) {
            Item::Text { y, .. } => *y,
            _ => unreachable!(),
        };
        assert!(y("One") < y("Two"), "rows stack");
        assert!(
            y("Footer") > y("Two") + 20.0,
            "footer {} drawn over the body (Two at {})",
            y("Footer"),
            y("Two")
        );
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
        assert!(span_of("here").underline.is_some());
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

    fn image_box(l: &MailLayout, want: &str) -> (f32, f32, f32, f32) {
        l.items
            .iter()
            .find_map(|i| match i {
                Item::Image {
                    x, y, w, h, src, ..
                } if src == want => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no image {want:?}"))
    }

    #[test]
    fn unloaded_images_keep_the_box_their_attributes_declare() {
        // Remote images are blocked until the reader allows them: the page must not
        // collapse, and must not reflow when they arrive (here at twice the size, hi-dpi).
        let html = r#"<img src="https://cdn.example/hero.png" width="600" height="200"
                        style="display:block;width:100%;height:auto"><p>After</p>"#;
        let blocked = lay(html);
        let sizes = [("https://cdn.example/hero.png".to_string(), (1200, 400))]
            .into_iter()
            .collect();
        let loaded = layout(html, 600.0, &sizes);
        let (_, _, w, h) = image_box(&blocked, "https://cdn.example/hero.png");
        assert!((h - w / 3.0).abs() < 0.5, "{w}x{h}: the 3:1 ratio");
        assert_eq!(
            image_box(&blocked, "https://cdn.example/hero.png"),
            image_box(&loaded, "https://cdn.example/hero.png")
        );
        assert_eq!(text_item(&blocked, "After"), text_item(&loaded, "After"));
    }

    #[test]
    fn unloaded_images_with_only_a_width_hold_a_line_for_their_alt_text() {
        let l = lay(
            r#"<img src="https://a.example/1.png" width="300" alt="Spring sale">
               <img src="https://a.example/2.png" width="300" alt="">
               <img src="https://a.example/3.png" width="100%" height="50%" alt="Banner">
               <img src="https://a.example/4.png" width="300" height="auto" style="min-height:40px" alt="Logo">"#,
        );
        assert_eq!(image_box(&l, "https://a.example/1.png").3, ALT_LINE);
        assert_eq!(
            image_box(&l, "https://a.example/2.png").3,
            0.0,
            "no alt: collapsed"
        );
        assert_eq!(
            image_box(&l, "https://a.example/3.png").3,
            0.0,
            "percentages declare no size"
        );
        assert_eq!(
            image_box(&l, "https://a.example/4.png").3,
            40.0,
            "the mail's own min-height wins"
        );
        // Once decoded, the image's own size rules.
        let sizes = [("https://a.example/1.png".to_string(), (600, 100))]
            .into_iter()
            .collect();
        let l = layout(
            r#"<img src="https://a.example/1.png" width="300" alt="Spring sale">"#,
            600.0,
            &sizes,
        );
        assert_eq!(image_box(&l, "https://a.example/1.png").3, 50.0);
    }

    #[test]
    fn dimension_attributes_parse_as_browsers_do() {
        assert_eq!(dimension_attr("600"), Some(600.0));
        assert_eq!(dimension_attr(" 37.5px"), Some(37.5));
        assert_eq!(dimension_attr("100%"), None);
        assert_eq!(dimension_attr("0"), None);
        assert_eq!(dimension_attr("auto"), None);
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
        let Item::Text { spans, .. } = text_item(&l, "•") else {
            unreachable!()
        };
        assert!(
            spans[0].mono,
            "bullets draw in the code face, which has `•`"
        );
        let l = lay("<ol><li>First</li></ol>");
        let Item::Text { spans, .. } = text_item(&l, "1.") else {
            unreachable!()
        };
        assert!(!spans[0].mono, "numbers stay in the item's font");
    }

    #[test]
    fn bidi_controls_alone_make_no_item() {
        let l = lay("<p>Order # \u{202b}026-1</p>");
        assert!(texts(&l).iter().all(|t| !t.contains('\u{202b}')));
        assert!(texts(&l).iter().any(|t| t.contains("026-1")));
    }

    #[test]
    fn code_is_set_in_the_code_face() {
        let l = lay(r#"<p>category <code>flagged_feature</code> is new</p>
               <pre><span class="pl-s">"schema"</span>: 1</pre>
               <p><span style="font-family: Courier New, monospace">a1b2c3</span></p>"#);
        let span = |want: &str| {
            l.items
                .iter()
                .find_map(|i| match i {
                    Item::Text { text, spans, .. } => spans
                        .iter()
                        .find(|sp| &text[sp.range.clone()] == want)
                        .cloned(),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no span {want:?} in {:?}", texts(&l)))
        };
        assert!(span("flagged_feature").mono);
        assert!(!span("category ").mono, "prose around code stays sans");
        assert!(span("\"schema\": 1").mono, "highlighted spans inside <pre>");
        assert!(span("a1b2c3").mono, "monospace inline styles");
        // Monospace: every character the same width.
        let width = |want: &str| match text_item(&lay(&format!("<code>{want}</code>")), want) {
            Item::Text { w, .. } => *w,
            _ => unreachable!(),
        };
        assert!((width("iiii") - width("WWWW")).abs() < 0.01);
    }

    #[test]
    fn an_xhtml_doctype_still_parses_as_html() {
        // As XML: the `<style>` text is drawn, and everything after the `<br>` is lost.
        let l = lay(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd">
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title><style>p{color:#ff0000}</style></head>
<body><p>One&nbsp;line<br>Two</p></body></html>"#);
        assert_eq!(texts(&l), vec!["One\u{a0}line", "Two"]);
        let l =
            lay(r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>A<br>B</p></body></html>"#);
        assert_eq!(texts(&l), vec!["A", "B"]);
    }

    #[test]
    fn relative_image_urls_do_not_lose_the_mail() {
        // Blitz panics resolving these without a base URL, and the layout came back empty.
        let l = lay(
            r#"<p>Hello</p><img src="//cdn.example/a.png" width="20" height="20"><img src="b.png" width="20" height="20">"#,
        );
        assert_eq!(texts(&l), vec!["Hello"]);
        assert_eq!(
            l.items
                .iter()
                .filter(|i| matches!(i, Item::Image { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn urls_blitz_cannot_resolve_do_not_lose_the_mail() {
        // Each of these panicked Blitz even with a base URL.
        for html in [
            r#"<style>@import url();</style><p>Hello</p>"#,
            r#"<svg><style>&#64;IMPORT "http://"</style></svg><p>Hello</p>"#,
            r#"<link rel="stylesheet" href="http://"><p>Hello</p>"#,
            r#"<p>Hello</p><img src="http://*|IMG|*/a.png" width="20" height="20">"#,
        ] {
            assert_eq!(texts(&lay(html)), vec!["Hello"], "{html}");
        }
        // The rules after a defused `@import` still apply.
        let l = lay(r#"<style>@import url();p{color:#ff0000}</style><p>Hello</p>"#);
        let Item::Text { spans, .. } = text_item(&l, "Hello") else {
            unreachable!()
        };
        assert_eq!(spans[0].color, [255, 0, 0, 255]);
    }

    #[test]
    fn thai_breaks_between_words() {
        // Needs blitz-dom's `complex-scripts` feature: Thai has no spaces, and without a
        // dictionary the paragraph is one line overflowing its box.
        let l = lay(r#"<p style="width:120px">ข้อความภาษาไทยที่ยาวมากและไม่มีช่องว่างเลย</p>"#);
        assert!(texts(&l).len() > 1, "{:?}", texts(&l));
        assert!(l.content_width <= 600.0);
    }

    #[test]
    fn a_full_height_body_does_not_collapse_the_page() {
        let l = lay(
            "<html><head><style>html,body{height:100%}</style></head><body><p>One</p><p>Two</p></body></html>",
        );
        let Item::Text { y, line_height, .. } = text_item(&l, "Two") else {
            unreachable!()
        };
        assert!(l.height >= y + line_height, "page {} tall", l.height);
        // The viewport is 1px tall, so `100vh` adds no empty space either.
        let l = lay(r#"<div style="min-height:100vh">One</div>"#);
        assert!(l.height < 50.0, "page {} tall", l.height);
    }

    #[test]
    fn floats_sit_side_by_side() {
        // Needs blitz-dom's `floats` feature: without it floated blocks stack.
        let l = lay(
            r#"<div style="width:300px"><div style="float:left;width:100px">Left</div><div style="float:right;width:100px">Right</div></div>
               <table width="150" align="left"><tr><td>ColA</td></tr></table><table width="150" align="left"><tr><td>ColB</td></tr></table>"#,
        );
        let pos = |s| match text_item(&l, s) {
            Item::Text { x, y, .. } => (*x, *y),
            _ => unreachable!(),
        };
        assert!((pos("Left").1 - pos("Right").1).abs() < 1.0);
        assert!(pos("Right").0 > pos("Left").0 + 150.0);
        assert!(
            (pos("ColA").1 - pos("ColB").1).abs() < 1.0,
            "tables with align=left float"
        );
        assert!(pos("ColB").0 > pos("ColA").0 + 100.0);
    }

    #[test]
    fn page_takes_the_body_background() {
        let l = lay(r#"<body style="background:#f0f2f2"><p>x</p></body>"#);
        assert_eq!(l.background, Some([0xf0, 0xf2, 0xf2, 255]));
        assert!(lay("<p>x</p>").background.is_none());
    }

    fn span<'a>(l: &'a MailLayout, want: &str) -> &'a TextSpan {
        l.items
            .iter()
            .find_map(|i| match i {
                Item::Text { text, spans, .. } => {
                    spans.iter().find(|s| &text[s.range.clone()] == want)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no span {want:?}"))
    }

    #[test]
    fn struck_and_underlined_text_carry_the_decorating_elements_colour() {
        let l = lay(
            r#"<p><s>$10</s> <del style="color:#888">$9</del> <span style="text-decoration:line-through;text-decoration-color:#f00">$8</span> $7</p>
               <a href="https://shop.example" style="color:#00f"><div><span style="color:#f00">Linked</span></div></a>"#,
        );
        assert_eq!(span(&l, "$10").strike, Some([0, 0, 0, 255]));
        assert_eq!(span(&l, "$9").strike, Some([0x88, 0x88, 0x88, 255]));
        assert_eq!(span(&l, "$8").strike, Some([255, 0, 0, 255]));
        // The link's underline reaches into the block inside it, in the link's colour.
        assert_eq!(span(&l, "Linked").underline, Some([0, 0, 255, 255]));
        assert_eq!(span(&l, "Linked").color, [255, 0, 0, 255]);
        // `text-decoration: none` on an inline block stops it.
        let l = lay(
            r#"<a href="https://shop.example"><span style="display:inline-block;text-decoration:none">Button</span></a>"#,
        );
        assert_eq!(span(&l, "Button").underline, None);
    }

    #[test]
    fn letter_spacing_reaches_the_text_item() {
        let l = lay(r#"<p style="letter-spacing:2px">SPACED</p><p>plain</p>"#);
        let tracking = |want: &str| {
            l.items.iter().find_map(|i| match i {
                Item::Text { text, tracking, .. } if text == want => Some(*tracking),
                _ => None,
            })
        };
        assert_eq!(tracking("SPACED"), Some(2.0));
        assert_eq!(tracking("plain"), Some(0.0));
    }
}
