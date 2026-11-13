//! Anonymous table boxes for table parts the mail's CSS re-displays.
//!
//! Browsers wrap a child of a table, row group or row that is not the table part
//! its parent expects in anonymous table boxes (CSS 2.1 §17.2.1) and show it: a
//! `<td style="display:block">` becomes a block inside an anonymous cell, a
//! `<tbody style="display:table">` a table inside an anonymous row and cell.
//! Mail relies on this constantly (`td.d-block` cards, mobile `@media` rules that
//! stack columns). Blitz's table layout instead skips such children with all
//! their content.
//!
//! The display that matters is the computed one (classes, `!important`,
//! `@media`), so this runs between a style pass and layout: it finds the stray
//! children and wraps them in real elements that the UA sheet ([`CSS`]) makes
//! table parts, which Blitz then lays out like any row or cell.

use blitz_dom::{ns, BaseDocument, LocalName, NodeId, QualName};
use style::values::specified::box_::{DisplayInside, DisplayOutside};

/// Tag names of the wrappers: custom elements, so no author type selector
/// (`td { display:block }`) matches them and re-displays them in turn.
const ROW: &str = "cosmic-anon-row";
const CELL: &str = "cosmic-anon-cell";
const TABLE: &str = "cosmic-anon-table";
/// Attribute that makes a `display:contents` child of a table a row group.
const GROUP: &str = "cosmic-anon-group";

/// UA rules for the wrappers. `!important` in the UA origin outranks every author
/// declaration, so the wrappers stay table parts whatever the mail's CSS says.
pub(super) const CSS: &str = "cosmic-anon-table { display: table !important; }\n\
     [cosmic-anon-group] { display: table-row-group !important; }\n\
     cosmic-anon-row { display: table-row !important; }\n\
     cosmic-anon-cell { display: table-cell !important; padding: 0 !important; \
     border: none !important; background: none !important; }";

/// Where a child sits, for deciding whether its parent table part accepts it.
#[derive(Clone, Copy, PartialEq)]
enum Level {
    /// Child of a table or a row group: rows (and row groups, columns).
    Rows,
    /// Child of a row: cells.
    Cells,
    /// Child of anything else: no rows or cells (they need an anonymous table).
    Outside,
}

/// What one child of a table part is, for wrapping.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// A table part its parent accepts as is.
    Fits,
    /// A cell directly under a table or row group, or outside any table: needs
    /// only an anonymous row (and table).
    Cell,
    /// A row or row group outside any table: needs an anonymous table.
    Row,
    /// Anything else that generates a box: needs an anonymous cell (and row).
    Stray,
    /// Generates no box of its own (`display:none`, out of flow, whitespace,
    /// comments): joins a run of strays it sits inside, never starts one.
    Neutral,
    /// `display:contents`: its children count at this level.
    Contents,
}

/// Wrap every stray child of a table part in anonymous rows/cells. Needs computed
/// styles (`resolve_stylist`); the caller resolves again afterwards, which restyles
/// the moved subtrees.
pub(super) fn wrap_stray_table_parts(doc: &mut BaseDocument) {
    let mut runs: Vec<(Level, Vec<(NodeId, Kind)>)> = Vec::new();
    let mut groups: Vec<NodeId> = Vec::new();
    let mut stack = vec![doc.root_node().id];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get_node(id) else {
            continue;
        };
        stack.extend(node.children.iter().copied());
        let level = match inside(doc, id) {
            Some(DisplayInside::Table)
            | Some(DisplayInside::TableRowGroup)
            | Some(DisplayInside::TableHeaderGroup)
            | Some(DisplayInside::TableFooterGroup) => Level::Rows,
            Some(DisplayInside::TableRow) => Level::Cells,
            Some(DisplayInside::None | DisplayInside::Contents) | None => continue,
            Some(_) => Level::Outside,
        };
        collect_runs(doc, id, level, &mut runs, &mut groups);
    }
    if runs.is_empty() && groups.is_empty() {
        return;
    }
    let mut m = doc.mutate();
    for id in groups {
        m.set_attribute(id, QualName::new(None, ns!(), LocalName::from(GROUP)), "");
    }
    for (level, run) in runs {
        let Some(&(first, _)) = run.first() else {
            continue;
        };
        // The wrapper the run goes into, and which of its members are grouped into
        // a further wrapper inside it (the rest go in as they are).
        let (outer, inner) = match level {
            Level::Cells => (CELL, None),
            Level::Rows => (ROW, Some((Kind::Stray, CELL))),
            Level::Outside => (TABLE, Some((Kind::Cell, ROW))),
        };
        let outer = element(&mut m, outer);
        m.insert_nodes_before(first, &[outer]);
        let (nodes, kinds): (Vec<NodeId>, Vec<Kind>) = run.into_iter().unzip();
        let Some((grouped, inner)) = inner else {
            m.append_children(outer, &nodes);
            continue;
        };
        let mut i = 0;
        while i < nodes.len() {
            if kinds[i] != grouped {
                m.append_children(outer, &nodes[i..=i]);
                i += 1;
                continue;
            }
            let mut end = i + 1;
            while kinds
                .get(end)
                .is_some_and(|&k| k == grouped || k == Kind::Neutral)
            {
                end += 1;
            }
            // Trailing neutral nodes stay outside the group.
            while kinds[end - 1] == Kind::Neutral {
                end -= 1;
            }
            let group = element(&mut m, inner);
            m.append_children(outer, &[group]);
            m.append_children(group, &nodes[i..end]);
            i = end;
        }
    }
}

fn element(m: &mut blitz_dom::DocumentMutator<'_>, tag: &str) -> NodeId {
    m.create_element(QualName::new(None, ns!(html), LocalName::from(tag)), vec![])
}

fn inside(doc: &BaseDocument, id: NodeId) -> Option<DisplayInside> {
    let node = doc.get_node(id)?;
    node.primary_styles().map(|s| s.clone_display().inside())
}

/// Collect the runs of consecutive children of `parent` (looking through
/// `display:contents`) that its table part does not accept. A `display:contents`
/// child of a table or row group goes into `groups`: Blitz's table layout drops
/// it with its content (it checks for `display:none` by the box's outer display,
/// which `contents` shares), so it is laid out as a row group instead — which,
/// like `contents`, draws no box of its own.
fn collect_runs(
    doc: &BaseDocument,
    parent: NodeId,
    level: Level,
    runs: &mut Vec<(Level, Vec<(NodeId, Kind)>)>,
    groups: &mut Vec<NodeId>,
) {
    let Some(node) = doc.get_node(parent) else {
        return;
    };
    let mut run: Vec<(NodeId, Kind)> = Vec::new();
    let flush = |run: &mut Vec<(NodeId, Kind)>, runs: &mut Vec<_>| {
        while run.last().is_some_and(|&(_, k)| k == Kind::Neutral) {
            run.pop();
        }
        if !run.is_empty() {
            runs.push((level, std::mem::take(run)));
        }
    };
    for &child in &node.children {
        match kind(doc, child, level) {
            Kind::Fits => flush(&mut run, runs),
            Kind::Contents => {
                flush(&mut run, runs);
                if level == Level::Rows {
                    groups.push(child);
                }
                collect_runs(doc, child, level, runs, groups);
            }
            Kind::Neutral => {
                if !run.is_empty() {
                    run.push((child, Kind::Neutral));
                }
            }
            k @ (Kind::Cell | Kind::Row | Kind::Stray) => run.push((child, k)),
        }
    }
    flush(&mut run, runs);
}

fn kind(doc: &BaseDocument, id: NodeId, level: Level) -> Kind {
    let Some(node) = doc.get_node(id) else {
        return Kind::Neutral;
    };
    if !node.is_element() {
        let text = node
            .text_data()
            .is_some_and(|t| !t.content.trim().is_empty());
        return match (text, level) {
            (false, _) => Kind::Neutral,
            (true, Level::Outside) => Kind::Fits,
            (true, _) => Kind::Stray,
        };
    }
    let Some(style) = node.primary_styles() else {
        return Kind::Neutral;
    };
    let display = style.clone_display();
    if display.inside() == DisplayInside::Contents {
        return Kind::Contents;
    }
    if display.outside() == DisplayOutside::None
        || style.clone_position().is_absolutely_positioned()
    {
        return Kind::Neutral;
    }
    let row_like = matches!(
        display.inside(),
        DisplayInside::TableRow
            | DisplayInside::TableRowGroup
            | DisplayInside::TableHeaderGroup
            | DisplayInside::TableFooterGroup
    );
    match (level, display.inside()) {
        (Level::Outside, DisplayInside::TableCell) => Kind::Cell,
        (Level::Outside, _) if row_like => Kind::Row,
        (Level::Outside, _) => Kind::Fits,
        (
            Level::Rows,
            DisplayInside::TableRow
            | DisplayInside::TableRowGroup
            | DisplayInside::TableHeaderGroup
            | DisplayInside::TableFooterGroup
            | DisplayInside::TableColumn
            | DisplayInside::TableColumnGroup,
        ) => Kind::Fits,
        (Level::Rows, DisplayInside::TableCell) => Kind::Cell,
        (Level::Cells, DisplayInside::TableCell) => Kind::Fits,
        // Stray columns inside a row: no box in browsers either.
        (Level::Cells, DisplayInside::TableColumn | DisplayInside::TableColumnGroup) => {
            Kind::Neutral
        }
        // Everything else, captions included: Blitz's table layout drops captions
        // too, and in an anonymous row they at least show, across the top of the
        // table where they nearly always sit.
        _ => Kind::Stray,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{layout, Item, MailLayout};

    fn lay(html: &str) -> MailLayout {
        layout(html, 600.0, &Default::default())
    }

    /// Where the text `want` is drawn, if it is.
    fn pos(l: &MailLayout, want: &str) -> Option<(f32, f32)> {
        l.items.iter().find_map(|i| match i {
            Item::Text { x, y, text, .. } if text == want => Some((*x, *y)),
            _ => None,
        })
    }

    #[test]
    fn block_cells_show_stacked_in_one_anonymous_cell() {
        // GitHub's `td.d-block` cards, and mobile rules that stack columns.
        let l = lay(
            r#"<style>.stack{display:block!important;width:100%!important}</style>
               <table width="100%"><tr><td style="display:block">Card</td></tr></table>
               <table width="500"><tr><td class="stack">One</td><td class="stack">Two</td></tr></table>
               <p>After</p>"#,
        );
        let card = pos(&l, "Card").expect("block cell drawn");
        let one = pos(&l, "One").expect("class-driven block cell drawn");
        let two = pos(&l, "Two").expect("second block cell drawn");
        let after = pos(&l, "After").expect("after");
        assert!(card.1 < one.1 && two.1 < after.1);
        assert!((one.0 - two.0).abs() < 1.0, "stacked: {one:?} {two:?}");
        assert!(two.1 > one.1 + 10.0, "stacked: {one:?} {two:?}");
    }

    #[test]
    fn a_block_cell_beside_real_cells_shares_their_row() {
        let l = lay(
            r#"<table width="500"><tr><td>Left</td><td style="display:inline-block">Mid</td><td>Right</td></tr></table>"#,
        );
        let (left, mid, right) = (
            pos(&l, "Left").expect("left"),
            pos(&l, "Mid").expect("mid"),
            pos(&l, "Right").expect("right"),
        );
        assert!(
            left.0 < mid.0 && mid.0 < right.0,
            "{left:?} {mid:?} {right:?}"
        );
        assert!((left.1 - right.1).abs() < 1.0);
    }

    #[test]
    fn table_and_block_row_groups_and_rows_show() {
        // Timberland: `<tbody style="display:table">`; also block rows, and a
        // `@media` rule that matches the 600px layout width.
        let l = lay(
            r#"<style>@media (max-width:800px){.row{display:block}}</style>
               <table width="500"><tbody style="display:table"><tr><td>Inner</td></tr></tbody></table>
               <table width="500"><tr style="display:block"><td>Blockrow</td></tr>
               <tr class="row"><td>Mediarow</td></tr><tr><td>Plain</td></tr></table>"#,
        );
        for want in ["Inner", "Blockrow", "Mediarow", "Plain"] {
            assert!(pos(&l, want).is_some(), "{want} not drawn");
        }
    }

    #[test]
    fn cells_outside_a_table_sit_side_by_side() {
        // A row made a block: its cells get an anonymous table, as in browsers.
        let l = lay(
            r#"<table width="500"><tr style="display:block"><td>X</td><td>Y</td></tr></table>
               <div style="display:table-cell">A</div><div style="display:table-cell">B</div>"#,
        );
        let (x, y) = (pos(&l, "X").expect("x"), pos(&l, "Y").expect("y"));
        assert!((x.1 - y.1).abs() < 1.0 && y.0 > x.0, "{x:?} {y:?}");
        let (a, b) = (pos(&l, "A").expect("a"), pos(&l, "B").expect("b"));
        assert!((a.1 - b.1).abs() < 1.0 && b.0 > a.0, "{a:?} {b:?}");
    }

    #[test]
    fn hidden_table_parts_stay_hidden_and_contents_rows_show() {
        let l = lay(r#"<style>.hide{display:none}</style>
               <table width="500"><tr><td style="display:none">Gone</td><td class="hide">Gone2</td>
               <td>Shown</td></tr><tr style="display:contents"><td>Contents</td></tr></table>"#);
        assert!(pos(&l, "Gone").is_none() && pos(&l, "Gone2").is_none());
        assert!(pos(&l, "Shown").is_some());
        assert!(
            pos(&l, "Contents").is_some(),
            "display:contents row dropped"
        );
    }
}
