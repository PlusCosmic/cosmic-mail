//! HTML mail as styled text.
//!
//! Rune has no HTML engine, so the reader renders HTML bodies as rich text:
//! `html2text` parses the markup (scripts, styles, forms and other active
//! content never execute — the output is plain text plus annotations) and this
//! module turns its annotated lines into [`Line`]s the reader draws with Rune
//! spans. Layout tables are flattened to one column (`raw_mode`), and lines are
//! left unwrapped so Rune wraps them to the reader's width.
//!
//! Links keep their target, but only `http(s)` targets are ever opened
//! (`desktop::open_url` re-validates). Images keep their `src` so the reader can
//! show inline `data:` images (cid parts the backend already inlined) and,
//! with consent, remote ones.

use std::ops::Range;

use html2text::render::{RichAnnotation, RichDecorator, TextDecorator};

/// Wide enough that html2text never wraps a paragraph itself.
const UNWRAPPED_WIDTH: usize = 100_000;

/// One styled range of a [`Line`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Style {
    Strong,
    Emphasis,
    Code,
    Strike,
    /// A hyperlink to this target (as written in the mail; validate before use).
    Link(String),
}

/// An image referenced at this point of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub src: String,
    pub alt: String,
}

/// One rendered line (a paragraph, list item, quote line, or pre line).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    /// Byte ranges of `text` with their style, in order.
    pub styles: Vec<(Range<usize>, Style)>,
    /// Images on this line (their alt text is not part of `text`).
    pub images: Vec<ImageRef>,
    /// Preformatted (`<pre>`): monospace, whitespace kept.
    pub pre: bool,
}

impl Line {
    /// Whether the line has no visible text and no images.
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty() && self.images.is_empty()
    }

    /// The link targets on this line, in order.
    pub fn links(&self) -> impl Iterator<Item = (&Range<usize>, &str)> {
        self.styles.iter().filter_map(|(r, s)| match s {
            Style::Link(url) => Some((r, url.as_str())),
            _ => None,
        })
    }
}

/// Rewrite `<b>`/`<i>` as `<strong>`/`<em>`, which html2text annotates (it
/// ignores the presentational pair). Other tags (`<br>`, `<body>`,
/// `<blockquote>`, `<img>`, ...) are untouched.
fn normalize_inline_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let tag = &rest[lt..];
        let (closing, name_at) = if tag.as_bytes().get(1) == Some(&b'/') {
            (true, 2)
        } else {
            (false, 1)
        };
        let letter = tag.as_bytes().get(name_at).map(u8::to_ascii_lowercase);
        let after = tag.as_bytes().get(name_at + 1).copied();
        let ends_name = matches!(after, Some(b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'/'));
        let replacement = match (letter, ends_name) {
            (Some(b'b'), true) => Some("strong"),
            (Some(b'i'), true) => Some("em"),
            _ => None,
        };
        match replacement {
            Some(name) => {
                out.push_str(if closing { "</" } else { "<" });
                out.push_str(name);
                rest = &tag[name_at + 1..];
            }
            None => {
                out.push('<');
                rest = &tag[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// End every CSS declaration inside `<style>` with `;`. html2text 0.15's
/// stylesheet parser drops a rule's last declaration when it has no trailing
/// `;` — and minified mail CSS (`.preheader{display:none}`) never has one, so
/// hidden content would show.
fn terminate_css_declarations(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut rest = html;
    while let Some(open) = find_ascii_ci(rest, "<style") {
        let Some(body) = rest[open..].find('>').map(|i| open + i + 1) else {
            break;
        };
        let end = find_ascii_ci(&rest[body..], "</style").map_or(rest.len(), |i| body + i);
        out.push_str(&rest[..body]);
        let mut last = None;
        for c in rest[body..end].chars() {
            if c == '}' && !matches!(last, None | Some(';' | '{' | '}')) {
                out.push(';');
            }
            if !c.is_whitespace() {
                last = Some(c);
            }
            out.push(c);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Byte offset of `needle` (ASCII) in `hay`, ignoring ASCII case.
fn find_ascii_ci(hay: &str, needle: &str) -> Option<usize> {
    hay.as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// html2text's rich decorator, minus the markdown-ish text it injects: `# `
/// before headings and `^{…}` around `<sup>` (Amazon prices are `£34<sup>99</sup>`).
/// The reader draws styles itself, so those markers would show up verbatim.
/// Colours are dropped too: the reader uses the theme's palette, not the mail's.
#[derive(Clone, Debug)]
struct MailDecorator(RichDecorator);

impl TextDecorator for MailDecorator {
    type Annotation = RichAnnotation;

    fn decorate_link_start(&mut self, url: &str) -> (String, RichAnnotation) {
        self.0.decorate_link_start(url)
    }
    fn decorate_link_end(&mut self) -> String {
        self.0.decorate_link_end()
    }
    fn decorate_em_start(&self) -> (String, RichAnnotation) {
        self.0.decorate_em_start()
    }
    fn decorate_em_end(&self) -> String {
        self.0.decorate_em_end()
    }
    fn decorate_strong_start(&self) -> (String, RichAnnotation) {
        self.0.decorate_strong_start()
    }
    fn decorate_strong_end(&self) -> String {
        self.0.decorate_strong_end()
    }
    fn decorate_strikeout_start(&self) -> (String, RichAnnotation) {
        self.0.decorate_strikeout_start()
    }
    fn decorate_strikeout_end(&self) -> String {
        self.0.decorate_strikeout_end()
    }
    fn decorate_code_start(&self) -> (String, RichAnnotation) {
        self.0.decorate_code_start()
    }
    fn decorate_code_end(&self) -> String {
        self.0.decorate_code_end()
    }
    fn decorate_preformat_first(&self) -> RichAnnotation {
        self.0.decorate_preformat_first()
    }
    fn decorate_preformat_cont(&self) -> RichAnnotation {
        self.0.decorate_preformat_cont()
    }
    fn decorate_image(&mut self, src: &str, title: &str) -> (String, RichAnnotation) {
        self.0.decorate_image(src, title)
    }
    fn header_prefix(&self, _level: usize) -> String {
        String::new()
    }
    fn quote_prefix(&self) -> String {
        self.0.quote_prefix()
    }
    fn unordered_item_prefix(&self) -> String {
        self.0.unordered_item_prefix()
    }
    fn ordered_item_prefix(&self, i: i64) -> String {
        self.0.ordered_item_prefix(i)
    }
    fn make_subblock_decorator(&self) -> Self {
        self.clone()
    }
    fn decorate_superscript_start(&self) -> (String, RichAnnotation) {
        (String::new(), RichAnnotation::Default)
    }
    fn decorate_superscript_end(&self) -> String {
        String::new()
    }
}

/// Render an HTML body to lines. Runs of blank lines collapse to one.
///
/// The mail's own CSS is applied (`use_doc_css`) so `display:none` content —
/// preheaders, screen-reader labels, mobile-only duplicates — stays hidden, as
/// it does in any other client. `@media` blocks are ignored, i.e. the desktop
/// layout is what renders.
pub fn render(html: &str) -> Vec<Line> {
    let html = terminate_css_declarations(&normalize_inline_tags(html));
    let lines = match html2text::config::with_decorator(MailDecorator(RichDecorator::new()))
        .use_doc_css()
        .raw_mode(true)
        .no_link_wrapping()
        .allow_width_overflow()
        .lines_from_read(html.as_bytes(), UNWRAPPED_WIDTH)
    {
        Ok(lines) => lines,
        Err(err) => {
            tracing::debug!(error = %err, "html rendering failed; showing nothing");
            return Vec::new();
        }
    };

    let mut out: Vec<Line> = Vec::new();
    for tagged in lines {
        let mut line = Line::default();
        for piece in tagged.tagged_strings() {
            // An image's alt text belongs to the image (the reader shows it as the
            // placeholder for a blocked image), not to the line's text — else a
            // shown image is followed by its alt ("Completed", "logo") as prose.
            if let Some(src) = piece.tag.iter().find_map(|t| match t {
                RichAnnotation::Image(src) => Some(src),
                _ => None,
            }) {
                line.images.push(ImageRef {
                    src: src.clone(),
                    alt: piece.s.trim().to_string(),
                });
                continue;
            }
            let start = line.text.len();
            line.text.push_str(&piece.s);
            let range = start..line.text.len();
            for tag in &piece.tag {
                let style = match tag {
                    RichAnnotation::Strong => Some(Style::Strong),
                    RichAnnotation::Emphasis => Some(Style::Emphasis),
                    RichAnnotation::Code => Some(Style::Code),
                    RichAnnotation::Strikeout => Some(Style::Strike),
                    RichAnnotation::Link(url) => Some(Style::Link(url.clone())),
                    RichAnnotation::Preformat(_) => {
                        line.pre = true;
                        None
                    }
                    _ => None,
                };
                if let Some(style) = style {
                    if !range.is_empty() {
                        push_style(&mut line.styles, range.clone(), style);
                    }
                }
            }
        }
        if !line.pre {
            let trimmed = line.text.trim_end().len();
            line.text.truncate(trimmed);
            for (r, _) in &mut line.styles {
                r.end = r.end.min(trimmed);
                r.start = r.start.min(r.end);
            }
            line.styles.retain(|(r, _)| !r.is_empty());
        }
        let blank = line.is_blank();
        if blank && out.last().is_none_or(Line::is_blank) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(Line::is_blank) {
        out.pop();
    }
    out
}

/// Append a style, merging it into the previous range when contiguous.
fn push_style(styles: &mut Vec<(Range<usize>, Style)>, range: Range<usize>, style: Style) {
    if let Some((last, last_style)) = styles.last_mut() {
        if *last_style == style && last.end == range.start {
            last.end = range.end;
            return;
        }
    }
    styles.push((range, style));
}

/// Decode a `data:image/...;base64,...` URI into its mime type and bytes.
pub fn decode_data_uri(src: &str) -> Option<(String, Vec<u8>)> {
    use base64::Engine;
    let rest = src.trim().strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let mime = meta.split(';').next().unwrap_or("").to_ascii_lowercase();
    if !mime.starts_with("image/") || !meta.contains(";base64") {
        return None;
    }
    let cleaned: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned)
        .ok()?;
    Some((mime, bytes))
}

/// Whether an image `src` is remote (`http(s)`), i.e. a tracking risk that
/// needs consent before loading.
pub fn is_remote(src: &str) -> bool {
    let s = src.trim_start().to_ascii_lowercase();
    s.starts_with("http://") || s.starts_with("https://") || s.starts_with("//")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn paragraphs_become_unwrapped_lines() {
        let long = "word ".repeat(60);
        let lines = render(&format!("<p>{long}</p><p>Second</p>"));
        assert_eq!(texts(&lines), vec![long.trim_end(), "", "Second"]);
    }

    #[test]
    fn scripts_and_styles_never_appear() {
        let lines = render(
            "<style>body{color:red}</style><script>alert('x')</script><p>Hello</p>\
             <form><input value=secret></form>",
        );
        let all = texts(&lines).join("\n");
        assert!(all.contains("Hello"));
        assert!(!all.contains("alert"));
        assert!(!all.contains("color:red"));
    }

    #[test]
    fn keeps_links_and_emphasis_as_styles() {
        let lines = render(r#"<p>Track <a href="https://ups.com/t?1">here</a> <b>now</b></p>"#);
        let line = &lines[0];
        assert_eq!(line.text, "Track here now");
        let links: Vec<_> = line.links().collect();
        assert_eq!(links.len(), 1);
        assert_eq!(&line.text[links[0].0.clone()], "here");
        assert_eq!(links[0].1, "https://ups.com/t?1");
        assert!(line
            .styles
            .iter()
            .any(|(r, s)| *s == Style::Strong && &line.text[r.clone()] == "now"));
    }

    #[test]
    fn presentational_bold_and_italic_are_styled() {
        let lines = render("<p><b>bold</b> <I class=x>it</I> <br><blockquote>q</blockquote></p>");
        let styled: Vec<(&str, &Style)> = lines
            .iter()
            .flat_map(|l| l.styles.iter().map(move |(r, s)| (&l.text[r.clone()], s)))
            .collect();
        assert!(styled.contains(&("bold", &Style::Strong)));
        assert!(styled.contains(&("it", &Style::Emphasis)));
        assert_eq!(
            normalize_inline_tags("<br><body><b>x</b><blockquote><img src=a>"),
            "<br><body><strong>x</strong><blockquote><img src=a>"
        );
    }

    #[test]
    fn layout_tables_flatten_to_one_column() {
        let lines = render("<table><tr><td>Left</td><td>Right</td></tr></table>");
        let all = texts(&lines).join("\n");
        assert!(all.contains("Left") && all.contains("Right"));
        assert!(!all.contains('│') && !all.contains('|'));
    }

    #[test]
    fn no_markdown_markers_for_headings_or_superscripts() {
        let lines = render("<h2>Your package was dispatched!</h2><p>&pound;34<sup>99</sup></p>");
        assert_eq!(
            texts(&lines),
            vec!["Your package was dispatched!", "", "£34⁹⁹"]
        );
    }

    #[test]
    fn hidden_content_stays_hidden() {
        assert_eq!(
            terminate_css_declarations("<STYLE>a{b:c}@media x{d{e:f}}g{h:i;}</style><p>}</p>"),
            "<STYLE>a{b:c;}@media x{d{e:f;}}g{h:i;}</style><p>}</p>"
        );
        let lines = render(
            r#"<style>.pre{display:none} @media (max-width:600px){.desk{display:none}}</style>
               <div class="pre">Preheader text</div>
               <span style="display: none">Completed</span>
               <p class="desk">Ordered</p>"#,
        );
        assert_eq!(texts(&lines), vec!["Ordered"]);
    }

    #[test]
    fn images_keep_their_source() {
        let lines = render(r#"<p><img src="https://t.example/p.gif" alt="logo"></p>"#);
        let images: Vec<_> = lines.iter().flat_map(|l| l.images.clone()).collect();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].src, "https://t.example/p.gif");
        assert_eq!(images[0].alt, "logo");
        assert!(!texts(&lines).join("").contains("logo"));
        assert!(is_remote(&images[0].src));
        assert!(!is_remote("data:image/png;base64,AAAA"));
    }

    #[test]
    fn blank_runs_collapse() {
        let lines = render("<p>a</p><br><br><br><p>b</p>");
        assert!(lines
            .windows(2)
            .all(|w| !(w[0].is_blank() && w[1].is_blank())));
        assert!(!lines.first().unwrap().is_blank());
        assert!(!lines.last().unwrap().is_blank());
    }

    #[test]
    fn decodes_base64_image_data_uris_only() {
        let (mime, bytes) = decode_data_uri("data:image/png;base64,aGVsbG8=").unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, b"hello");
        assert!(decode_data_uri("data:text/html;base64,aGVsbG8=").is_none());
        assert!(decode_data_uri("https://example.com/a.png").is_none());
    }
}
