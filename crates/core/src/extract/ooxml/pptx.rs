//! PowerPoint (`.pptx`) extractor.
//!
//! A deck is a sequence of slides, so the output is a sequence of sections: the
//! slide's title placeholder becomes the heading (or `Slide N` when it has
//! none), body placeholders with several paragraphs become lists — that is what
//! a bulleted text frame actually is — and tables come through as tables.
//! Speaker notes live in a separate part and are left out.

use std::collections::HashMap;
use std::path::Path;

use crate::detect::{Format, Sniff};
use crate::error::{Error, Result};
use crate::extract::container::Container;
use crate::extract::ooxml::relationships;
use crate::extract::xml::{self, Element};
use crate::extract::{ExtractOpts, Extractor};
use crate::model::{Block, Document, Inline, Row, Span};

pub struct PptxExtractor;

impl Extractor for PptxExtractor {
    fn name(&self) -> &'static str {
        "pptx"
    }

    fn supports(&self, _path: &Path, sniff: &Sniff) -> bool {
        crate::detect::detect(sniff) == Some(Format::Pptx)
    }

    fn extract(&self, path: &Path, _opts: &ExtractOpts) -> Result<Document> {
        let mut container = Container::open(path)?;

        let slide_names: Vec<String> = container
            .names_under("ppt/slides/slide")
            .into_iter()
            .filter(|name| name.ends_with(".xml"))
            .collect();

        let mut blocks = Vec::new();
        for (index, name) in slide_names.iter().enumerate() {
            let source = container
                .read(name)
                .map_err(|message| Error::parse(path, message))?;
            let rels = rels_name(name).and_then(|name| container.read_optional(&name));
            let links = relationships(rels.as_deref());

            blocks.extend(
                parse_slide(&source, index + 1, &links).map_err(|m| Error::parse(path, m))?,
            );
        }

        let mut meta = super::core_properties(&mut container);
        meta.source_format = Some(Format::Pptx);
        meta.source_path = Some(path.display().to_string());
        meta.page_count = Some(slide_names.len() as u32);

        Ok(Document { meta, blocks })
    }
}

/// `ppt/slides/slide1.xml` → `ppt/slides/_rels/slide1.xml.rels`.
fn rels_name(slide: &str) -> Option<String> {
    let (directory, file) = slide.rsplit_once('/')?;
    Some(format!("{directory}/_rels/{file}.rels"))
}

/// Parse one slide into blocks. Pure.
pub fn parse_slide(
    source: &str,
    number: usize,
    links: &HashMap<String, String>,
) -> std::result::Result<Vec<Block>, String> {
    let root = xml::parse(source)?;

    let mut heading: Option<Inline> = None;
    let mut body = Vec::new();

    // Slides after the first start a new page in the output.
    let mut blocks = if number > 1 {
        vec![Block::PageBreak {
            page: number as u32,
        }]
    } else {
        Vec::new()
    };

    for shape in root.find_all("sp") {
        let Some(frame) = shape.find("txBody") else {
            continue;
        };
        let paragraphs = text_paragraphs(frame, links);
        if paragraphs.is_empty() {
            continue;
        }

        if heading.is_none() && is_title(shape) {
            heading = paragraphs.first().map(|(_, inline)| inline.clone());
            body.extend(paragraphs.into_iter().skip(1));
        } else {
            body.extend(paragraphs);
        }
    }

    blocks.push(Block::Heading {
        level: 2,
        text: heading.unwrap_or_else(|| Inline::text(format!("Slide {number}"))),
    });

    // One paragraph is a line of text; several are the deck's bullet list.
    if body.len() > 1 {
        blocks.push(nest(&body));
    } else {
        blocks.extend(body.into_iter().map(|(_, text)| Block::Paragraph { text }));
    }

    for table in root.find_all("tbl") {
        if let Some(table) = table_block(table, links) {
            blocks.push(table);
        }
    }

    Ok(blocks)
}

/// Whether a shape is the slide's title placeholder.
fn is_title(shape: &Element) -> bool {
    shape
        .find("ph")
        .and_then(|placeholder| placeholder.attr("type"))
        .is_some_and(|kind| matches!(kind, "title" | "ctrTitle"))
}

/// The paragraphs of a text frame, with their indent level.
fn text_paragraphs(frame: &Element, links: &HashMap<String, String>) -> Vec<(u8, Inline)> {
    frame
        .find_all("p")
        .into_iter()
        .filter_map(|paragraph| {
            let level = paragraph
                .child("pPr")
                .and_then(|properties| properties.attr_number("lvl"))
                .unwrap_or(0)
                .clamp(0, 8) as u8;
            let inline = paragraph_inline(paragraph, links);
            (!inline.plain_text().trim().is_empty()).then_some((level, inline))
        })
        .collect()
}

fn paragraph_inline(paragraph: &Element, links: &HashMap<String, String>) -> Inline {
    let mut spans = Vec::new();

    for child in paragraph.elements() {
        match child.name.as_str() {
            "r" => {
                let properties = child.child("rPr");
                let bold = toggle_attr(properties, "b");
                let italic = toggle_attr(properties, "i");
                let text = child.child("t").map(Element::text).unwrap_or_default();
                if text.is_empty() {
                    continue;
                }

                let inner = vec![Span::Text(text)];
                let styled = match (bold, italic) {
                    (false, false) => inner,
                    (true, false) => vec![Span::Bold(inner)],
                    (false, true) => vec![Span::Italic(inner)],
                    (true, true) => vec![Span::Bold(vec![Span::Italic(inner)])],
                };

                // `hlinkClick` names a relationship in the slide's rels part.
                match properties
                    .and_then(|p| p.child("hlinkClick"))
                    .and_then(|link| link.attr("id"))
                    .and_then(|id| links.get(id))
                {
                    Some(href) => spans.push(Span::Link {
                        href: href.clone(),
                        text: styled,
                    }),
                    None => spans.extend(styled),
                }
            }
            "br" => spans.push(Span::LineBreak),
            "fld" => {
                let text = child.child("t").map(Element::text).unwrap_or_default();
                if !text.is_empty() {
                    spans.push(Span::Text(text));
                }
            }
            _ => {}
        }
    }

    Inline::new(spans)
}

/// DrawingML writes booleans as attributes (`b="1"`), not child elements.
fn toggle_attr(properties: Option<&Element>, name: &str) -> bool {
    match properties.and_then(|p| p.attr(name)) {
        Some(value) => !matches!(value, "0" | "false"),
        None => false,
    }
}

/// Build nested lists from paragraphs carrying their indent level.
fn nest(items: &[(u8, Inline)]) -> Block {
    let mut list_items: Vec<Vec<Block>> = Vec::new();

    let mut index = 0;
    while index < items.len() {
        let (level, inline) = &items[index];
        let mut blocks = vec![Block::Paragraph {
            text: inline.clone(),
        }];

        let children_start = index + 1;
        let mut children_end = children_start;
        while children_end < items.len() && items[children_end].0 > *level {
            children_end += 1;
        }
        if children_end > children_start {
            blocks.push(nest(&items[children_start..children_end]));
        }

        list_items.push(blocks);
        index = children_end;
    }

    Block::List {
        ordered: false,
        items: list_items,
    }
}

fn table_block(table: &Element, links: &HashMap<String, String>) -> Option<Block> {
    let mut rows: Vec<Row> = Vec::new();

    for row in table.find_all("tr") {
        let cells: Vec<Inline> = row
            .find_all("tc")
            .into_iter()
            .map(|cell| {
                let text = text_paragraphs(cell, links)
                    .into_iter()
                    .map(|(_, inline)| inline.plain_text())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn slide(shapes: &str) -> String {
        format!(
            r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree>{shapes}</p:spTree></p:cSld></p:sld>"#
        )
    }

    fn title_shape(text: &str) -> String {
        format!(
            r#"<p:sp><p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
               <p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"#
        )
    }

    #[test]
    fn the_title_placeholder_becomes_the_heading() {
        let blocks = parse_slide(&slide(&title_shape("Agenda")), 1, &HashMap::new()).unwrap();
        assert_eq!(
            blocks,
            vec![Block::Heading {
                level: 2,
                text: Inline::text("Agenda")
            }]
        );
    }

    #[test]
    fn a_slide_without_a_title_is_numbered() {
        let blocks = parse_slide(&slide(""), 3, &HashMap::new()).unwrap();
        assert_eq!(
            blocks,
            vec![
                Block::PageBreak { page: 3 },
                Block::Heading {
                    level: 2,
                    text: Inline::text("Slide 3")
                },
            ]
        );
    }

    #[test]
    fn body_paragraphs_become_a_nested_list() {
        let body = r#"<p:sp><p:txBody>
              <a:p><a:r><a:t>first</a:t></a:r></a:p>
              <a:p><a:pPr lvl="1"/><a:r><a:t>deeper</a:t></a:r></a:p>
              <a:p><a:r><a:t>second</a:t></a:r></a:p>
            </p:txBody></p:sp>"#;
        let blocks = parse_slide(
            &slide(&format!("{}{body}", title_shape("T"))),
            1,
            &HashMap::new(),
        )
        .unwrap();

        assert_eq!(
            blocks[1],
            Block::List {
                ordered: false,
                items: vec![
                    vec![
                        Block::paragraph("first"),
                        Block::List {
                            ordered: false,
                            items: vec![vec![Block::paragraph("deeper")]],
                        },
                    ],
                    vec![Block::paragraph("second")],
                ],
            }
        );
    }

    #[test]
    fn a_single_body_paragraph_stays_a_paragraph() {
        let body =
            r#"<p:sp><p:txBody><a:p><a:r><a:t>just text</a:t></a:r></a:p></p:txBody></p:sp>"#;
        let blocks = parse_slide(
            &slide(&format!("{}{body}", title_shape("T"))),
            1,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(blocks[1], Block::paragraph("just text"));
    }

    #[test]
    fn runs_carry_drawingml_styling() {
        let body = r#"<p:sp><p:txBody><a:p>
              <a:r><a:rPr b="1"/><a:t>bold</a:t></a:r>
              <a:r><a:rPr i="1"/><a:t> italic</a:t></a:r>
            </a:p></p:txBody></p:sp>"#;
        let blocks = parse_slide(&slide(body), 1, &HashMap::new()).unwrap();

        let Block::Paragraph { text } = &blocks[1] else {
            panic!("expected a paragraph, got {blocks:?}");
        };
        assert_eq!(
            text.spans,
            vec![
                Span::Bold(vec![Span::Text("bold".into())]),
                Span::Italic(vec![Span::Text(" italic".into())]),
            ]
        );
    }

    #[test]
    fn tables_come_through() {
        let table = r#"<p:graphicFrame><a:graphic><a:graphicData><a:tbl>
              <a:tr><a:tc><a:txBody><a:p><a:r><a:t>a</a:t></a:r></a:p></a:txBody></a:tc>
                    <a:tc><a:txBody><a:p><a:r><a:t>b</a:t></a:r></a:p></a:txBody></a:tc></a:tr>
              <a:tr><a:tc><a:txBody><a:p><a:r><a:t>1</a:t></a:r></a:p></a:txBody></a:tc>
                    <a:tc><a:txBody><a:p><a:r><a:t>2</a:t></a:r></a:p></a:txBody></a:tc></a:tr>
            </a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#;
        let blocks = parse_slide(
            &slide(&format!("{}{table}", title_shape("T"))),
            1,
            &HashMap::new(),
        )
        .unwrap();

        assert_eq!(
            blocks.last(),
            Some(&Block::Table {
                header: Some(Row::from_texts(["a", "b"])),
                rows: vec![Row::from_texts(["1", "2"])],
            })
        );
    }
}
