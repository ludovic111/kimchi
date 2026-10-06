//! A small XML tree for the XML formats (FCPXML, Final Cut 7 XML, MLT, Premiere projects): read
//! a whole document with quick-xml into [`Element`]s, look things up by name, and write a
//! document back with [`Writer`]. Project files are small enough (a few MB at most) that a tree
//! is simpler than streaming.

use std::fmt::Write as _;

use quick_xml::Reader;
use quick_xml::events::Event;

/// Deepest nesting read: a file nested deeper is refused rather than overflowing the stack.
const MAX_DEPTH: usize = 512;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    /// Text directly inside the element (its children's text not included), entities decoded.
    pub text: String,
}

impl Element {
    pub fn new(name: impl Into<String>) -> Element {
        Element { name: name.into(), ..Default::default() }
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn attr_f64(&self, name: &str) -> Option<f64> {
        self.attr(name).and_then(|v| v.trim().parse().ok()).filter(|v: &f64| v.is_finite())
    }

    /// The first child named `name`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Follows a path of child names (`"rate/timebase"`).
    pub fn path(&self, path: &str) -> Option<&Element> {
        path.split('/').try_fold(self, |e, name| e.child(name))
    }

    /// The trimmed text of the element at `path`, if it has any.
    pub fn text_at(&self, path: &str) -> Option<&str> {
        self.path(path).map(|e| e.text.trim()).filter(|t| !t.is_empty())
    }

    pub fn f64_at(&self, path: &str) -> Option<f64> {
        self.text_at(path).and_then(|t| t.parse().ok()).filter(|v: &f64| v.is_finite())
    }

    pub fn i64_at(&self, path: &str) -> Option<i64> {
        self.text_at(path).and_then(|t| t.parse::<i64>().ok().or_else(|| t.parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v.round() as i64)))
    }

    /// `TRUE`/`FALSE`, `true`/`false`, `1`/`0`.
    pub fn bool_at(&self, path: &str) -> Option<bool> {
        self.text_at(path).and_then(parse_bool)
    }

    /// Every element below this one (depth first), itself included.
    pub fn descendants(&self) -> Vec<&Element> {
        let mut out = vec![];
        let mut stack = vec![self];
        while let Some(e) = stack.pop() {
            out.push(e);
            stack.extend(e.children.iter().rev());
        }
        out
    }

    /// Every element named `name` below this one.
    pub fn find_all<'a>(&'a self, name: &str) -> Vec<&'a Element> {
        self.descendants().into_iter().filter(|e| e.name == name).collect()
    }

    // Building.

    pub fn with_attr(mut self, k: &str, v: impl ToString) -> Element {
        self.attrs.push((k.to_string(), v.to_string()));
        self
    }

    pub fn set_attr(&mut self, k: &str, v: impl ToString) {
        match self.attrs.iter_mut().find(|(n, _)| n == k) {
            Some((_, old)) => *old = v.to_string(),
            None => self.attrs.push((k.to_string(), v.to_string())),
        }
    }

    pub fn with_text(mut self, t: impl ToString) -> Element {
        self.text = t.to_string();
        self
    }

    pub fn with_child(mut self, c: Element) -> Element {
        self.children.push(c);
        self
    }

    pub fn push(&mut self, c: Element) -> &mut Element {
        self.children.push(c);
        self.children.last_mut().expect("just pushed")
    }

    /// Adds `<name>text</name>`.
    pub fn leaf(&mut self, name: &str, text: impl ToString) -> &mut Element {
        self.children.push(Element::new(name).with_text(text));
        self
    }
}

pub fn parse_bool(t: &str) -> Option<bool> {
    match t.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// Reads a whole document; its root element.
pub fn parse(text: &str) -> Result<Element, String> {
    let text = text.trim_start_matches('\u{feff}');
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    reader.config_mut().check_end_names = false;
    let mut stack: Vec<Element> = vec![];
    let mut root: Option<Element> = None;
    let bad = |e: &dyn std::fmt::Display, pos: u64| format!("The XML doesn't read at byte {pos}: {e}");
    loop {
        let pos = reader.buffer_position();
        let ev = reader.read_event().map_err(|e| bad(&e, pos))?;
        let empty = matches!(ev, Event::Empty(_));
        match ev {
            Event::Start(s) | Event::Empty(s) => {
                let mut el = Element::new(String::from_utf8_lossy(s.name().as_ref()).into_owned());
                for a in s.attributes().with_checks(false) {
                    let a = a.map_err(|e| bad(&e, pos))?;
                    let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
                    let value = a.unescape_value().map(|v| v.into_owned()).unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned());
                    el.attrs.push((key, value));
                }
                if empty {
                    attach(&mut stack, &mut root, el);
                } else {
                    if stack.len() >= MAX_DEPTH {
                        return Err(format!("The XML is nested more than {MAX_DEPTH} levels deep."));
                    }
                    stack.push(el);
                }
            }
            Event::End(_) => {
                if let Some(mut el) = stack.pop() {
                    // Indentation between child elements isn't text.
                    if !el.children.is_empty() {
                        el.text = el.text.trim().to_string();
                    }
                    attach(&mut stack, &mut root, el);
                }
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.xml_content().unwrap_or_default());
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Event::GeneralRef(r) => {
                if let Some(top) = stack.last_mut() {
                    if r.is_char_ref() {
                        if let Ok(Some(c)) = r.resolve_char_ref() {
                            top.text.push(c);
                        }
                    } else {
                        let name = r.decode().unwrap_or_default();
                        match quick_xml::escape::resolve_predefined_entity(&name) {
                            Some(v) => top.text.push_str(v),
                            None => {
                                top.text.push('&');
                                top.text.push_str(&name);
                                top.text.push(';');
                            }
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    // An unclosed document: close what is open.
    while let Some(el) = stack.pop() {
        attach(&mut stack, &mut root, el);
    }
    root.ok_or_else(|| "The file has no XML in it.".to_string())
}

fn attach(stack: &mut [Element], root: &mut Option<Element>, el: Element) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(el),
        None => {
            if root.is_none() {
                *root = Some(el);
            }
        }
    }
}

/// Escapes text and attribute values.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Characters XML 1.0 can't hold at all.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// Writes a document: the XML declaration, an optional doctype line, then `root`, indented by
/// two spaces. Elements with text and no children stay on one line.
pub fn write(root: &Element, doctype: Option<&str>) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    if let Some(d) = doctype {
        out.push_str(d);
        out.push('\n');
    }
    write_el(&mut out, root, 0);
    out
}

fn write_el(out: &mut String, el: &Element, depth: usize) {
    let pad = "  ".repeat(depth);
    let _ = write!(out, "{pad}<{}", el.name);
    for (k, v) in &el.attrs {
        let _ = write!(out, " {k}=\"{}\"", escape(v));
    }
    if el.children.is_empty() && el.text.is_empty() {
        out.push_str("/>\n");
        return;
    }
    out.push('>');
    if el.children.is_empty() {
        out.push_str(&escape(&el.text));
        let _ = writeln!(out, "</{}>", el.name);
        return;
    }
    if !el.text.trim().is_empty() {
        out.push_str(&escape(&el.text));
    }
    out.push('\n');
    for c in &el.children {
        write_el(out, c, depth + 1);
    }
    let _ = writeln!(out, "{pad}</{}>", el.name);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes() {
        let doc = r#"<?xml version="1.0"?><!DOCTYPE xmeml><xmeml version="4"><a x="1 &amp; 2"><b>Tom &amp; Jerry &#233;</b><c/><![CDATA[<raw>]]></a></xmeml>"#;
        let root = parse(doc).unwrap();
        assert_eq!(root.name, "xmeml");
        let a = root.child("a").unwrap();
        assert_eq!(a.attr("x"), Some("1 & 2"));
        assert_eq!(root.text_at("a/b"), Some("Tom & Jerry é"));
        assert_eq!(a.text, "<raw>");
        assert!(a.child("c").is_some());
        let back = parse(&write(&root, Some("<!DOCTYPE xmeml>"))).unwrap();
        assert_eq!(back, root);
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(parse("").is_err());
        assert!(parse("not xml at all").is_err());
        assert!(parse("<a><b></a>").is_ok());
        let deep = "<a>".repeat(MAX_DEPTH + 5);
        assert!(parse(&deep).is_err());
    }
}
