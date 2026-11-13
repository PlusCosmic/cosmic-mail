//! Overflow clipping for the display list.
//!
//! A box whose `overflow` is not `visible` on an axis clips its descendants to its
//! padding box on that axis. Mail leans on it: preheaders and footers hidden in
//! `max-height:0;overflow:hidden`, tickers that run off a fixed-width box. The walk
//! clips each such subtree as it leaves it ([`clip_subtree`]): items wholly outside
//! are dropped, and a run that crosses the edge is preceded by an [`Item::Clip`] the
//! reader draws as a clipping element. Clips nest as runs inside runs.
//!
//! Simplifications: every descendant is clipped, including positioned ones whose
//! containing block lies outside the clipper (CSS lets those escape; mail barely
//! positions anything); a rounded clipper clips to one radius (its top-left one).

use blitz_dom::Node;
use style::computed_values::overflow_x::T as Overflow;
use style::values::specified::box_::DisplayInside;

use super::Item;

/// `[x0, y0, x1, y1]`; an axis that does not clip is infinite.
type Rect = [f32; 4];

/// Slack for rounding: an item this far over the edge still counts as inside.
const SLACK: f32 = 0.5;

/// The padding box `node` clips its descendants to, and that clip's corner radius,
/// when its `overflow` clips on either axis. `(x, y)` is its border box's origin;
/// `radius` is the outer corner radius the walk drew its box with.
pub(super) fn clip_of(node: &Node, x: f32, y: f32, radius: f32) -> Option<(Rect, f32)> {
    let styles = node.primary_styles()?;
    let display = styles.clone_display();
    // `overflow` applies to block containers, flex and grid containers, tables and cells —
    // not to inline boxes, rows or row groups (as Chrome draws them).
    if display.is_inline_flow()
        || matches!(
            display.inside(),
            DisplayInside::None
                | DisplayInside::Contents
                | DisplayInside::TableRow
                | DisplayInside::TableRowGroup
                | DisplayInside::TableHeaderGroup
                | DisplayInside::TableFooterGroup
                | DisplayInside::TableColumn
                | DisplayInside::TableColumnGroup
        )
    {
        return None;
    }
    let clips = |o: Overflow| o != Overflow::Visible;
    // A browser grows tables and cells to fit their content, so they never cut it off
    // vertically; Blitz keeps a cell's `height`, and clipping to that would hide rows of text.
    let grows = matches!(
        display.inside(),
        DisplayInside::Table | DisplayInside::TableCell
    );
    let (cx, cy) = (
        clips(styles.clone_overflow_x()),
        clips(styles.clone_overflow_y()) && !grows,
    );
    if !cx && !cy {
        return None;
    }
    let l = node.final_layout();
    let (w, h) = (l.size.width, l.size.height);
    let b = l.border;
    let inf = f32::INFINITY;
    let rect = [
        if cx { x + b.left } else { -inf },
        if cy { y + b.top } else { -inf },
        if cx { x + w - b.right } else { inf },
        if cy { y + h - b.bottom } else { inf },
    ];
    // The padding box's corners are the border box's, inset by the border.
    let inner = if cx && cy {
        (radius - b.top.max(b.left)).max(0.0)
    } else {
        0.0
    };
    Some((rect, inner))
}

/// Clip the items the walk emitted for a clipper's subtree (`out[start..]`) to `clip`.
pub(super) fn clip_subtree(out: &mut Vec<Item>, start: usize, clip: Rect, radius: f32) {
    if start >= out.len() {
        return;
    }
    let run: Vec<Item> = out.drain(start..).collect();
    let (kept, crosses) = clip_run(run, clip, radius);
    if crosses && !kept.is_empty() {
        // An axis that does not clip spans what the run draws on it.
        let mut r = clip;
        if !r[0].is_finite() || !r[1].is_finite() {
            let ext = kept.iter().filter_map(bounds).fold(
                [
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                ],
                |a, b| {
                    [
                        a[0].min(b[0]),
                        a[1].min(b[1]),
                        a[2].max(b[2]),
                        a[3].max(b[3]),
                    ]
                },
            );
            for axis in 0..2 {
                if !r[axis].is_finite() {
                    r[axis] = ext[axis];
                    r[axis + 2] = ext[axis + 2];
                }
            }
        }
        if r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1] {
            out.push(Item::Clip {
                x: r[0],
                y: r[1],
                w: r[2] - r[0],
                h: r[3] - r[1],
                radius,
                len: kept.len(),
            });
        }
    }
    out.extend(kept);
}

/// Drop what lies wholly outside `clip`; say whether anything kept crosses its edge.
/// Nested clip runs are clipped to the intersection.
fn clip_run(items: Vec<Item>, clip: Rect, radius: f32) -> (Vec<Item>, bool) {
    let mut kept = Vec::with_capacity(items.len());
    let mut crosses = false;
    let mut it = items.into_iter();
    while let Some(item) = it.next() {
        if let Item::Clip {
            x,
            y,
            w,
            h,
            radius: r,
            len,
        } = item
        {
            let inner: Vec<Item> = it.by_ref().take(len).collect();
            let own = [x, y, x + w, y + h];
            let meet = [
                own[0].max(clip[0]),
                own[1].max(clip[1]),
                own[2].min(clip[2]),
                own[3].min(clip[3]),
            ];
            if meet[2] - meet[0] <= 0.0 || meet[3] - meet[1] <= 0.0 {
                continue;
            }
            let (inner, _) = clip_run(inner, meet, 0.0);
            if inner.is_empty() {
                continue;
            }
            crosses |= !inside(own, clip, radius);
            kept.push(Item::Clip {
                x: meet[0],
                y: meet[1],
                w: meet[2] - meet[0],
                h: meet[3] - meet[1],
                radius: r,
                len: inner.len(),
            });
            kept.extend(inner);
            continue;
        }
        let Some(b) = bounds(&item) else {
            kept.push(item);
            continue;
        };
        let ix = b[2].min(clip[2]) - b[0].max(clip[0]);
        let iy = b[3].min(clip[3]) - b[1].max(clip[1]);
        // Zero-size items (a 0-wide box edge) are kept if they lie within the clip.
        let wholly_outside =
            (ix < 0.0 || iy < 0.0) || (ix == 0.0 && b[2] > b[0]) || (iy == 0.0 && b[3] > b[1]);
        if wholly_outside {
            continue;
        }
        crosses |= !inside(b, clip, radius);
        kept.push(item);
    }
    (kept, crosses)
}

/// Whether `b` lies within `clip` (with slack), clear of its rounded corners.
fn inside(b: Rect, clip: Rect, radius: f32) -> bool {
    let within = b[0] >= clip[0] - SLACK
        && b[1] >= clip[1] - SLACK
        && b[2] <= clip[2] + SLACK
        && b[3] <= clip[3] + SLACK;
    if !within || radius <= SLACK {
        return within;
    }
    // In a corner's square: the rounding may cut it.
    let near_x = b[0] < clip[0] + radius || b[2] > clip[2] - radius;
    let near_y = b[1] < clip[1] + radius || b[3] > clip[3] - radius;
    !(near_x && near_y)
}

/// An item's extent; `None` for a clip marker.
fn bounds(item: &Item) -> Option<Rect> {
    match item {
        Item::Box { x, y, w, h, .. }
        | Item::Image { x, y, w, h, .. }
        | Item::Background { x, y, w, h, .. } => Some([*x, *y, x + w, y + h]),
        Item::Text {
            x,
            y,
            w,
            line_height,
            ..
        } => Some([*x, *y, x + w, y + line_height]),
        Item::Clip { .. } => None,
    }
}

/// The right and bottom edges of what the list visibly draws: each item cut to the
/// clips it is drawn in.
pub(super) fn visible_extent(items: &[Item]) -> (f32, f32) {
    let mut right = f32::NEG_INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    // Open clips: their rect and how many items they still cover.
    let mut open: Vec<(Rect, usize)> = Vec::new();
    for item in items {
        let clip = open.last().map(|c| c.0);
        for c in &mut open {
            c.1 = c.1.saturating_sub(1);
        }
        match item {
            Item::Clip {
                x, y, w, h, len, ..
            } => {
                let own = [*x, *y, x + w, y + h];
                let r = match clip {
                    Some(c) => [
                        own[0].max(c[0]),
                        own[1].max(c[1]),
                        own[2].min(c[2]),
                        own[3].min(c[3]),
                    ],
                    None => own,
                };
                open.push((r, *len));
            }
            _ => {
                if let Some(b) = bounds(item) {
                    let (r, btm) = match clip {
                        Some(c) => (b[2].min(c[2]), b[3].min(c[3])),
                        None => (b[2], b[3]),
                    };
                    right = right.max(r);
                    bottom = bottom.max(btm);
                }
            }
        }
        while open.last().is_some_and(|c| c.1 == 0) {
            open.pop();
        }
    }
    (right, bottom)
}

#[cfg(test)]
mod tests {
    use super::super::{layout, Item, MailLayout};

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

    #[test]
    fn content_hidden_by_overflow_or_styling_never_shows() {
        let l = lay(r#"<div style="max-height:0;overflow:hidden">Preheader</div>
               <div style="height:0;overflow:hidden">Zero high</div>
               <div style="width:0;overflow:hidden">Zero wide</div>
               <div style="height:20px;overflow:hidden"><div style="height:20px">Kept</div><p>Pushed out</p></div>
               <p style="text-indent:-9999px">Off canvas</p>
               <div style="font-size:0">Sized nil</div>
               <p style="color:transparent">Clear</p>
               <ul><li style="overflow:hidden">Unbulleted</li></ul>
               <p>Shown</p>"#);
        assert_eq!(texts(&l), vec!["Kept", "Unbulleted", "Shown"]);
        assert!(
            !l.items.iter().any(|i| matches!(i, Item::Clip { .. })),
            "nothing crosses an edge, so nothing needs a clip"
        );
    }

    #[test]
    fn a_clipped_line_is_cut_at_its_box_and_does_not_widen_the_page() {
        // A `nowrap` ticker in a narrow `overflow:hidden` box: drawn cut at the box, and the
        // page is not scaled down to fit the whole line.
        let ticker = "Free delivery on everything ".repeat(20);
        let l = lay(&format!(
            r#"<div style="width:100px;overflow:hidden;white-space:nowrap">{ticker}</div><p>After</p>"#
        ));
        assert_eq!(l.content_width, 600.0);
        let i = l
            .items
            .iter()
            .position(|i| matches!(i, Item::Text { text, .. } if text.starts_with("Free")))
            .expect("the ticker still draws");
        assert!(
            matches!(l.items[i - 1], Item::Clip { w, len: 1, .. } if (w - 100.0).abs() < 0.5),
            "{:?}",
            l.items[i - 1]
        );
        let Item::Text { w, .. } = &l.items[i] else {
            unreachable!()
        };
        assert!(
            *w > 600.0,
            "the line keeps its laid-out width; the clip cuts it"
        );
    }

    #[test]
    fn cells_grow_past_their_height_instead_of_clipping() {
        // A browser grows a cell to fit its content: `overflow:hidden` never cuts it off.
        let l = lay(
            r#"<table><tr><td style="height:20px;overflow:hidden">One<br>Two<br>Three</td></tr></table>"#,
        );
        assert_eq!(texts(&l), vec!["One", "Two", "Three"]);
    }

    #[test]
    fn nested_clips_intersect() {
        let l = lay(
            r#"<div style="width:300px;overflow:hidden"><div style="width:600px;height:20px;overflow:hidden;white-space:nowrap">
               <img src="cid:a" width="900" height="10"></div></div>"#,
        );
        let clips: Vec<f32> = l
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Clip { w, .. } => Some(*w),
                _ => None,
            })
            .collect();
        assert_eq!(clips.len(), 2, "{:?}", l.items);
        assert!(clips.iter().all(|w| *w <= 300.5), "{clips:?}");
        assert_eq!(l.content_width, 600.0);
    }
}
