//! OOXML extractors — docx and pptx, plus the parts they share.
//!
//! Both are ZIP + XML: the document body lives in one part, its hyperlink
//! targets in a `_rels` part next to it, and the document properties in
//! `docProps/core.xml`.

pub mod docx;
pub mod pptx;

use std::collections::HashMap;

use crate::extract::container::Container;
use crate::extract::xml;
use crate::model::Metadata;

/// Read `docProps/core.xml` — the OOXML metadata part shared by every flavour.
pub fn core_properties(container: &mut Container) -> Metadata {
    let Some(source) = container.read_optional("docProps/core.xml") else {
        return Metadata::default();
    };
    let Ok(root) = xml::parse(&source) else {
        return Metadata::default();
    };

    let text_of = |name: &str| {
        root.find(name)
            .map(|element| element.text().trim().to_string())
            .filter(|text| !text.is_empty())
    };

    Metadata {
        title: text_of("title"),
        author: text_of("creator"),
        created: text_of("created"),
        language: text_of("language"),
        ..Metadata::default()
    }
}

/// Relationship id → target, from a `_rels` part.
///
/// Hyperlinks in OOXML carry a relationship id (`r:id="rId7"`); the URL itself
/// lives in the part's relationships file.
pub fn relationships(source: Option<&str>) -> HashMap<String, String> {
    let mut targets = HashMap::new();

    let Some(root) = source.and_then(|source| xml::parse(source).ok()) else {
        return targets;
    };
    for relationship in root.find_all("Relationship") {
        if let (Some(id), Some(target)) = (relationship.attr("Id"), relationship.attr("Target")) {
            targets.insert(id.to_string(), target.to_string());
        }
    }

    targets
}

/// An OOXML boolean attribute: present means on unless it says otherwise.
pub fn toggle(element: Option<&xml::Element>) -> bool {
    match element {
        None => false,
        Some(element) => !matches!(element.attr("val"), Some("0" | "false" | "off")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_core_properties() {
        let bytes = crate::extract::container::tests::zip_bytes(&[(
            "docProps/core.xml",
            r#"<cp:coreProperties xmlns:cp="c" xmlns:dc="d" xmlns:dcterms="t">
                 <dc:title>A title</dc:title>
                 <dc:creator>Ada</dc:creator>
                 <dcterms:created>2026-07-24T10:00:00Z</dcterms:created>
               </cp:coreProperties>"#,
        )]);
        let mut container = Container::from_bytes(&bytes).unwrap();
        let meta = core_properties(&mut container);

        assert_eq!(meta.title.as_deref(), Some("A title"));
        assert_eq!(meta.author.as_deref(), Some("Ada"));
        assert_eq!(meta.created.as_deref(), Some("2026-07-24T10:00:00Z"));
    }

    #[test]
    fn missing_properties_are_not_an_error() {
        let bytes = crate::extract::container::tests::zip_bytes(&[("word/document.xml", "<w:x/>")]);
        let mut container = Container::from_bytes(&bytes).unwrap();
        assert_eq!(core_properties(&mut container), Metadata::default());
    }

    #[test]
    fn reads_relationship_targets() {
        let targets = relationships(Some(
            r#"<Relationships><Relationship Id="rId1" Target="https://example.com"/></Relationships>"#,
        ));
        assert_eq!(
            targets.get("rId1").map(String::as_str),
            Some("https://example.com")
        );
        assert!(relationships(None).is_empty());
    }

    #[test]
    fn toggle_reads_ooxml_booleans() {
        let on = xml::parse("<b/>").unwrap();
        let off = xml::parse(r#"<b val="0"/>"#).unwrap();
        assert!(toggle(Some(&on)));
        assert!(!toggle(Some(&off)));
        assert!(!toggle(None));
    }
}
