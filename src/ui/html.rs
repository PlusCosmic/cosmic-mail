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

use html2text::render::RichAnnotation;

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
    /// Images on this line (their alt text is part of `text`).
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

/// Render an HTML body to lines. Runs of blank lines collapse to one.
pub fn render(html: &str) -> Vec<Line> {
    let html = normalize_inline_tags(html);
    let lines = match html2text::config::rich()
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
                    RichAnnotation::Image(src) => {
                        line.images.push(ImageRef {
                            src: src.clone(),
                            alt: piece.s.trim().to_string(),
                        });
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
    fn images_keep_their_source() {
        let lines = render(r#"<p><img src="https://t.example/p.gif" alt="logo"></p>"#);
        let images: Vec<_> = lines.iter().flat_map(|l| l.images.clone()).collect();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].src, "https://t.example/p.gif");
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
