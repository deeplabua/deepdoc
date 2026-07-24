//! OpenDocument text and presentations (`.odt`, `.odp`).
//!
//! ODF puts the content in `content.xml` and the metadata in `meta.xml`.
//! Structure is explicit — `text:h` carries its own outline level, `text:list`
//! nests directly — but character styling is not: a `text:span` names a style
//! whose weight and slant live in `office:automatic-styles`, so that table is
//! read first.

use std::collections::HashMap;
use std::path::Path;

use crate::detect::{Format, Sniff};
use crate::error::{Error, Result};
use crate::extract::container::Container;
use crate::extract::xml::{self, Element};
use crate::extract::{ExtractOpts, Extractor};
use crate::model::{Block, Document, Inline, Metadata, Row, Span};

/// Handles both `.odt` and `.odp`; the body element decides which is which.
pub struct OdfExtractor;

impl Extractor for OdfExtractor {
    fn name(&self) -> &'static str {
        "odf"
    }

    fn supports(&self, _path: &Path, sniff: &Sniff) -> bool {
        matches!(
            crate::detect::detect(sniff),
            Some(Format::Odt | Format::Odp)
        )
    }

    fn extract(&self, path: &Path, _opts: &ExtractOpts) -> Result<Document> {
        let mut container = Container::open(path)?;

        let content = container
            .read("content.xml")
            .map_err(|message| Error::parse(path, message))?;
        let blocks = parse(&content).map_err(|message| Error::parse(path, message))?;

        let mut meta = metadata(container.read_optional("meta.xml").as_deref());
        meta.source_format = crate::detect::detect_path(path).ok();
        meta.source_path = Some(path.display().to_string());

        Ok(Document { meta, blocks })
    }
}

/// Parse `content.xml` into blocks. Pure.
pub fn parse(content: &str) -> std::result::Result<Vec<Block>, String> {
    let root = xml::parse(content)?;
    let styles = StyleTable::parse(&root);

    let Some(body) = root.child("body") else {
        return Ok(Vec::new());
    };

    // A presentation is a sequence of pages; a text document is one flow.
    if let Some(presentation) = body.child("presentation") {
        return Ok(pages(presentation, &styles));
    }

    let text = body.child("text").unwrap_or(body);
    Ok(blocks(text, &styles))
}

/// Read `meta.xml` for the document's title and author.
pub fn metadata(source: Option<&str>) -> Metadata {
    let Some(root) = source.and_then(|source| xml::parse(source).ok()) else {
        return Metadata::default();
    };

    let text_of = |name: &str| {
        root.find(name)
            .map(|element| element.text().trim().to_string())
            .filter(|text| !text.is_empty())
    };

    Metadata {
        title: text_of("title"),
        author: text_of("creator").or_else(|| text_of("initial-creator")),
        created: text_of("creation-date"),
        language: text_of("language"),
        ..Metadata::default()
    }
}

/// Each `draw:page` becomes a section, like a pptx slide.
fn pages(presentation: &Element, styles: &StyleTable) -> Vec<Block> {
    let mut out = Vec::new();

    for (index, page) in presentation.find_all("page").into_iter().enumerate() {
        let number = index + 1;
        if number > 1 {
            out.push(Block::PageBreak {
                page: number as u32,
            });
        }

        let mut content = blocks(page, styles);
        // The first line of a slide is its title, as on a pptx slide.
        let heading = match content.first() {
            Some(Block::Paragraph { text }) => {
                let text = text.clone();
                content.remove(0);
                text
            }
            _ => Inline::text(
                page.attr("name")
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("Slide {number}")),
            ),
        };

        out.push(Block::Heading {
            level: 2,
            text: heading,
        });
        out.extend(content);
    }

    out
}

/// Walk a block container: the text body, a list item or a table cell.
fn blocks(container: &Element, styles: &StyleTable) -> Vec<Block> {
    let mut out = Vec::new();

    for child in container.elements() {
        match child.name.as_str() {
            "h" => {
                let level = child.attr_number("outline-level").unwrap_or(1).clamp(1, 6) as u8;
                let text = inline(child, styles);
                if !text.plain_text().trim().is_empty() {
                    out.push(Block::Heading { level, text });
                }
            }
            "p" => {
                let text = inline(child, styles);
                if !text.plain_text().trim().is_empty() {
                    out.push(Block::Paragraph { text });
                }
                out.extend(images(child));
            }
            "list" => {
                if let Some(list) = list_block(child, styles) {
                    out.push(list);
                }
            }
            "table" => {
                if let Some(table) = table_block(child, styles) {
                    out.push(table);
                }
            }
            // Frames, sections and text boxes wrap content without adding any.
            "frame" | "text-box" | "section" | "a" | "soft-page-break" => {
                out.extend(blocks(child, styles));
            }
            _ => {}
        }
    }

    out
}

fn list_block(list: &Element, styles: &StyleTable) -> Option<Block> {
    let items: Vec<Vec<Block>> = list
        .find_all("list-item")
        .into_iter()
        .map(|item| blocks(item, styles))
        .filter(|blocks| !blocks.is_empty())
        .collect();

    if items.is_empty() {
        return None;
    }
    Some(Block::List {
        ordered: styles.is_ordered(list.attr("style-name").unwrap_or_default()),
        items,
    })
}

fn table_block(table: &Element, styles: &StyleTable) -> Option<Block> {
    let mut rows: Vec<Row> = Vec::new();

    for row in table.find_all("table-row") {
        let cells: Vec<Inline> = row
            .find_all("table-cell")
            .into_iter()
            .map(|cell| {
                let text = blocks(cell, styles)
                    .iter()
                    .map(Block::plain_text)
                    .collect::<Vec<_>>()
                    .join(" ");
                Inline::text(text.trim())
            })
            .collect();
        if !cells.is_empty() {
            rows.push(Row::new(cells));
        }
    }

    if rows.is_empty() {
        return None;
    }
    let header = rows.remove(0);
    Some(Block::Table {
        header: Some(header),
        rows,
    })
}

/// Collect the inline content of a paragraph or heading.
fn inline(paragraph: &Element, styles: &StyleTable) -> Inline {
    let mut spans = Vec::new();
    collect_inline(paragraph, styles, &mut spans);
    Inline::new(spans)
}

fn collect_inline(element: &Element, styles: &StyleTable, spans: &mut Vec<Span>) {
    for child in &element.children {
        match child {
            xml::Node::Text(text) => spans.push(Span::Text(text.clone())),
            xml::Node::Element(element) => match element.name.as_str() {
                "span" => {
                    let mut inner = Vec::new();
                    collect_inline(element, styles, &mut inner);
                    if inner.is_empty() {
                        continue;
                    }
                    let style = element.attr("style-name").unwrap_or_default();
                    spans.extend(styles.wrap(style, inner));
                }
                "a" => {
                    let mut inner = Vec::new();
                    collect_inline(element, styles, &mut inner);
                    match element.attr("href") {
                        Some(href) if !inner.is_empty() => spans.push(Span::Link {
                            href: href.to_string(),
                            text: inner,
                        }),
                        _ => spans.extend(inner),
                    }
                }
                "line-break" => spans.push(Span::LineBreak),
                "tab" => spans.push(Span::Text("\t".into())),
                // `<text:s text:c="3"/>` is a run of spaces.
                "s" => {
                    let count = element.attr_number("c").unwrap_or(1).clamp(1, 64) as usize;
                    spans.push(Span::Text(" ".repeat(count)));
                }
                // Frames inside a paragraph hold images, handled as blocks.
                "frame" => {}
                _ => collect_inline(element, styles, spans),
            },
        }
    }
}

/// Images are `draw:frame` elements carrying an `svg:title`/`svg:desc`.
fn images(paragraph: &Element) -> Vec<Block> {
    paragraph
        .find_all("frame")
        .into_iter()
        .filter(|frame| frame.find("image").is_some())
        .map(|frame| {
            let alt = frame
                .find("desc")
                .or_else(|| frame.find("title"))
                .map(|element| element.text().trim().to_string())
                .filter(|text| !text.is_empty())
                .or_else(|| frame.attr("name").map(str::to_string));
            Block::Image { alt }
        })
        .collect()
}

/// The style names a document refers to, resolved: character styling for spans
/// and the bullet-versus-number choice for lists.
#[derive(Default)]
struct StyleTable {
    /// Style name → (bold, italic).
    styles: HashMap<String, (bool, bool)>,
    /// Names of list styles whose first level is numbered.
    ordered_lists: std::collections::HashSet<String>,
}

impl StyleTable {
    fn parse(root: &Element) -> StyleTable {
        let mut styles = HashMap::new();
        let mut ordered_lists = std::collections::HashSet::new();

        // `<text:list-style style:name="L1"><text:list-level-style-number …>`
        // is a numbered list; `…-style-bullet` and `…-style-image` are not.
        for list_style in root.find_all("list-style") {
            let Some(name) = list_style.attr("name") else {
                continue;
            };
            let numbered = list_style
                .elements()
                .find(|level| level.name.starts_with("list-level-style-"))
                .is_some_and(|level| level.name == "list-level-style-number");
            if numbered {
                ordered_lists.insert(name.to_string());
            }
        }

        for style in root.find_all("style") {
            let Some(name) = style.attr("name") else {
                continue;
            };
            let Some(properties) = style.child("text-properties") else {
                continue;
            };
            let bold = properties.attr("font-weight").is_some_and(|weight| {
                weight == "bold" || weight.parse().is_ok_and(|w: u32| w >= 600)
            });
            let italic = properties
                .attr("font-style")
                .is_some_and(|slant| slant == "italic" || slant == "oblique");

            if bold || italic {
                styles.insert(name.to_string(), (bold, italic));
            }
        }

        StyleTable {
            styles,
            ordered_lists,
        }
    }

    /// Bulleted unless the list's style says it counts.
    fn is_ordered(&self, style: &str) -> bool {
        self.ordered_lists.contains(style)
    }

    fn wrap(&self, style: &str, inner: Vec<Span>) -> Vec<Span> {
        match self.styles.get(style) {
            None | Some((false, false)) => inner,
            Some((true, false)) => vec![Span::Bold(inner)],
            Some((false, true)) => vec![Span::Italic(inner)],
            Some((true, true)) => vec![Span::Bold(vec![Span::Italic(inner)])],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_document(body: &str, styles: &str) -> String {
        format!(
            r#"<office:document-content xmlns:office="o" xmlns:text="t" xmlns:table="tb" xmlns:draw="d">
                 <office:automatic-styles>{styles}</office:automatic-styles>
                 <office:body><office:text>{body}</office:text></office:body>
               </office:document-content>"#
        )
    }

    fn blocks_of(body: &str) -> Vec<Block> {
        parse(&text_document(body, "")).expect("valid odf")
    }

    #[test]
    fn headings_carry_their_outline_level() {
        assert_eq!(
            blocks_of(
                r#"<text:h text:outline-level="1">Report</text:h>
                   <text:h text:outline-level="3">Detail</text:h>
                   <text:p>Body.</text:p>"#
            ),
            vec![
                Block::Heading {
                    level: 1,
                    text: Inline::text("Report")
                },
                Block::Heading {
                    level: 3,
                    text: Inline::text("Detail")
                },
                Block::paragraph("Body."),
            ]
        );
    }

    #[test]
    fn spans_pick_up_styling_from_the_automatic_styles() {
        let source = text_document(
            r#"<text:p>plain <text:span text:style-name="T1">bold</text:span> <text:span text:style-name="T2">italic</text:span></text:p>"#,
            r#"<style:style style:name="T1"><style:text-properties fo:font-weight="bold"/></style:style>
               <style:style style:name="T2"><style:text-properties fo:font-style="italic"/></style:style>"#,
        );
        let parsed = parse(&source).unwrap();

        let Block::Paragraph { text } = &parsed[0] else {
            panic!("expected a paragraph, got {parsed:?}");
        };
        assert_eq!(
            text.spans,
            vec![
                Span::Text("plain ".into()),
                Span::Bold(vec![Span::Text("bold".into())]),
                Span::Text(" ".into()),
                Span::Italic(vec![Span::Text("italic".into())]),
            ]
        );
    }

    #[test]
    fn links_tabs_and_breaks_survive() {
        let Block::Paragraph { text } = &blocks_of(
            r#"<text:p><text:a xlink:href="https://example.com">site</text:a><text:line-break/><text:tab/>end</text:p>"#,
        )[0] else {
            panic!("expected a paragraph");
        };
        assert_eq!(
            text.spans,
            vec![
                Span::Link {
                    href: "https://example.com".into(),
                    text: vec![Span::Text("site".into())],
                },
                Span::LineBreak,
                Span::Text("\t".into()),
                Span::Text("end".into()),
            ]
        );
    }

    #[test]
    fn nested_lists_keep_their_shape() {
        assert_eq!(
            blocks_of(
                r#"<text:list>
                     <text:list-item><text:p>one</text:p>
                       <text:list><text:list-item><text:p>inner</text:p></text:list-item></text:list>
                     </text:list-item>
                     <text:list-item><text:p>two</text:p></text:list-item>
                   </text:list>"#
            ),
            vec![Block::List {
                ordered: false,
                items: vec![
                    vec![
                        Block::paragraph("one"),
                        Block::List {
                            ordered: false,
                            items: vec![vec![Block::paragraph("inner")]],
                        },
                    ],
                    vec![Block::paragraph("two")],
                ],
            }]
        );
    }

    #[test]
    fn a_numbered_list_style_makes_an_ordered_list() {
        let source = text_document(
            r#"<text:list text:style-name="L1"><text:list-item><text:p>one</text:p></text:list-item></text:list>
               <text:list text:style-name="L2"><text:list-item><text:p>two</text:p></text:list-item></text:list>"#,
            r#"<text:list-style style:name="L1"><text:list-level-style-number text:level="1"/></text:list-style>
               <text:list-style style:name="L2"><text:list-level-style-bullet text:level="1"/></text:list-style>"#,
        );

        let parsed = parse(&source).unwrap();
        assert_eq!(
            parsed,
            vec![
                Block::List {
                    ordered: true,
                    items: vec![vec![Block::paragraph("one")]],
                },
                Block::List {
                    ordered: false,
                    items: vec![vec![Block::paragraph("two")]],
                },
            ]
        );
    }

    #[test]
    fn tables_use_the_first_row_as_the_header() {
        assert_eq!(
            blocks_of(
                r#"<table:table>
                     <table:table-row><table:table-cell><text:p>a</text:p></table:table-cell><table:table-cell><text:p>b</text:p></table:table-cell></table:table-row>
                     <table:table-row><table:table-cell><text:p>1</text:p></table:table-cell><table:table-cell><text:p>2</text:p></table:table-cell></table:table-row>
                   </table:table>"#
            ),
            vec![Block::Table {
                header: Some(Row::from_texts(["a", "b"])),
                rows: vec![Row::from_texts(["1", "2"])],
            }]
        );
    }

    #[test]
    fn images_keep_their_description() {
        assert_eq!(
            blocks_of(
                r#"<text:p><draw:frame draw:name="Image1"><draw:image xlink:href="pic.png"/><svg:desc>a chart</svg:desc></draw:frame></text:p>"#
            ),
            vec![Block::Image {
                alt: Some("a chart".into())
            }]
        );
    }

    #[test]
    fn presentations_become_sections() {
        let source = r#"<office:document-content xmlns:office="o" xmlns:draw="d" xmlns:text="t">
              <office:body><office:presentation>
                <draw:page draw:name="Intro">
                  <draw:frame><draw:text-box><text:p>Agenda</text:p><text:p>first point</text:p></draw:text-box></draw:frame>
                </draw:page>
                <draw:page draw:name="Second">
                  <draw:frame><draw:text-box><text:p>Numbers</text:p></draw:text-box></draw:frame>
                </draw:page>
              </office:presentation></office:body>
            </office:document-content>"#;

        assert_eq!(
            parse(source).unwrap(),
            vec![
                Block::Heading {
                    level: 2,
                    text: Inline::text("Agenda")
                },
                Block::paragraph("first point"),
                Block::PageBreak { page: 2 },
                Block::Heading {
                    level: 2,
                    text: Inline::text("Numbers")
                },
            ]
        );
    }

    #[test]
    fn reads_metadata() {
        let meta = metadata(Some(
            r#"<office:document-meta xmlns:office="o" xmlns:dc="d" xmlns:meta="m">
                 <office:meta>
                   <dc:title>A title</dc:title>
                   <dc:creator>Ada</dc:creator>
                   <meta:creation-date>2026-07-24T10:00:00</meta:creation-date>
                 </office:meta>
               </office:document-meta>"#,
        ));
        assert_eq!(meta.title.as_deref(), Some("A title"));
        assert_eq!(meta.author.as_deref(), Some("Ada"));
        assert_eq!(meta.created.as_deref(), Some("2026-07-24T10:00:00"));
        assert_eq!(metadata(None), Metadata::default());
    }

    #[test]
    fn content_without_a_body_is_not_an_error() {
        assert!(
            parse(r#"<office:document-content xmlns:office="o"/>"#)
                .unwrap()
                .is_empty()
        );
    }
}
