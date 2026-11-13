//! HTML's legacy presentational attributes (`cellpadding`, `valign`, `<font color>`…),
//! which mail is written in and Blitz mostly ignores, as user-agent stylesheets.
//!
//! Blitz maps only a few attributes to style itself (`synthesize_presentational_hints_…`
//! in `blitz-dom/src/stylo.rs`: lowercase `align` to `text-align`, `width`/`height`,
//! `hspace`/`vspace`, `<img border>`, `#rgb`/`#rrggbb` `bgcolor`, `hidden`). The rest follow
//! the HTML Standard's "Rendering" section here. They are rules at user-agent origin, so
//! every author rule and inline style beats them, as it beats a browser's presentational
//! hints; rewriting `style` attributes would invert that. Each selector names its attribute
//! and so outranks Blitz's default sheet (`td`, `table`), and the sheets come after it.
//!
//! Keyword attributes (`valign=top`) are one fixed sheet. Valued ones (`cellpadding="8"`)
//! get a rule per distinct value present in the mail: the value is parsed here, by the
//! HTML microsyntax the spec names, and only the parsed number or colour reaches the
//! declaration; the raw value appears only as an escaped CSS string in the selector, so
//! no attribute can inject CSS.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use blitz_dom::{ns, BaseDocument, LocalName, NodeId, QualName, UserAgentStylesheet};
use style::servo::attr::{parse_legacy_color, parse_unsigned_integer};

/// Add the stylesheets for `doc`'s presentational attributes, before styles resolve.
pub(super) fn apply(doc: &mut BaseDocument) {
    static FIXED: LazyLock<UserAgentStylesheet> =
        LazyLock::new(|| UserAgentStylesheet::parse(FIXED_CSS));
    doc.add_user_agent_stylesheet(FIXED.clone());
    mark_cells(doc);
    let valued = valued_rules(doc);
    if !valued.is_empty() {
        doc.add_user_agent_stylesheet(UserAgentStylesheet::parse(&valued));
    }
}

/// Keyword-valued attributes, the same for every mail. Matched ASCII case-insensitively,
/// as HTML matches these keywords. Blitz's own `align` mapping takes only lowercase
/// `left`/`right`/`center` (and applies to every element); these cover the rest.
const FIXED_CSS: &str = "
:is(thead, tbody, tfoot, tr, td, th)[valign=top i] { vertical-align: top; }
:is(thead, tbody, tfoot, tr, td, th)[valign=middle i] { vertical-align: middle; }
:is(thead, tbody, tfoot, tr, td, th)[valign=bottom i] { vertical-align: bottom; }
:is(thead, tbody, tfoot, tr, td, th)[valign=baseline i] { vertical-align: baseline; }

:is(div, caption, thead, tbody, tfoot, tr, td, th)[align=left i] { text-align: -moz-left; }
:is(div, caption, thead, tbody, tfoot, tr, td, th)[align=right i] { text-align: -moz-right; }
:is(div, caption, thead, tbody, tfoot, tr, td, th):is([align=center i], [align=middle i]) {
    text-align: -moz-center;
}
:is(div, caption, thead, tbody, tfoot, tr, td, th, p, h1, h2, h3, h4, h5, h6)[align=justify i] {
    text-align: justify;
}
:is(p, h1, h2, h3, h4, h5, h6)[align=left i] { text-align: left; }
:is(p, h1, h2, h3, h4, h5, h6)[align=right i] { text-align: right; }
:is(p, h1, h2, h3, h4, h5, h6)[align=center i] { text-align: center; }

table[align=left i] { float: left; }
table[align=right i] { float: right; }
table[align=center i] { margin-left: auto; margin-right: auto; }

img[align=left i] { float: left; }
img[align=right i] { float: right; }
img[align=top i] { vertical-align: top; }
img[align=texttop i] { vertical-align: text-top; }
img:is([align=middle i], [align=absmiddle i], [align=abscenter i], [align=center i]) {
    vertical-align: middle;
}
img:is([align=bottom i], [align=baseline i]) { vertical-align: baseline; }
img[align=absbottom i] { vertical-align: bottom; }
";

/// One rule per distinct (element, attribute, value) the mail uses, for the attributes
/// whose value carries a number or colour.
fn valued_rules(doc: &BaseDocument) -> String {
    let mut rules = BTreeSet::new();
    let mut stack = vec![doc.root_node().id];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get_node(id) else {
            continue;
        };
        stack.extend(node.children.iter().copied());
        let Some(el) = node.element_data() else {
            continue;
        };
        for attr in el.attrs() {
            if let Some(rule) = rule_for(&el.name.local, &attr.name.local, &attr.value) {
                rules.insert(rule);
            }
        }
    }
    rules.into_iter().collect::<Vec<_>>().join("\n")
}

/// The rule `<tag attr=value>` maps to, if any.
fn rule_for(tag: &str, attr: &str, value: &str) -> Option<String> {
    let sel = |t: &str| format!("{t}[{attr}={}]", css_string(value));
    match (tag, attr) {
        ("table", "cellspacing") => {
            let n = pixels(value)?;
            Some(format!("{} {{ border-spacing: {n}px; }}", sel("table")))
        }
        ("table", "cellpadding") => {
            let n = pixels(value)?;
            Some(format!("{} {{ padding: {n}px; }}", cells_of(attr, value)))
        }
        // A width on the table, 1px on its cells; Chrome draws them in the text colour
        // (`bordercolor` overrides it, by colour only, so it never fights this rule).
        ("table", "border") => {
            let n = parse_unsigned_integer(value.chars()).unwrap_or(1);
            (n > 0).then(|| {
                format!(
                    "{} {{ border-width: {n}px; border-style: outset; }}\n\
                     {} {{ border-width: 1px; border-style: inset; }}",
                    sel("table"),
                    cells_of(attr, value)
                )
            })
        }
        ("table", "bordercolor") => {
            let c = color(value)?;
            let t = sel("table");
            Some(format!(
                "{t}, {} {{ border-color: {c}; }}",
                cells_of(attr, value)
            ))
        }
        ("body" | "table" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th", "bgcolor") => Some(
            format!("{} {{ background-color: {}; }}", sel(tag), color(value)?),
        ),
        // `background="url"` (body, table, td, th) belongs here as `background-image`
        // once the display list draws background images.
        ("font", "color") | ("body", "text") => {
            Some(format!("{} {{ color: {}; }}", sel(tag), color(value)?))
        }
        ("font", "size") => Some(format!(
            "{} {{ font-size: {}; }}",
            sel("font"),
            legacy_font_size(value)?
        )),
        // `:link`; Blitz never marks a link visited or active, so `vlink`/`alink` never apply.
        ("body", "link") => Some(format!(
            "{} a[href] {{ color: {}; }}",
            sel("body"),
            color(value)?
        )),
        _ => None,
    }
}

/// The table attributes that style the table's cells.
const CELL_ATTRS: [&str; 3] = ["cellpadding", "border", "bordercolor"];

/// The cells that belong to a table with `attr=value`, not to a table nested in them:
/// the ones [`mark_cells`] gave a copy of the attribute.
fn cells_of(attr: &str, value: &str) -> String {
    format!(":is(td, th)[cosmic-table-{attr}={}]", css_string(value))
}

/// Copy each table's cell-styling attributes onto its own cells as `cosmic-table-…`.
/// A structural selector (`table > tbody > tr > td`) would say the same, but only until
/// `tables` wraps a re-displayed cell or row group in an anonymous element; the copy
/// stays with the cell wherever it is moved.
fn mark_cells(doc: &mut BaseDocument) {
    let mut marks: Vec<(NodeId, String, String)> = Vec::new();
    let mut stack = vec![doc.root_node().id];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get_node(id) else {
            continue;
        };
        stack.extend(node.children.iter().copied());
        let Some(el) = node.element_data().filter(|el| &*el.name.local == "table") else {
            continue;
        };
        let attrs: Vec<(String, String)> = el
            .attrs()
            .iter()
            .filter(|a| CELL_ATTRS.contains(&&*a.name.local))
            .map(|a| {
                (
                    format!("cosmic-table-{}", a.name.local),
                    a.value.to_string(),
                )
            })
            .collect();
        if attrs.is_empty() {
            continue;
        }
        for cell in own_cells(doc, id) {
            for (name, value) in &attrs {
                marks.push((cell, name.clone(), value.clone()));
            }
        }
    }
    let mut m = doc.mutate();
    for (id, name, value) in marks {
        m.set_attribute(
            id,
            QualName::new(None, ns!(), LocalName::from(name)),
            &value,
        );
    }
}

/// The `<td>`/`<th>`s in `table`'s own rows, directly in it or in its row groups.
fn own_cells(doc: &BaseDocument, table: NodeId) -> Vec<NodeId> {
    let is = |id: NodeId, names: &[&str]| {
        doc.get_node(id)
            .and_then(|n| n.element_data())
            .is_some_and(|el| names.contains(&&*el.name.local))
    };
    let children = |id: NodeId| {
        doc.get_node(id)
            .map(|n| n.children.clone())
            .unwrap_or_default()
    };
    let mut rows = Vec::new();
    for child in children(table) {
        if is(child, &["tr"]) {
            rows.push(child);
        } else if is(child, &["thead", "tbody", "tfoot"]) {
            rows.extend(children(child).into_iter().filter(|r| is(*r, &["tr"])));
        }
    }
    rows.into_iter()
        .flat_map(children)
        .filter(|c| is(*c, &["td", "th"]))
        .collect()
}

/// The HTML "maps to the pixel length property" mapping: a non-negative integer.
fn pixels(value: &str) -> Option<u32> {
    parse_unsigned_integer(value.chars())
        .ok()
        .filter(|n| *n <= 10_000)
}

/// The HTML "rules for parsing a legacy colour value", as `#rrggbb`: named colours, hex
/// with or without `#`, and junk read as hex the way browsers do.
fn color(value: &str) -> Option<String> {
    // Stylo's parser indexes the first byte after trimming; an all-space value is empty.
    if value
        .trim_matches(|c: char| c.is_ascii_whitespace())
        .is_empty()
    {
        return None;
    }
    let c = parse_legacy_color(value).ok()?.into_srgb_legacy();
    let [r, g, b, _] = c
        .raw_components()
        .map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8);
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

/// The HTML "rules for parsing a legacy font size", as the keyword `<font size>` maps to:
/// `1`–`7`, or `+N`/`-N` relative to 3, clamped.
fn legacy_font_size(value: &str) -> Option<&'static str> {
    let v = value.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let (sign, digits) = match v.as_bytes().first()? {
        b'+' => (1, &v[1..]),
        b'-' => (-1, &v[1..]),
        _ => (0, v),
    };
    let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let n: i64 = digits.parse().unwrap_or(i64::MAX).min(100);
    let size = match sign {
        0 => n,
        s => 3 + s * n,
    };
    Some(match size.clamp(1, 7) {
        1 => "x-small",
        2 => "small",
        3 => "medium",
        4 => "large",
        5 => "x-large",
        6 => "xx-large",
        _ => "xxx-large",
    })
}

/// `value` as a quoted CSS string: everything but ASCII alphanumerics escaped as hex, so
/// a hostile attribute value cannot end the string or the rule.
fn css_string(value: &str) -> String {
    let mut s = String::with_capacity(value.len() + 2);
    s.push('"');
    for c in value.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else {
            s.push_str(&format!("\\{:x} ", c as u32));
        }
    }
    s.push('"');
    s
}

#[cfg(test)]
mod tests {
    use super::super::{layout, Item, MailLayout};
    use super::*;

    fn lay(html: &str) -> MailLayout {
        layout(html, 600.0, &Default::default())
    }

    /// The (x, y, width) of the text item `want`.
    fn at(l: &MailLayout, want: &str) -> (f32, f32, f32) {
        l.items
            .iter()
            .find_map(|i| match i {
                Item::Text { x, y, w, text, .. } if text.trim() == want => Some((*x, *y, *w)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no text {want:?} in {:?}", l.items))
    }

    fn color_of(l: &MailLayout, want: &str) -> [u8; 4] {
        l.items
            .iter()
            .find_map(|i| match i {
                Item::Text { text, spans, .. } if text.trim() == want => Some(spans[0].color),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no text {want:?}"))
    }

    #[test]
    fn cellpadding_stays_with_a_cell_the_table_pass_wraps() {
        // `tables` moves a `display:block` cell into an anonymous cell, so it is no
        // longer its row's child; it is still the table's cell.
        let l = lay(
            r#"<body style="margin:0"><table cellpadding="10" cellspacing="0" width="300">
               <tr><td style="display:block">Block</td></tr></table>"#,
        );
        let (x, y, _) = at(&l, "Block");
        assert_eq!(x, 10.0);
        assert!((y - 10.0).abs() < 1.0, "{y}");
    }

    #[test]
    fn cellpadding_and_cellspacing_reach_only_the_tables_own_cells() {
        let l = lay(
            r#"<body style="margin:0"><table cellpadding="10" cellspacing="0"><tr valign="top"><td>A</td>
               <td><table><tr><td>Inner</td></tr></table></td></tr></table>"#,
        );
        let (ax, ay, aw) = at(&l, "A");
        assert_eq!(ax, 10.0, "padding 10, no spacing");
        assert!((ay - 10.0).abs() < 1.0, "{ay}");
        // 20px of padding (A's right, the next cell's left) after A, then the nested
        // table's defaults: 2px spacing and 1px padding.
        let (ix, ..) = at(&l, "Inner");
        assert!((ix - (ax + aw + 20.0 + 3.0)).abs() < 1.0, "{ix}");
    }

    #[test]
    fn author_css_beats_the_attributes() {
        let l = lay(r#"<style>td{padding:0}</style><body style="margin:0">
               <table cellpadding="10" cellspacing="0"><tr><td>A</td></tr></table>
               <font color="red" style="color:#00f">Blue</font>"#);
        assert_eq!(at(&l, "A").0, 0.0);
        assert_eq!(color_of(&l, "Blue"), [0, 0, 255, 255]);
    }

    #[test]
    fn valign_on_a_row_reaches_its_cells() {
        let l = lay(
            r#"<table><tr valign="TOP"><td>Top</td><td>One<br>Two<br>Three</td>
               <td valign="bottom">Bottom</td></tr></table>"#,
        );
        assert_eq!(at(&l, "Top").1, at(&l, "One").1);
        assert_eq!(at(&l, "Bottom").1, at(&l, "Three").1);
    }

    #[test]
    fn a_centred_table_is_centred_and_its_text_is_not() {
        let l = lay(
            r#"<body style="margin:0"><table align="center" width="200" cellpadding="0"
               cellspacing="0"><tr><td>Left</td></tr></table>"#,
        );
        assert_eq!(at(&l, "Left").0, 200.0);
    }

    #[test]
    fn font_and_body_colours_and_sizes_apply() {
        let l = lay(r#"<body text="336699" bgcolor="lightblue"><p>Body</p>
               <font color="ff0000" size="+2">Big</font></body>"#);
        assert_eq!(color_of(&l, "Body"), [0x33, 0x66, 0x99, 255]);
        assert_eq!(color_of(&l, "Big"), [255, 0, 0, 255]);
        let size = l.items.iter().find_map(|i| match i {
            Item::Text { text, size, .. } if text == "Big" => Some(*size),
            _ => None,
        });
        assert_eq!(size, Some(24.0), "size=+2 is x-large");
        assert_eq!(l.background, Some([0xad, 0xd8, 0xe6, 255]));
    }

    #[test]
    fn table_border_draws_on_the_table_and_its_cells() {
        let l = lay(r#"<table border="2" bordercolor="green"><tr><td>x</td></tr></table>"#);
        let borders: Vec<_> = l
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Box {
                    border: Some(b), ..
                } => Some((b.widths[0], b.colors[0])),
                _ => None,
            })
            .collect();
        assert_eq!(
            borders,
            vec![(2.0, [0, 128, 0, 255]), (1.0, [0, 128, 0, 255])]
        );
    }

    #[test]
    fn an_image_aligned_left_floats_beside_the_text() {
        let src = "https://mail.invalid/a.png";
        let sizes = [(src.to_string(), (50, 50))].into_iter().collect();
        let l = layout(
            &format!(r#"<body style="margin:0"><img src="{src}" align="left">Beside</body>"#),
            600.0,
            &sizes,
        );
        let (x, y, _) = at(&l, "Beside");
        assert_eq!(x, 50.0);
        assert!(y < 1.0, "beside the image, not below it: {y}");
    }

    #[test]
    fn legacy_colours_parse_like_a_browser() {
        assert_eq!(color("ffcc00").as_deref(), Some("#ffcc00"));
        assert_eq!(color("#FC0").as_deref(), Some("#ffcc00"));
        assert_eq!(color(" Red ").as_deref(), Some("#ff0000"));
        assert_eq!(color("chucknorris").as_deref(), Some("#c00000"));
        assert_eq!(color("transparent"), None);
        assert_eq!(color("  "), None);
        assert_eq!(color(""), None);
    }

    #[test]
    fn legacy_font_sizes_are_relative_to_three_and_clamped() {
        assert_eq!(legacy_font_size("2"), Some("small"));
        assert_eq!(legacy_font_size("+2"), Some("x-large"));
        assert_eq!(legacy_font_size("-1"), Some("small"));
        assert_eq!(legacy_font_size(" 9px"), Some("xxx-large"));
        assert_eq!(legacy_font_size("-99999999999999999999"), Some("x-small"));
        assert_eq!(legacy_font_size("big"), None);
    }

    #[test]
    fn attribute_values_cannot_inject_css() {
        let rule = rule_for("font", "size", "4\"]{} *{display:none} x[a=\"").unwrap();
        assert_eq!(rule.matches('{').count(), 1, "{rule}");
        assert_eq!(rule.matches('"').count(), 2, "{rule}");
        assert!(rule.ends_with("{ font-size: large; }"), "{rule}");
    }
}
