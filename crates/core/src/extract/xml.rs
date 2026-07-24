//! A tiny XML tree, shared by the OOXML and ODF extractors.
//!
//! The office formats need to look *around* an element (a paragraph's style
//! lives in a sibling subtree, a hyperlink target in another file), which a
//! streaming reader makes awkward. Parsing into a small tree first keeps each
//! extractor readable; the documents involved are already fully in memory
//! because they come out of a ZIP entry.
//!
//! Names are stored without their prefix: OOXML and ODF use fixed, well-known
//! prefixes (`w:`, `a:`, `text:`, `table:`), and matching on local names keeps
//! the extractors free of namespace plumbing.

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

/// An XML element: local name, attributes and children.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

/// A child of an element.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    /// Value of an attribute, matched on its local name.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// An attribute parsed as a number.
    pub fn attr_number(&self, name: &str) -> Option<i64> {
        self.attr(name)?.trim().parse().ok()
    }

    /// Direct child elements, in document order.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|node| match node {
            Node::Element(element) => Some(element),
            Node::Text(_) => None,
        })
    }

    /// First direct child with this name.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|element| element.name == name)
    }

    /// Follow a chain of direct children: `element.path(["pPr", "pStyle"])`.
    pub fn path<'a, I>(&self, names: I) -> Option<&Element>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut current = self;
        for name in names {
            current = current.child(name)?;
        }
        Some(current)
    }

    /// First descendant with this name, depth first.
    pub fn find(&self, name: &str) -> Option<&Element> {
        if self.name == name {
            return Some(self);
        }
        self.elements().find_map(|child| child.find(name))
    }

    /// Every descendant with this name. Does not descend into a match, so
    /// nested structures (a list inside a list item) stay for the caller.
    pub fn find_all(&self, name: &str) -> Vec<&Element> {
        let mut found = Vec::new();
        self.collect(name, &mut found);
        found
    }

    fn collect<'a>(&'a self, name: &str, found: &mut Vec<&'a Element>) {
        for child in self.elements() {
            if child.name == name {
                found.push(child);
            } else {
                child.collect(name, found);
            }
        }
    }

    /// All text under this element, concatenated.
    pub fn text(&self) -> String {
        let mut out = String::new();
        self.write_text(&mut out);
        out
    }

    fn write_text(&self, out: &mut String) {
        for child in &self.children {
            match child {
                Node::Text(text) => out.push_str(text),
                Node::Element(element) => element.write_text(out),
            }
        }
    }
}

/// Parse a document and return its root element.
pub fn parse(xml: &str) -> std::result::Result<Element, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = false;

    let mut stack: Vec<Element> = vec![Element::default()];

    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => stack.push(element_from(&start)),
            Ok(Event::Empty(empty)) => {
                let element = element_from(&empty);
                push_child(&mut stack, Node::Element(element));
            }
            Ok(Event::End(_)) => {
                if stack.len() > 1 {
                    let element = stack.pop().expect("checked length");
                    push_child(&mut stack, Node::Element(element));
                }
            }
            Ok(Event::Text(text)) => {
                if let Ok(text) = text.xml10_content()
                    && !text.is_empty()
                {
                    push_child(&mut stack, Node::Text(text.to_string()));
                }
            }
            Ok(Event::CData(data)) => {
                if let Ok(text) = data.decode() {
                    push_child(&mut stack, Node::Text(text.to_string()));
                }
            }
            // In quick-xml 0.41 entity references arrive as their own events.
            Ok(Event::GeneralRef(reference)) => {
                if let Some(text) = resolve_entity(&reference) {
                    push_child(&mut stack, Node::Text(text));
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
    }

    // Unwrap the virtual root and hand back the document element.
    let root = stack.swap_remove(0);
    root.elements()
        .next()
        .cloned()
        .ok_or_else(|| "the document has no root element".to_string())
}

fn element_from(start: &quick_xml::events::BytesStart<'_>) -> Element {
    let name = String::from_utf8_lossy(start.local_name().as_ref()).to_string();

    let attrs = start
        .attributes()
        .filter_map(Result::ok)
        .filter_map(|attr| {
            let key = String::from_utf8_lossy(attr.key.local_name().as_ref()).to_string();
            let value = attr
                .decoded_and_normalized_value(XmlVersion::Explicit1_0, reader_decoder())
                .ok()?;
            Some((key, value.to_string()))
        })
        .collect();

    Element {
        name,
        attrs,
        children: Vec::new(),
    }
}

/// OOXML and ODF are always UTF-8, so the default decoder is the right one.
fn reader_decoder() -> quick_xml::Decoder {
    quick_xml::Reader::from_str("").decoder()
}

fn push_child(stack: &mut [Element], node: Node) {
    if let Some(parent) = stack.last_mut() {
        // Merge adjacent text so entity references do not split a run.
        if let (Node::Text(text), Some(Node::Text(previous))) = (&node, parent.children.last_mut())
        {
            previous.push_str(text);
            return;
        }
        parent.children.push(node);
    }
}

fn resolve_entity(reference: &quick_xml::events::BytesRef<'_>) -> Option<String> {
    if let Ok(Some(ch)) = reference.resolve_char_ref() {
        return Some(ch.to_string());
    }
    let name = reference.decode().ok()?;
    let resolved = match name.as_ref() {
        "lt" => "<",
        "gt" => ">",
        "amp" => "&",
        "quot" => "\"",
        "apos" => "'",
        _ => return None,
    };
    Some(resolved.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_elements_attributes_and_text() {
        let root = parse(r#"<w:p w:rsid="1"><w:r><w:t>hello</w:t></w:r></w:p>"#).unwrap();
        assert_eq!(root.name, "p");
        assert_eq!(root.attr("rsid"), Some("1"));
        assert_eq!(root.text(), "hello");
    }

    #[test]
    fn keeps_empty_elements_as_children() {
        let root = parse("<p><br/><t>after</t></p>").unwrap();
        let names: Vec<&str> = root.elements().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["br", "t"]);
    }

    #[test]
    fn resolves_entities_into_one_text_run() {
        let root = parse("<t>a &amp; b &#65;</t>").unwrap();
        assert_eq!(root.text(), "a & b A");
        assert_eq!(root.children.len(), 1, "text should be merged: {root:?}");
    }

    #[test]
    fn find_all_does_not_descend_into_matches() {
        let root =
            parse("<body><list><item><list><item>deep</item></list></item></list></body>").unwrap();
        let lists = root.find_all("list");
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].find_all("item").len(), 1);
    }

    #[test]
    fn path_walks_direct_children_only() {
        let root = parse("<p><pPr><pStyle val=\"Heading1\"/></pPr><r><t>x</t></r></p>").unwrap();
        assert_eq!(
            root.path(["pPr", "pStyle"]).and_then(|e| e.attr("val")),
            Some("Heading1")
        );
        assert!(root.path(["pStyle"]).is_none());
    }

    #[test]
    fn malformed_xml_is_an_error() {
        assert!(parse("<a><b></a>").is_err());
        assert!(parse("").is_err());
    }
}
