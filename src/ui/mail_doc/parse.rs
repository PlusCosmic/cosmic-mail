//! Mail into a Blitz document: always as HTML, and defused of the inputs that panic Blitz.
//!
//! `HtmlDocument::from_html` sniffs a document that starts with `<?xml`, or with an XHTML
//! doctype, or whose root declares the XHTML namespace, and hands it to an XML parser. Mail
//! with an XHTML doctype is tag soup like any other: as XML it loses the HTML tree builder
//! (implied `<tbody>`, void elements, entities), drops content and draws `<style>` text. So
//! the mail goes through html5ever here with Blitz's own options and sink, never the sniff.
//!
//! The mail's own doctype sets the document's mode (no-quirks, limited-quirks or quirks), as in
//! a browser: Blitz's sink hands the mode the tree builder chooses to the document, which lays
//! out and styles in it (GOTCHAS, "Blitz"). Nothing here changes it: `drop_doctype` only keeps
//! the doctype node out of the tree, a BOM, `<?xml …?>` or a comment before the doctype are
//! skipped as browsers skip them, and the `@import` rename happens after parsing.
//!
//! The sink is wrapped to keep three things from reaching Blitz's unwraps (each panics the
//! layout, and the reader falls back to flattened text):
//! - an `<img src>` or `<link href>` that is not a URL even against the base (`http://`, an
//!   unfilled `http://*|IMG|*` merge tag): `BaseDocument::resolve_url` panics. Blanked: no
//!   URL means nothing to load, which is what the reader would get anyway.
//! - an `@import` whose URL is empty or invalid (`@import url()`): `net.rs` unwraps it. Every
//!   `@import` is renamed to an unknown at-rule, which Stylo drops. Nothing is ever fetched
//!   (the net provider is Blitz's no-op), so an import never contributed rules anyway.

use std::borrow::Cow;
use std::cell::RefCell;

use blitz_dom::{local_name, BaseDocument, DocumentConfig, NodeId};
use blitz_html::DocumentHtmlParser;
use html5ever::interface::{ElementFlags, NodeOrText, QualName, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tokenizer::TokenizerOpts;
use html5ever::tree_builder::TreeBuilderOpts;
use html5ever::{Attribute, ParseOpts};

/// Parse `html` as HTML into a document configured by `config` (whose `base_url` the
/// `src`/`href` check resolves against).
pub(super) fn document(html: &str, config: DocumentConfig) -> BaseDocument {
    let base = config
        .base_url
        .as_deref()
        .and_then(|u| url::Url::parse(u).ok());
    let mut doc = BaseDocument::new(config);
    let mut mutr = doc.mutate();
    let sink = MailSink {
        inner: DocumentHtmlParser::new(&mut mutr),
        base,
        styles: RefCell::default(),
    };
    // The options blitz-html parses HTML with. `read_from` on a byte slice cannot fail.
    let opts = ParseOpts {
        tokenizer: TokenizerOpts::default(),
        tree_builder: TreeBuilderOpts {
            exact_errors: false,
            scripting_enabled: false,
            iframe_srcdoc: false,
            drop_doctype: true,
            quirks_mode: QuirksMode::NoQuirks,
        },
    };
    let styles = html5ever::parse_document(sink, opts)
        .from_utf8()
        .read_from(&mut html.as_bytes())
        .unwrap_or_default();
    // `<style>` text is parsed when the mutator flushes (on drop), so it can still change.
    for id in styles {
        let Some(css) = mutr.doc.get_node(id).map(|n| n.text_content()) else {
            continue;
        };
        if let Some(css) = defuse_imports(&css) {
            mutr.remove_and_drop_all_children(id);
            let text = mutr.create_text_node(&css);
            mutr.append_children(id, &[text]);
        }
    }
    drop(mutr);
    doc
}

/// Blitz's HTML sink, with `src`/`href` checked as elements are created (Blitz starts
/// loading an `<img>` or `<link>` the moment it is inserted) and `<style>`s collected.
struct MailSink<'m, 'd> {
    inner: DocumentHtmlParser<'m, 'd>,
    base: Option<url::Url>,
    styles: RefCell<Vec<NodeId>>,
}

impl MailSink<'_, '_> {
    /// Whether Blitz can resolve `raw` (it panics on what it cannot).
    fn resolves(&self, raw: &str) -> bool {
        match &self.base {
            Some(base) => base.join(raw).is_ok(),
            None => url::Url::parse(raw).is_ok(),
        }
    }
}

impl<'m, 'd> TreeSink for MailSink<'m, 'd> {
    type Handle = NodeId;
    type Output = Vec<NodeId>;
    type ElemName<'a>
        = <DocumentHtmlParser<'m, 'd> as TreeSink>::ElemName<'a>
    where
        Self: 'a;

    fn finish(self) -> Vec<NodeId> {
        self.inner.finish();
        self.styles.into_inner()
    }

    fn create_element(
        &self,
        name: QualName,
        mut attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> NodeId {
        let url_attr = match &*name.local {
            "img" => Some(local_name!("src")),
            "link" => Some(local_name!("href")),
            _ => None,
        };
        if let Some(url_attr) = url_attr {
            for a in &mut attrs {
                if a.name.local == url_attr && !self.resolves(&a.value) {
                    a.value = StrTendril::new();
                }
            }
        }
        let is_style = name.local == local_name!("style");
        let id = self.inner.create_element(name, attrs, flags);
        if is_style {
            self.styles.borrow_mut().push(id);
        }
        id
    }

    fn parse_error(&self, msg: Cow<'static, str>) {
        self.inner.parse_error(msg)
    }
    fn get_document(&self) -> NodeId {
        self.inner.get_document()
    }
    fn elem_name<'a>(&'a self, target: &'a NodeId) -> Self::ElemName<'a> {
        self.inner.elem_name(target)
    }
    fn create_comment(&self, text: StrTendril) -> NodeId {
        self.inner.create_comment(text)
    }
    fn create_pi(&self, target: StrTendril, data: StrTendril) -> NodeId {
        self.inner.create_pi(target, data)
    }
    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        self.inner.append(parent, child)
    }
    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        self.inner.append_before_sibling(sibling, new_node)
    }
    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        self.inner
            .append_based_on_parent_node(element, prev_element, child)
    }
    fn append_doctype_to_document(&self, name: StrTendril, public: StrTendril, system: StrTendril) {
        self.inner.append_doctype_to_document(name, public, system)
    }
    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        self.inner.get_template_contents(target)
    }
    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        self.inner.same_node(x, y)
    }
    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.inner.set_quirks_mode(mode)
    }
    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<Attribute>) {
        self.inner.add_attrs_if_missing(target, attrs)
    }
    fn remove_from_parent(&self, target: &NodeId) {
        self.inner.remove_from_parent(target)
    }
    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        self.inner.reparent_children(node, new_parent)
    }
}

/// `css` with every `@import` renamed `@x-import` (an unknown at-rule, dropped whole), or
/// `None` when it has none. The at-keyword is compared as CSS does, after unescaping and
/// ignoring ASCII case, so `@IMPORT` and `@\69mport` are caught too. Inside a string or
/// comment the rename is harmless.
fn defuse_imports(css: &str) -> Option<String> {
    let mut out = String::new();
    let mut copied = 0;
    for (at, _) in css.match_indices('@') {
        let name_start = at + 1;
        if !ident_name(&css[name_start..]).eq_ignore_ascii_case("import") {
            continue;
        }
        out.push_str(&css[copied..name_start]);
        out.push_str("x-");
        copied = name_start;
    }
    if copied == 0 {
        return None;
    }
    out.push_str(&css[copied..]);
    Some(out)
}

/// The unescaped CSS name at the start of `s` (stops at the first non-name character).
fn ident_name(s: &str) -> String {
    let mut name = String::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || !c.is_ascii() {
            name.push(c);
            chars.next();
        } else if c == '\\' {
            chars.next();
            let mut hex = String::new();
            while hex.len() < 6 && chars.peek().is_some_and(char::is_ascii_hexdigit) {
                hex.extend(chars.next());
            }
            if hex.is_empty() {
                match chars.next() {
                    Some('\n' | '\r' | '\u{c}') | None => break,
                    Some(c) => name.push(c),
                }
            } else {
                let cp = u32::from_str_radix(&hex, 16).unwrap_or(0xfffd);
                name.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                if chars.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                    chars.next();
                }
            }
        } else {
            break;
        }
        // No at-rule name worth comparing is this long.
        if name.len() > 16 {
            break;
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use blitz_dom::QuirksMode as Mode;

    fn mode(html: &str) -> Mode {
        document(html, DocumentConfig::default()).quirks_mode()
    }

    const XHTML_TRANSITIONAL: &str = r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd">"#;

    #[test]
    fn the_mails_doctype_sets_the_mode() {
        assert_eq!(mode("<!DOCTYPE html><p>x"), Mode::NoQuirks);
        assert_eq!(mode("<p>x"), Mode::Quirks);
        assert_eq!(
            mode(&format!("{XHTML_TRANSITIONAL}<p>x")),
            Mode::LimitedQuirks
        );
        assert_eq!(
            mode(
                r#"<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN" "http://www.w3.org/TR/html4/loose.dtd"><p>x"#
            ),
            Mode::LimitedQuirks
        );
        // Without the system identifier, HTML 4.01 Transitional is quirks mode.
        assert_eq!(
            mode(r#"<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN"><p>x"#),
            Mode::Quirks
        );
        assert_eq!(
            mode(
                r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"><p>x"#
            ),
            Mode::NoQuirks
        );
    }

    #[test]
    fn what_comes_before_the_doctype_keeps_its_mode() {
        let xml = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n{XHTML_TRANSITIONAL}<p>x");
        assert_eq!(mode(&xml), Mode::LimitedQuirks);
        assert_eq!(mode("\u{feff}<!DOCTYPE html><p>x"), Mode::NoQuirks);
        assert_eq!(mode("<!--™-->\n  <!DOCTYPE html><p>x"), Mode::NoQuirks);
        // `@import` is renamed after parsing, in the document's mode.
        assert_eq!(
            mode("<!DOCTYPE html><style>@import url();</style><p>x"),
            Mode::NoQuirks
        );
    }

    #[test]
    fn imports_are_renamed_in_any_spelling() {
        assert_eq!(defuse_imports("p{color:red}"), None);
        assert_eq!(
            defuse_imports("@import url();@IMPORT \"http://\";@\\69mport x;@\\49 MPORT y;@media{}")
                .as_deref(),
            Some("@x-import url();@x-IMPORT \"http://\";@x-\\69mport x;@x-\\49 MPORT y;@media{}")
        );
        assert_eq!(defuse_imports("@importer{}"), None);
    }
}
