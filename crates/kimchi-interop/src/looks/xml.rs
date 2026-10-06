//! A small element tree over quick-xml, enough to walk XMP and Premiere preset files.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

#[derive(Debug, Clone, Default)]
pub struct Element {
    /// The qualified name as written (`crs:Exposure2012`).
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    /// The text directly inside it, entities resolved.
    pub text: String,
}

impl Element {
    /// The name without its namespace prefix.
    pub fn local(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// The first child called `name` (qualified or local).
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name || c.local() == name)
    }

    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim())
    }

    /// Every element below this one, depth first.
    pub fn descendants(&self) -> Vec<&Element> {
        let mut out = vec![];
        let mut stack: Vec<&Element> = self.children.iter().rev().collect();
        while let Some(e) = stack.pop() {
            out.push(e);
            stack.extend(e.children.iter().rev());
        }
        out
    }
}

fn start(e: &BytesStart) -> Element {
    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let attrs = e
        .attributes()
        .flatten()
        .map(|a| {
            let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
            let v = a.unescape_value().map(|v| v.into_owned()).unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned());
            (k, v)
        })
        .collect();
    Element { name, attrs, ..Default::default() }
}

/// Parses a document into its root element.
pub fn parse(text: &str) -> Result<Element, String> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<Element> = vec![Element { name: "#document".into(), ..Default::default() }];
    loop {
        let event = reader.read_event().map_err(|e| format!("not valid XML ({e}, at byte {})", reader.buffer_position()))?;
        match event {
            Event::Start(e) => stack.push(start(&e)),
            Event::Empty(e) => {
                let el = start(&e);
                if let Some(top) = stack.last_mut() {
                    top.children.push(el);
                }
            }
            Event::End(_) => {
                let done = stack.pop().ok_or("unbalanced XML")?;
                match stack.last_mut() {
                    Some(top) => top.children.push(done),
                    None => return Err("unbalanced XML".into()),
                }
            }
            Event::Text(t) => {
                if let (Some(top), Ok(s)) = (stack.last_mut(), t.decode()) {
                    top.text.push_str(&s);
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref() {
                    Ok(Some(c)) => Some(c.to_string()),
                    _ => match r.decode().as_deref() {
                        Ok("amp") => Some("&".into()),
                        Ok("lt") => Some("<".into()),
                        Ok("gt") => Some(">".into()),
                        Ok("quot") => Some("\"".into()),
                        Ok("apos") => Some("'".into()),
                        _ => None,
                    },
                };
                if let (Some(top), Some(s)) = (stack.last_mut(), resolved) {
                    top.text.push_str(&s);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let mut doc = stack.into_iter().next().ok_or("empty XML")?;
    // An unclosed document keeps what was read.
    Ok(if doc.children.len() == 1 { doc.children.remove(0) } else { doc })
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_attributes_text_and_entities() {
        let root = super::parse("<a x=\"1 &amp; 2\"><b>Tom &amp; Jerry</b><c/></a>").unwrap();
        assert_eq!(root.name, "a");
        assert_eq!(root.attr("x"), Some("1 & 2"));
        assert_eq!(root.child_text("b"), Some("Tom & Jerry"));
        assert_eq!(root.descendants().len(), 2);
        assert!(super::parse("<a><b></a>").is_err());
    }
}
