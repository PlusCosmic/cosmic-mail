//! What a box looks like, beyond where it is: per-side borders and corner radii,
//! gradients, shadows and background images, collapsed table borders, row
//! backgrounds and the boxes of inline elements. `walk` places boxes; this reads
//! the computed style the way `blitz-paint`'s `render_element` does and turns it
//! into display-list items the reader can draw.

use super::{rgba, Item, Rgba};
use blitz_dom::node::{SpecialElementData, TextBrush};
use blitz_dom::util::ToColorColor;
use blitz_dom::{BaseDocument, Node, NodeId};
use parley::PositionedLayoutItem;
use style::computed_values::border_collapse::T as BorderCollapse;
use style::properties::ComputedValues;
use style::values::computed::Length;
use style::values::specified::BorderStyle;

/// How a border side is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineStyle {
    Solid,
    Dashed,
    Dotted,
}

/// An element's border: each side's width, colour and style, in CSS order (top, right,
/// bottom, left). A side that draws nothing has width 0.
#[derive(Debug, Clone, PartialEq)]
pub struct Border {
    pub widths: [f32; 4],
    pub colors: [Rgba; 4],
    pub styles: [LineStyle; 4],
}

/// A `linear-gradient()` background, painted over the fill.
#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    /// CSS angle in radians: 0 runs bottom to top, π/2 left to right, π top to bottom.
    pub angle: f32,
    /// Offsets along the gradient line (0..=1) and colours; at most 8 (Rune's limit).
    pub stops: Vec<(f32, Rgba)>,
}

/// One `box-shadow`.
#[derive(Debug, Clone, PartialEq)]
pub struct Shadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Rgba,
    pub inset: bool,
}

/// How a background image is sized in its area (`background-size`), resolved to px
/// where the area alone decides it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BgSize {
    Cover,
    Contain,
    /// Width and height; `None` is `auto` (from the image's own size and aspect).
    Explicit(Option<f32>, Option<f32>),
}

/// The paint of an element's box, before it has a place.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct BoxPaint {
    pub fill: Option<Rgba>,
    pub border: Option<Border>,
    pub radius: [f32; 4],
    pub gradient: Option<Gradient>,
    pub shadows: Vec<Shadow>,
}

impl BoxPaint {
    fn draws(&self) -> bool {
        self.fill.is_some()
            || self.border.is_some()
            || self.gradient.is_some()
            || !self.shadows.is_empty()
    }

    fn item(self, x: f32, y: f32, w: f32, h: f32) -> Option<Item> {
        (self.draws() && w > 0.0 && h > 0.0).then_some(Item::Box {
            x,
            y,
            w,
            h,
            fill: self.fill,
            border: self.border,
            radius: self.radius,
            gradient: self.gradient,
            shadows: self.shadows,
        })
    }
}

/// An element's box item at its border box, with border `widths` (the layout's, which
/// already say 0 for a `none` side) or `None` to take them from the style.
pub(super) fn element_box(
    styles: &ComputedValues,
    widths: Option<[f32; 4]>,
    (x, y, w, h): (f32, f32, f32, f32),
) -> Option<Item> {
    box_paint(styles, widths, w, h).item(x, y, w, h)
}

pub(super) fn box_paint(
    styles: &ComputedValues,
    widths: Option<[f32; 4]>,
    w: f32,
    h: f32,
) -> BoxPaint {
    let mut paint = BoxPaint {
        fill: super::fill_of(styles),
        border: border_of(styles, widths.unwrap_or_else(|| style_widths(styles))),
        radius: radii(styles, w, h),
        gradient: None,
        shadows: shadows(styles),
    };
    // The topmost gradient layer. A flat fill stands in for one Rune cannot draw, so
    // light text on a gradient button never lands on the page's white.
    for image in styles.get_background().background_image.0.iter() {
        match gradient(styles, image, w, h) {
            Some(Ok(g)) => {
                paint.gradient = Some(g);
                break;
            }
            Some(Err(avg)) => {
                paint.fill = paint.fill.or(Some(avg));
                break;
            }
            None => {}
        }
    }
    paint
}

/// The corner radius of the top-left corner (what an image is rounded by).
pub(super) fn top_left_radius(styles: &ComputedValues, w: f32, h: f32) -> f32 {
    radii(styles, w, h)[0]
}

/// Border widths from the style alone (`none`/`hidden` sides are 0), for boxes whose
/// layout has none: collapsed table cells and inline elements.
fn style_widths(styles: &ComputedValues) -> [f32; 4] {
    let b = styles.get_border();
    [
        side_width(b.border_top_width.0.to_f32_px(), b.border_top_style),
        side_width(b.border_right_width.0.to_f32_px(), b.border_right_style),
        side_width(b.border_bottom_width.0.to_f32_px(), b.border_bottom_style),
        side_width(b.border_left_width.0.to_f32_px(), b.border_left_style),
    ]
}

fn side_width(width: f32, style: BorderStyle) -> f32 {
    if style.none_or_hidden() {
        0.0
    } else {
        width
    }
}

fn border_of(styles: &ComputedValues, mut widths: [f32; 4]) -> Option<Border> {
    let b = styles.get_border();
    let current = styles.clone_color();
    let colors = [
        &b.border_top_color,
        &b.border_right_color,
        &b.border_bottom_color,
        &b.border_left_color,
    ]
    .map(|c| rgba(c.resolve_to_absolute(&current).as_color_color()));
    let line_styles = [
        b.border_top_style,
        b.border_right_style,
        b.border_bottom_style,
        b.border_left_style,
    ];
    for i in 0..4 {
        if colors[i][3] == 0 || line_styles[i].none_or_hidden() {
            widths[i] = 0.0;
        }
    }
    let styles = line_styles.map(|s| match s {
        BorderStyle::Dashed => LineStyle::Dashed,
        BorderStyle::Dotted => LineStyle::Dotted,
        // Double, groove, ridge, inset and outset draw as solid lines of the full width.
        _ => LineStyle::Solid,
    });
    widths.iter().any(|w| *w > 0.0).then_some(Border {
        widths,
        colors,
        styles,
    })
}

/// Corner radii (top-left, top-right, bottom-right, bottom-left) at a `w`×`h` box, from
/// each corner's horizontal radius (Rune's corners are circular), scaled down together
/// where adjacent ones would overlap, as CSS does.
fn radii(styles: &ComputedValues, w: f32, h: f32) -> [f32; 4] {
    let b = styles.get_border();
    let r = |c: &style::values::computed::BorderCornerRadius| {
        let x = c.0.width.0.resolve(Length::new(w)).px();
        let y = c.0.height.0.resolve(Length::new(h)).px();
        x.min(y).max(0.0)
    };
    let radii = [
        r(&b.border_top_left_radius),
        r(&b.border_top_right_radius),
        r(&b.border_bottom_right_radius),
        r(&b.border_bottom_left_radius),
    ];
    let sums = [
        (radii[0] + radii[1], w),
        (radii[1] + radii[2], h),
        (radii[2] + radii[3], w),
        (radii[3] + radii[0], h),
    ];
    let f = sums
        .iter()
        .filter(|(s, _)| *s > 0.0)
        .map(|(s, side)| side / s)
        .fold(1.0f32, f32::min);
    radii.map(|r| r * f)
}

fn shadows(styles: &ComputedValues) -> Vec<Shadow> {
    let current = styles.clone_color();
    styles
        .get_effects()
        .box_shadow
        .0
        .iter()
        .map(|s| Shadow {
            x: s.base.horizontal.px(),
            y: s.base.vertical.px(),
            blur: s.base.blur.px(),
            spread: s.spread.px(),
            color: rgba(s.base.color.resolve_to_absolute(&current).as_color_color()),
            inset: s.inset,
        })
        .filter(|s| s.color[3] > 0)
        .collect()
}

/// A background layer as a gradient Rune draws (`Ok`), or as the average of its colours
/// for one it cannot (`Err`: radial, conic, repeating); `None` for a layer that is not a
/// gradient.
fn gradient(
    styles: &ComputedValues,
    image: &style::values::computed::Image,
    w: f32,
    h: f32,
) -> Option<Result<Gradient, Rgba>> {
    use std::f32::consts::PI;
    use style::values::computed::image::{Gradient as G, LineDirection};
    use style::values::generics::image::{GenericGradientItem as GI, GradientFlags, Image};
    use style::values::specified::position::{
        HorizontalPositionKeyword as HK, VerticalPositionKeyword as VK,
    };

    let Image::Gradient(g) = image else {
        return None;
    };
    let current = styles.clone_color();
    let color =
        |c: &style::values::computed::Color| rgba(c.resolve_to_absolute(&current).as_color_color());
    let (items, angle, flags) = match &**g {
        G::Linear {
            direction,
            items,
            flags,
            ..
        } => {
            let diag = h.atan2(w.max(0.001));
            let angle = match direction {
                LineDirection::Angle(a) => a.radians(),
                LineDirection::Vertical(VK::Top) => 0.0,
                LineDirection::Vertical(VK::Bottom) => PI,
                LineDirection::Horizontal(HK::Right) => PI / 2.0,
                LineDirection::Horizontal(HK::Left) => 3.0 * PI / 2.0,
                LineDirection::Corner(HK::Right, VK::Top) => diag,
                LineDirection::Corner(HK::Right, VK::Bottom) => PI - diag,
                LineDirection::Corner(HK::Left, VK::Bottom) => PI + diag,
                LineDirection::Corner(HK::Left, VK::Top) => -diag,
            };
            (items, Some(angle), *flags)
        }
        G::Radial { items, flags, .. } => (items, None, *flags),
        G::Conic { items, flags, .. } => {
            let colors: Vec<Rgba> = items
                .iter()
                .filter_map(|i| match i {
                    GI::SimpleColorStop(c) | GI::ComplexColorStop { color: c, .. } => {
                        Some(color(c))
                    }
                    GI::InterpolationHint(_) => None,
                })
                .collect();
            let _ = flags;
            return average(&colors).map(Err);
        }
    };
    // The gradient line's length, which length stops are measured along.
    let len = angle.map_or(w.max(h), |a| (w * a.sin()).abs() + (h * a.cos()).abs());
    let mut stops: Vec<(Option<f32>, Rgba)> = Vec::new();
    for item in items.iter() {
        match item {
            GI::SimpleColorStop(c) => stops.push((None, color(c))),
            GI::ComplexColorStop { color: c, position } => {
                let at = position.resolve(Length::new(len.max(0.001))).px() / len.max(0.001);
                stops.push((Some(at), color(c)));
            }
            GI::InterpolationHint(_) => {}
        }
    }
    let colors: Vec<Rgba> = stops.iter().map(|s| s.1).collect();
    let (Some(angle), false) = (angle, flags.contains(GradientFlags::REPEATING)) else {
        return average(&colors).map(Err);
    };
    if stops.is_empty() {
        return None;
    }
    // Unpositioned stops: the first at 0, the last at 1, the rest spread evenly between
    // their positioned neighbours; positions never go backwards.
    let n = stops.len();
    if stops[0].0.is_none() {
        stops[0].0 = Some(0.0);
    }
    if stops[n - 1].0.is_none() {
        stops[n - 1].0 = Some(1.0);
    }
    let mut i = 0;
    while i < n {
        if stops[i].0.is_some() {
            i += 1;
            continue;
        }
        let start = i - 1;
        let end = (i..n).find(|&j| stops[j].0.is_some()).unwrap_or(n - 1);
        let (a, b) = (stops[start].0.unwrap_or(0.0), stops[end].0.unwrap_or(1.0));
        for (k, stop) in stops.iter_mut().enumerate().take(end).skip(i) {
            stop.0 = Some(a + (b - a) * (k - start) as f32 / (end - start) as f32);
        }
        i = end;
    }
    let mut last = f32::MIN;
    let mut out: Vec<(f32, Rgba)> = stops
        .into_iter()
        .map(|(at, c)| {
            last = last.max(at.unwrap_or(0.0));
            (last, c)
        })
        .collect();
    if out.len() > 8 {
        // Keep the ends and an even spread of the middle.
        let picked: Vec<(f32, Rgba)> = (0..8).map(|k| out[k * (out.len() - 1) / 7]).collect();
        out = picked;
    }
    if out.len() == 1 {
        return Some(Err(out[0].1));
    }
    Some(Ok(Gradient { angle, stops: out }))
}

fn average(colors: &[Rgba]) -> Option<Rgba> {
    if colors.is_empty() {
        return None;
    }
    let n = colors.len() as u32;
    let mut sum = [0u32; 4];
    for c in colors {
        for k in 0..4 {
            sum[k] += c[k] as u32;
        }
    }
    let avg = sum.map(|s| (s / n) as u8);
    (avg[3] > 0).then_some(avg)
}

/// Background images (`background-image: url()`), bottom layer first, drawn in the
/// padding box and clipped to it.
pub(super) fn background_images(
    styles: &ComputedValues,
    l: &taffy::Layout,
    (x, y, w, h): (f32, f32, f32, f32),
    out: &mut Vec<Item>,
) {
    use style::values::generics::background::BackgroundSize as BS;
    use style::values::generics::image::Image;
    use style::values::generics::length::GenericLengthPercentageOrAuto as LPA;
    use style::values::specified::background::BackgroundRepeatKeyword as RK;

    let bg = styles.get_background();
    let layers = &bg.background_image.0;
    if !layers.iter().any(|i| matches!(i, Image::Url(_))) {
        return;
    }
    let area = (
        x + l.border.left,
        y + l.border.top,
        (w - l.border.left - l.border.right).max(0.0),
        (h - l.border.top - l.border.bottom).max(0.0),
    );
    if area.2 <= 0.0 || area.3 <= 0.0 {
        return;
    }
    let outer = radii(styles, w, h);
    let inner = [
        (outer[0] - l.border.left.max(l.border.top)).max(0.0),
        (outer[1] - l.border.right.max(l.border.top)).max(0.0),
        (outer[2] - l.border.right.max(l.border.bottom)).max(0.0),
        (outer[3] - l.border.left.max(l.border.bottom)).max(0.0),
    ];
    // Lists shorter than the image list repeat (CSS "repeated as necessary").
    let pick = |n: usize, i: usize| if n == 0 { None } else { Some(i % n) };
    for (i, image) in layers.iter().enumerate().rev() {
        let Image::Url(url) = image else {
            continue;
        };
        let Some(src) = url_string(url) else {
            continue;
        };
        let size = pick(bg.background_size.0.len(), i)
            .map(|k| match &bg.background_size.0[k] {
                BS::Cover => BgSize::Cover,
                BS::Contain => BgSize::Contain,
                BS::ExplicitSize { width, height } => {
                    let px =
                        |v: &style::values::generics::length::GenericLengthPercentageOrAuto<
                            style::values::computed::NonNegativeLengthPercentage,
                        >,
                         basis: f32| match v {
                            LPA::Auto => None,
                            LPA::LengthPercentage(lp) => {
                                Some(lp.0.resolve(Length::new(basis)).px())
                            }
                        };
                    BgSize::Explicit(px(width, area.2), px(height, area.3))
                }
            })
            .unwrap_or(BgSize::Explicit(None, None));
        // A position is `px + fraction × (area − image)`; split it at layout, when the
        // image's size is not known yet. Positions are linear in their basis.
        let split = |lp: &style::values::computed::LengthPercentage| {
            let px = lp.resolve(Length::new(0.0)).px();
            let frac = (lp.resolve(Length::new(100.0)).px() - px) / 100.0;
            (frac, px)
        };
        let pos_x = pick(bg.background_position_x.0.len(), i)
            .map(|k| split(&bg.background_position_x.0[k]))
            .unwrap_or((0.0, 0.0));
        let pos_y = pick(bg.background_position_y.0.len(), i)
            .map(|k| split(&bg.background_position_y.0[k]))
            .unwrap_or((0.0, 0.0));
        let repeat = pick(bg.background_repeat.0.len(), i)
            .map(|k| {
                let r = &bg.background_repeat.0[k];
                [r.0 != RK::NoRepeat, r.1 != RK::NoRepeat]
            })
            .unwrap_or([true, true]);
        out.push(Item::Background {
            x: area.0,
            y: area.1,
            w: area.2,
            h: area.3,
            radius: inner,
            src,
            size,
            position: [pos_x, pos_y],
            repeat,
        });
    }
}

fn url_string(url: &style::values::computed::url::ComputedUrl) -> Option<String> {
    use style::values::computed::url::ComputedUrl;
    match url {
        ComputedUrl::Valid(u) => {
            let s = u.as_str();
            // Relative to our inert base: nothing to load.
            (!s.starts_with(super::BASE_URL)).then(|| s.to_string())
        }
        ComputedUrl::Invalid(_) => None,
    }
}

/// Where one copy of a background image goes in its area `[x, y, w, h]`, for an image
/// `natural` px in size: `[x, y, w, h]`. The reader repeats it from there.
pub fn background_tile(
    area: [f32; 4],
    natural: (f32, f32),
    size: BgSize,
    position: [(f32, f32); 2],
) -> [f32; 4] {
    let [ax, ay, aw, ah] = area;
    let (nw, nh) = (natural.0.max(1.0), natural.1.max(1.0));
    let (iw, ih) = match size {
        BgSize::Cover => {
            let k = (aw / nw).max(ah / nh);
            (nw * k, nh * k)
        }
        BgSize::Contain => {
            let k = (aw / nw).min(ah / nh);
            (nw * k, nh * k)
        }
        BgSize::Explicit(Some(w), Some(h)) => (w, h),
        BgSize::Explicit(Some(w), None) => (w, nh * w / nw),
        BgSize::Explicit(None, Some(h)) => (nw * h / nh, h),
        BgSize::Explicit(None, None) => (nw, nh),
    };
    let [(fx, px), (fy, py)] = position;
    [
        ax + px + fx * (aw - iw),
        ay + py + fy * (ah - ih),
        iw.max(0.0),
        ih.max(0.0),
    ]
}

/// The table element a cell belongs to (its parent row's parent group's parent, or nearer).
fn table_node<'a>(doc: &'a BaseDocument, node: &Node) -> Option<&'a Node> {
    let mut id = node.parent;
    for _ in 0..4 {
        let n = doc.get_node(id?)?;
        if matches!(
            n.element_data().map(|e| &e.special_data),
            Some(SpecialElementData::TableRoot(_))
        ) {
            return Some(n);
        }
        id = n.parent;
    }
    None
}

/// Row-group and row backgrounds of a table at `(x, y)`, painted behind each of the
/// row's cells as browsers do (rows are not boxes in Blitz's table layout). Group
/// backgrounds first, then rows; cells paint over both.
pub(super) fn row_backgrounds(
    doc: &BaseDocument,
    table: &Node,
    x: f32,
    y: f32,
    out: &mut Vec<Item>,
) {
    let Some(SpecialElementData::TableRoot(ctx)) = table.element_data().map(|e| &e.special_data)
    else {
        return;
    };
    let paint_of = |id: Option<NodeId>| -> Option<BoxPaint> {
        let n = doc.get_node(id?)?;
        if matches!(n.taffy_display(), taffy::Display::None) {
            return None;
        }
        let s = n.primary_styles()?;
        let mut p = box_paint(&s, Some([0.0; 4]), 0.0, 0.0);
        // Rows have no border box of their own here, nor corners or shadows.
        p.radius = [0.0; 4];
        p.shadows.clear();
        p.border = None;
        p.draws().then_some(p)
    };
    for groups in [true, false] {
        for cell in &ctx.cells {
            let Some(row) = ctx.rows.get(cell.row as usize) else {
                continue;
            };
            let part = if groups { row.group } else { Some(row.node_id) };
            let Some(paint) = paint_of(part) else {
                continue;
            };
            let Some(c) = doc.get_node(cell.node_id) else {
                continue;
            };
            if matches!(c.taffy_display(), taffy::Display::None) {
                continue;
            }
            let l = c.final_layout();
            if let Some(item) = paint.item(
                x + l.location.x,
                y + l.location.y,
                l.size.width,
                l.size.height,
            ) {
                out.push(item);
            }
        }
    }
}

/// A cell's own borders in a `border-collapse: collapse` table. Blitz lays collapsed
/// borders out as gutters between cells (as wide as the first cell's widest border) and
/// zeroes the cells' layout borders, so `walk` sees none. Each cell draws its own sides
/// into the gutters around it; where neighbours both set the shared edge, the later
/// cell's wins (browsers pick the wider, then by style — a simplification).
pub(super) fn collapsed_cell_border(
    doc: &BaseDocument,
    node: &Node,
    styles: &ComputedValues,
    (x, y, w, h): (f32, f32, f32, f32),
    out: &mut Vec<Item>,
) {
    if styles.get_inherited_table().border_collapse != BorderCollapse::Collapse
        || !matches!(
            styles.get_box().display.inside(),
            style::values::specified::box_::DisplayInside::TableCell
        )
    {
        return;
    }
    let Some(table) = table_node(doc, node) else {
        return;
    };
    let Some(SpecialElementData::TableRoot(ctx)) = table.element_data().map(|e| &e.special_data)
    else {
        return;
    };
    // The gutter, as Blitz computes it from the first cell's border.
    let (gx, gy) = ctx.border_style.as_ref().map_or((0.0, 0.0), |b| {
        let side = |w: &style::values::computed::BorderSideWidth, s| side_width(w.0.to_f32_px(), s);
        (
            side(&b.border_left_width, b.border_left_style)
                .max(side(&b.border_right_width, b.border_right_style)),
            side(&b.border_top_width, b.border_top_style)
                .max(side(&b.border_bottom_width, b.border_bottom_style)),
        )
    });
    let Some(border) = border_of(styles, style_widths(styles)) else {
        return;
    };
    let (bx, by, bw, bh) = (x - gx, y - gy, w + 2.0 * gx, h + 2.0 * gy);
    if bw <= 0.0 || bh <= 0.0 {
        return;
    }
    out.push(Item::Box {
        x: bx,
        y: by,
        w: bw,
        h: bh,
        fill: None,
        border: Some(border),
        radius: [0.0; 4],
        gradient: None,
        shadows: Vec::new(),
    });
}

/// The boxes of inline elements (`<span>`, `<a>`, `<font>` with a background, border or
/// rounded corners) in an inline root's text, one per line the element is on, drawn
/// before the text. Parley gives runs, not boxes: a fragment spans the runs and inline
/// boxes of the element and its descendants on that line, as tall as their content area,
/// grown by the element's padding and border. Horizontal padding takes no space in
/// Blitz's inline layout, so it grows outwards over its neighbours.
pub(super) fn inline_boxes(
    doc: &BaseDocument,
    root: NodeId,
    layout: &parley::Layout<TextBrush>,
    cx: f32,
    cy: f32,
    out: &mut Vec<Item>,
) {
    use std::collections::BTreeMap;
    // Per element: is it decorated (cached), and its fragments by line: x0, x1, top,
    // bottom (own runs' extent where it has any, else its descendants').
    let mut decorated: std::collections::HashMap<NodeId, bool> = Default::default();
    let mut frags: BTreeMap<NodeId, Vec<(usize, [f32; 4], bool)>> = BTreeMap::new();
    for (li, line) in layout.lines().enumerate() {
        for item in line.items() {
            let (id, own, rect) = match item {
                PositionedLayoutItem::GlyphRun(gr) => {
                    let fm = gr.run().font_metrics();
                    let base = cy + gr.baseline();
                    let x0 = cx + gr.offset();
                    (
                        gr.style().brush.id,
                        true,
                        [
                            x0,
                            x0 + gr.advance(),
                            base - fm.ascent,
                            base + fm.descent.abs(),
                        ],
                    )
                }
                PositionedLayoutItem::InlineBox(b) => {
                    let Some(parent) = doc.get_node(NodeId::from_u64(b.id)).and_then(|n| n.parent)
                    else {
                        continue;
                    };
                    let (x0, y0) = (cx + b.x, cy + b.y);
                    (parent, false, [x0, x0 + b.width, y0, y0 + b.height])
                }
            };
            let mut cur = Some(id);
            let mut first = own;
            while let Some(nid) = cur {
                if nid == root {
                    break;
                }
                let Some(n) = doc.get_node(nid) else {
                    break;
                };
                let is = *decorated.entry(nid).or_insert_with(|| {
                    n.primary_styles().is_some_and(|s| {
                        let p = box_paint(&s, None, 1.0, 1.0);
                        p.fill.is_some() || p.border.is_some() || p.gradient.is_some()
                    })
                });
                if is {
                    let list = frags.entry(nid).or_default();
                    match list.last_mut() {
                        Some((l, r, o)) if *l == li => {
                            r[0] = r[0].min(rect[0]);
                            r[1] = r[1].max(rect[1]);
                            if first && !*o {
                                // Own runs set the height; drop descendants' extent.
                                r[2] = rect[2];
                                r[3] = rect[3];
                                *o = true;
                            } else if first == *o {
                                r[2] = r[2].min(rect[2]);
                                r[3] = r[3].max(rect[3]);
                            }
                        }
                        _ => list.push((li, rect, first)),
                    }
                }
                first = false;
                cur = n.parent;
            }
        }
    }
    for (id, list) in frags {
        let Some(s) = doc.get_node(id).and_then(|n| n.primary_styles()) else {
            continue;
        };
        let p = s.get_padding();
        let pad = |v: &style::values::computed::NonNegativeLengthPercentage| {
            v.0.resolve(Length::new(0.0)).px()
        };
        let padding = [
            pad(&p.padding_top),
            pad(&p.padding_right),
            pad(&p.padding_bottom),
            pad(&p.padding_left),
        ];
        let widths = style_widths(&s);
        let n = list.len();
        for (k, (_, r, _)) in list.into_iter().enumerate() {
            let (first, last) = (k == 0, k + 1 == n);
            // Sliced across lines: the start edge on the first fragment, the end on the last.
            let left = if first { padding[3] + widths[3] } else { 0.0 };
            let right = if last { padding[1] + widths[1] } else { 0.0 };
            let x = r[0] - left;
            let y = r[2] - padding[0] - widths[0];
            let w = r[1] - r[0] + left + right;
            let h = r[3] - r[2] + padding[0] + padding[2] + widths[0] + widths[2];
            let mut sides = widths;
            if !first {
                sides[3] = 0.0;
            }
            if !last {
                sides[1] = 0.0;
            }
            let mut paint = box_paint(&s, Some(sides), w, h);
            paint.shadows.clear();
            if !first {
                paint.radius[0] = 0.0;
                paint.radius[3] = 0.0;
            }
            if !last {
                paint.radius[1] = 0.0;
                paint.radius[2] = 0.0;
            }
            if let Some(item) = paint.item(x, y, w, h) {
                out.push(item);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{layout, Item, MailLayout};
    use super::*;

    fn lay(html: &str) -> MailLayout {
        layout(html, 600.0, &Default::default())
    }

    type BoxRow = (f32, f32, f32, f32, Option<Rgba>, Option<Border>, [f32; 4]);

    /// Boxes as `(x, y, w, h, fill, border, radius)`.
    fn boxes(l: &MailLayout) -> Vec<BoxRow> {
        l.items
            .iter()
            .filter_map(|i| match i {
                Item::Box {
                    x,
                    y,
                    w,
                    h,
                    fill,
                    border,
                    radius,
                    ..
                } => Some((*x, *y, *w, *h, *fill, border.clone(), *radius)),
                _ => None,
            })
            .collect()
    }

    const RED: Rgba = [255, 0, 0, 255];
    const BLUE: Rgba = [0, 0, 255, 255];

    #[test]
    fn collapsed_table_cells_draw_their_borders() {
        // Blitz zeroes cell borders in the collapsed model; dividers must still draw.
        let l = lay(
            r#"<table width="300" style="border-collapse:collapse"><tr><td style="border-bottom:1px solid #f00">x</td></tr><tr><td>y</td></tr></table>"#,
        );
        let b = boxes(&l);
        let (_, y, w, h, fill, border, _) = b
            .iter()
            .find(|b| b.5.is_some())
            .cloned()
            .expect("a border box");
        let border = border.expect("border");
        assert_eq!(fill, None);
        assert_eq!(border.widths, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(border.colors[2], RED);
        assert!(w >= 300.0 - 1.0, "the divider spans the row: {w}");
        let Some(Item::Text { y: ty, .. }) = l
            .items
            .iter()
            .find(|i| matches!(i, Item::Text { text, .. } if text == "y"))
        else {
            panic!("no y")
        };
        assert!(y + h <= *ty + 0.5, "the divider sits above the next row");
    }

    #[test]
    fn row_and_row_group_backgrounds_paint_behind_their_cells() {
        let l = lay(
            r#"<table width="300"><tbody style="background:#00f"><tr style="background:#f00"><td>a</td><td>b</td></tr><tr><td>c</td></tr></tbody></table>"#,
        );
        let fills: Vec<_> = boxes(&l).into_iter().map(|b| b.4).collect();
        assert_eq!(
            fills,
            vec![Some(BLUE), Some(BLUE), Some(BLUE), Some(RED), Some(RED)],
            "group under all three cells, then the row under its two"
        );
    }

    #[test]
    fn an_inline_link_with_a_background_is_a_button() {
        let l = lay(
            r##"<p>Go <a href="#" style="background:#06c;color:#fff;padding:10px 20px;border-radius:4px">Shop now</a></p>"##,
        );
        let b = boxes(&l);
        let (x, y, w, h, fill, _, radius) = b.first().cloned().expect("a box behind the link");
        assert_eq!(fill, Some([0x00, 0x66, 0xcc, 255]));
        assert_eq!(radius, [4.0; 4]);
        let Some(Item::Text {
            x: tx,
            y: ty,
            w: tw,
            line_height,
            text,
            spans,
            ..
        }) = l.items.iter().find(|i| matches!(i, Item::Text { .. }))
        else {
            panic!("no text")
        };
        let shop = spans
            .iter()
            .find(|s| &text[s.range.clone()] == "Shop now")
            .expect("span");
        assert!(
            l.items.iter().position(|i| matches!(i, Item::Box { .. }))
                < l.items.iter().position(|i| matches!(i, Item::Text { .. })),
            "box under the text"
        );
        assert!(
            x < tx + tw - 60.0 && x + w > tx + tw + 19.0,
            "padding grows the box sideways"
        );
        assert!(
            y < *ty - 9.0 && y + h > ty + line_height + 9.0,
            "and up and down"
        );
        assert_eq!(shop.color, [255, 255, 255, 255]);
        // A background wrapping across lines: one box per line.
        let l = layout(
            r#"<p style="width:120px"><span style="background:#ff0">one two three four five six</span></p>"#,
            600.0,
            &Default::default(),
        );
        let n_lines = l
            .items
            .iter()
            .filter(|i| matches!(i, Item::Text { .. }))
            .count();
        assert!(n_lines > 1);
        assert_eq!(boxes(&l).len(), n_lines);
    }

    #[test]
    fn border_sides_keep_their_own_colour_style_and_corners() {
        let l = lay(
            r#"<div style="width:100px;height:20px;border-left:4px solid #00f;border-bottom:2px dashed #f00;border-radius:10px 10px 0 0"></div>"#,
        );
        let (_, _, _, _, _, border, radius) = boxes(&l)[0].clone();
        let border = border.expect("border");
        assert_eq!(border.widths, [0.0, 0.0, 2.0, 4.0]);
        assert_eq!((border.colors[3], border.colors[2]), (BLUE, RED));
        assert_eq!(
            (border.styles[3], border.styles[2]),
            (LineStyle::Solid, LineStyle::Dashed)
        );
        assert_eq!(radius, [10.0, 10.0, 0.0, 0.0]);
        // Corners that would overlap scale down together.
        let l = lay(
            r#"<div style="width:100px;height:20px;background:#eee;border-radius:20px"></div>"#,
        );
        assert_eq!(boxes(&l)[0].6, [10.0; 4]);
    }

    #[test]
    fn gradients_draw_or_fall_back_to_a_flat_colour() {
        let l = lay(
            r#"<div style="width:200px;height:30px;background:linear-gradient(to right,#f00,#00f 80%)"></div>
               <div style="width:200px;height:30px;background:radial-gradient(#ff0000,#0000ff)"></div>"#,
        );
        let grads: Vec<_> = l
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Box { gradient, fill, .. } => Some((gradient.clone(), *fill)),
                _ => None,
            })
            .collect();
        let (Some(g), None) = &grads[0] else {
            panic!("{grads:?}")
        };
        assert!((g.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        assert_eq!(g.stops, vec![(0.0, RED), (0.8, BLUE)]);
        assert_eq!(
            grads[1],
            (None, Some([127, 0, 127, 255])),
            "radial: the average"
        );
    }

    #[test]
    fn background_images_become_items_behind_the_content() {
        let l = lay(
            r#"<div style="height:50px;padding:5px;background:#000 url(https://img.example/hero.jpg) center / cover no-repeat">Hero</div>
               <table background="https://img.example/tile.png"><tr><td>Cell</td></tr></table>"#,
        );
        let bgs: Vec<_> = l
            .items
            .iter()
            .enumerate()
            .filter_map(|(k, i)| match i {
                Item::Background {
                    src,
                    size,
                    position,
                    repeat,
                    h,
                    ..
                } => Some((k, src.as_str(), *size, *position, *repeat, *h)),
                _ => None,
            })
            .collect();
        assert_eq!(bgs.len(), 2, "{:?}", l.items);
        let (k, src, size, position, repeat, h) = bgs[0];
        assert_eq!(src, "https://img.example/hero.jpg");
        assert_eq!(size, BgSize::Cover);
        assert_eq!(position, [(0.5, 0.0), (0.5, 0.0)]);
        assert_eq!(repeat, [false, false]);
        assert_eq!(h, 60.0, "the padding box");
        let text_at = l
            .items
            .iter()
            .position(|i| matches!(i, Item::Text { text, .. } if text == "Hero"));
        assert!(Some(k) < text_at);
        assert!(
            matches!(
                l.items[k - 1],
                Item::Box {
                    fill: Some([0, 0, 0, 255]),
                    ..
                }
            ),
            "over the colour"
        );
        assert_eq!(bgs[1].1, "https://img.example/tile.png");
        assert_eq!(bgs[1].4, [true, true]);
    }

    #[test]
    fn background_tiles_follow_css_sizing() {
        let area = [0.0, 0.0, 600.0, 300.0];
        // cover: scaled to fill, centred.
        let t = background_tile(
            area,
            (1200.0, 400.0),
            BgSize::Cover,
            [(0.5, 0.0), (0.5, 0.0)],
        );
        assert_eq!(t, [-150.0, 0.0, 900.0, 300.0]);
        // `100%` wide, auto height, top.
        let t = background_tile(
            area,
            (1200.0, 400.0),
            BgSize::Explicit(Some(600.0), None),
            [(0.0, 0.0), (0.0, 0.0)],
        );
        assert_eq!(t, [0.0, 0.0, 600.0, 200.0]);
        // Natural size, offset in px.
        let t = background_tile(
            area,
            (10.0, 10.0),
            BgSize::Explicit(None, None),
            [(0.0, 4.0), (1.0, 0.0)],
        );
        assert_eq!(t, [4.0, 290.0, 10.0, 10.0]);
    }
}
