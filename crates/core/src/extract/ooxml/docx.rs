//! Word (`.docx`) extractor.
//!
//! What is read: paragraph styles (`Heading1`…`Heading9`, `Title`, plus
//! `outlineLvl` as a fallback) become headings, numbered paragraphs (`numPr`)
//! become nested lists with the ordered/bullet choice taken from
//! `numbering.xml`, `w:tbl` becomes a table, runs carry bold and italic, and
//! hyperlinks resolve through the part's relationships.

use std::collections::HashMap;
use std::path::Path;

use crate::detect::{Format, Sniff};
use crate::error::{Error, Result};
use crate::extract::container::Container;
use crate::extract::ooxml::{relationships, toggle};
use crate::extract::xml::{self, Element};
use crate::extract::{ExtractOpts, Extractor};
use crate::model::{Block, Document, Inline, Row, Span};

pub struct DocxExtractor;

impl Extractor for DocxExtractor {
    fn name(&self) -> &'static str {
        "docx"
    }

    fn supports(&self, _path: &Path, sniff: &Sniff) -> bool {
        crate::detect::detect(sniff) == Some(Format::Docx)
    }

    fn extract(&self, path: &Path, _opts: &ExtractOpts) -> Result<Document> {
        let mut container = Container::open(path)?;

        let document = container
            .read("word/document.xml")
            .map_err(|message| Error::parse(path, message))?;
        let numbering = container.read_optional("word/numbering.xml");
        let rels = container.read_optional("word/_rels/document.xml.rels");

        let blocks = parse(&document, numbering.as_deref(), rels.as_deref())
            .map_err(|message| Error::parse(path, message))?;

        let mut meta = super::core_properties(&mut container);
        meta.source_format = Some(Format::Docx);
        meta.source_path = Some(path.display().to_string());

        Ok(Document { meta, blocks })
    }
}

/// Parse `word/document.xml` into blocks. Pure.
pub fn parse(
    document: &str,
    numbering: Option<&str>,
    rels: Option<&str>,
) -> std::result::Result<Vec<Block>, String> {
    let root = xml::parse(document)?;
    let body = root.child("body").unwrap_or(&root);
    let context = Context {
        numbering: NumberingTable::parse(numbering),
        links: relationships(rels),
    };

    Ok(context.blocks(body))
}

struct Context {
    numbering: NumberingTable,
    links: HashMap<String, String>,
}

/// A paragraph that belongs to a list, before nesting is worked out.
struct ListItem {
    level: u8,
    ordered: bool,
    blocks: Vec<Block>,
}

impl Context {
    /// Walk a block container (the body, or a table cell).
    fn blocks(&self, container: &Element) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut pending: Vec<ListItem> = Vec::new();

        for child in container.elements() {
            match child.name.as_str() {
                "p" => match self.list_item(child) {
                    Some(item) => pending.push(item),
                    None => {
                        flush_list(&mut pending, &mut blocks);
                        blocks.extend(self.paragraph(child));
                    }
                },
                "tbl" => {
                    flush_list(&mut pending, &mut blocks);
                    if let Some(table) = self.table(child) {
                        blocks.push(table);
                    }
                }
                // sectPr, bookmarks, proofing marks and the like carry no text.
                _ => {}
            }
        }

        flush_list(&mut pending, &mut blocks);
        blocks
    }

    /// A numbered or bulleted paragraph, if this paragraph is one.
    fn list_item(&self, paragraph: &Element) -> Option<ListItem> {
        let properties = paragraph.child("pPr")?;
        let numbering = properties.child("numPr")?;

        let level = numbering
            .child("ilvl")
            .and_then(|ilvl| ilvl.attr_number("val"))
            .unwrap_or(0)
            .clamp(0, 8) as u8;
        let id = numbering
            .child("numId")
            .and_then(|num| num.attr_number("val"))
            .unwrap_or(0);

        Some(ListItem {
            level,
            ordered: self.numbering.is_ordered(id, level),
            blocks: self.paragraph(paragraph),
        })
    }

    /// A paragraph becomes a heading or a paragraph, plus any images in it.
    fn paragraph(&self, paragraph: &Element) -> Vec<Block> {
        let (inline, images) = self.inline(paragraph);

        let mut blocks = Vec::new();
        if !inline.plain_text().trim().is_empty() {
            blocks.push(match heading_level(paragraph) {
                Some(level) => Block::Heading {
                    level,
                    text: inline,
                },
                None => Block::Paragraph { text: inline },
            });
        }
        blocks.extend(images);
        blocks
    }

    /// Collect the inline content of a paragraph, and any images alongside it.
    fn inline(&self, paragraph: &Element) -> (Inline, Vec<Block>) {
        let mut spans = Vec::new();
        let mut images = Vec::new();
        self.walk_inline(paragraph, &mut spans, &mut images);
        (Inline::new(spans), images)
    }

    fn walk_inline(&self, element: &Element, spans: &mut Vec<Span>, images: &mut Vec<Block>) {
        for child in element.elements() {
            match child.name.as_str() {
                "r" => self.run(child, spans, images),
                "hyperlink" => {
                    let mut inner = Vec::new();
                    self.walk_inline(child, &mut inner, images);
                    match child.attr("id").and_then(|id| self.links.get(id)) {
                        Some(href) if !inner.is_empty() => spans.push(Span::Link {
                            href: href.clone(),
                            text: inner,
                        }),
                        // An internal anchor has no URL worth keeping.
                        _ => spans.extend(inner),
                    }
                }
                // Content controls, revisions and smart tags wrap runs.
                "sdt" | "sdtContent" | "ins" | "smartTag" | "bookmarkStart" | "fldSimple" => {
                    self.walk_inline(child, spans, images);
                }
                // Deleted text must not come back.
                "del" => {}
                _ => {}
            }
        }
    }

    fn run(&self, run: &Element, spans: &mut Vec<Span>, images: &mut Vec<Block>) {
        let properties = run.child("rPr");
        let bold = toggle(properties.and_then(|p| p.child("b")));
        let italic = toggle(properties.and_then(|p| p.child("i")));

        let mut inner = Vec::new();
        for child in run.elements() {
            match child.name.as_str() {
                "t" => {
                    let text = child.text();
                    if !text.is_empty() {
                        inner.push(Span::Text(text));
                    }
                }
                "br" | "cr" => inner.push(Span::LineBreak),
                "tab" => inner.push(Span::Text("\t".into())),
                "noBreakHyphen" => inner.push(Span::Text("-".into())),
                "drawing" | "pict" | "object" => {
                    if let Some(image) = image_block(child) {
                        images.push(image);
                    }
                }
                _ => {}
            }
        }

        if inner.is_empty() {
            return;
        }
        spans.push(match (bold, italic) {
            (false, false) => {
                spans.extend(inner);
                return;
            }
            (true, false) => Span::Bold(inner),
            (false, true) => Span::Italic(inner),
            (true, true) => Span::Bold(vec![Span::Italic(inner)]),
        });
    }

    fn table(&self, table: &Element) -> Option<Block> {
        let mut rows: Vec<Row> = Vec::new();

        for row in table.find_all("tr") {
            let cells: Vec<Inline> = row
                .find_all("tc")
                .into_iter()
                .map(|cell| {
                    // A cell holds blocks; a Markdown cell holds one line.
                    let text = self
                        .blocks(cell)
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
        // Word documents put the header first; there is no reliable marker for
        // it, and a Markdown table needs one.
        let header = rows.remove(0);
        Some(Block::Table {
            header: Some(header),
            rows,
        })
    }
}

/// Turn the collected list paragraphs into nested list blocks.
fn flush_list(pending: &mut Vec<ListItem>, blocks: &mut Vec<Block>) {
    if pending.is_empty() {
        return;
    }
    let items = std::mem::take(pending);
    blocks.push(nest(&items));
}

/// Build a list from a flat run of items carrying their indent level.
fn nest(items: &[ListItem]) -> Block {
    let base = items[0].level;
    let ordered = items[0].ordered;
    let mut list_items: Vec<Vec<Block>> = Vec::new();

    let mut index = 0;
    while index < items.len() {
        let mut blocks = items[index].blocks.clone();

        // Everything deeper than this item belongs inside it.
        let children_start = index + 1;
        let mut children_end = children_start;
        while children_end < items.len() && items[children_end].level > base {
            children_end += 1;
        }
        if children_end > children_start {
            blocks.push(nest(&items[children_start..children_end]));
        }

        list_items.push(blocks);
        index = children_end;
    }

    Block::List {
        ordered,
        items: list_items,
    }
}

/// `Heading2` → 2, `Title` → 1, `outlineLvl 0` → 1.
fn heading_level(paragraph: &Element) -> Option<u8> {
    let properties = paragraph.child("pPr")?;

    if let Some(style) = properties.child("pStyle").and_then(|s| s.attr("val")) {
        let lowered = style.to_ascii_lowercase();
        if let Some(rest) = lowered.strip_prefix("heading")
            && let Ok(level) = rest.trim_start_matches('-').parse::<u8>()
            && level >= 1
        {
            return Some(level.min(6));
        }
        match lowered.as_str() {
            "title" => return Some(1),
            "subtitle" => return Some(2),
            _ => {}
        }
    }

    let outline = properties.child("outlineLvl")?.attr_number("val")?;
    (0..=8)
        .contains(&outline)
        .then(|| (outline as u8 + 1).min(6))
}

/// A drawing's alt text, from `wp:docPr`.
fn image_block(drawing: &Element) -> Option<Block> {
    let properties = drawing.find("docPr")?;
    let alt = properties
        .attr("descr")
        .or_else(|| properties.attr("name"))
        .map(str::trim)
        .filter(|alt| !alt.is_empty())
        .map(str::to_string);
    Some(Block::Image { alt })
}

/// Which `numId`/level combinations are ordered rather than bulleted.
#[derive(Default)]
struct NumberingTable {
    /// (numId, level) → ordered
    formats: HashMap<(i64, u8), bool>,
}

impl NumberingTable {
    fn parse(source: Option<&str>) -> NumberingTable {
        let Some(root) = source.and_then(|source| xml::parse(source).ok()) else {
            return NumberingTable::default();
        };

        // abstractNumId → level → ordered
        let mut abstracts: HashMap<i64, HashMap<u8, bool>> = HashMap::new();
        for definition in root.find_all("abstractNum") {
            let Some(id) = definition.attr_number("abstractNumId") else {
                continue;
            };
            let levels = abstracts.entry(id).or_default();
            for level in definition.find_all("lvl") {
                let index = level.attr_number("ilvl").unwrap_or(0).clamp(0, 8) as u8;
                let format = level
                    .child("numFmt")
                    .and_then(|fmt| fmt.attr("val"))
                    .unwrap_or("bullet");
                levels.insert(index, format != "bullet" && format != "none");
            }
        }

        let mut formats = HashMap::new();
        for num in root.find_all("num") {
            let Some(id) = num.attr_number("numId") else {
                continue;
            };
            let Some(abstract_id) = num
                .child("abstractNumId")
                .and_then(|element| element.attr_number("val"))
            else {
                continue;
            };
            if let Some(levels) = abstracts.get(&abstract_id) {
                for (level, ordered) in levels {
                    formats.insert((id, *level), *ordered);
                }
            }
        }

        NumberingTable { formats }
    }

    /// Bulleted unless the numbering table says otherwise — a missing or
    /// unreadable `numbering.xml` should not invent numbers.
    fn is_ordered(&self, id: i64, level: u8) -> bool {
        self.formats.get(&(id, level)).copied().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Metadata;
    use crate::render::{RenderOpts, to_markdown};

    fn document(body: &str) -> String {
        format!(r#"<w:document xmlns:w="w"><w:body>{body}</w:body></w:document>"#)
    }

    fn blocks(body: &str) -> Vec<Block> {
        parse(&document(body), None, None).expect("valid docx xml")
    }

    fn markdown(blocks: Vec<Block>) -> String {
        to_markdown(
            &Document {
                meta: Metadata::default(),
                blocks,
            },
            &RenderOpts::default(),
        )
    }

    const NUMBERING: &str = r#"<w:numbering xmlns:w="w">
        <w:abstractNum w:abstractNumId="0">
          <w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl>
          <w:lvl w:ilvl="1"><w:numFmt w:val="decimal"/></w:lvl>
        </w:abstractNum>
        <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
      </w:numbering>"#;

    #[test]
    fn paragraph_styles_become_headings() {
        assert_eq!(
            blocks(
                r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Summary</w:t></w:r></w:p>
                   <w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>Report</w:t></w:r></w:p>
                   <w:p><w:r><w:t>Body text.</w:t></w:r></w:p>"#
            ),
            vec![
                Block::Heading {
                    level: 2,
                    text: Inline::text("Summary")
                },
                Block::Heading {
                    level: 1,
                    text: Inline::text("Report")
                },
                Block::paragraph("Body text."),
            ]
        );
    }

    #[test]
    fn outline_level_is_the_heading_fallback() {
        assert_eq!(
            blocks(
                r#"<w:p><w:pPr><w:outlineLvl w:val="1"/></w:pPr><w:r><w:t>Deep</w:t></w:r></w:p>"#
            ),
            vec![Block::Heading {
                level: 2,
                text: Inline::text("Deep")
            }]
        );
    }

    #[test]
    fn runs_carry_bold_and_italic() {
        let Block::Paragraph { text } = &blocks(
            r#"<w:p>
                 <w:r><w:t xml:space="preserve">plain </w:t></w:r>
                 <w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r>
                 <w:r><w:rPr><w:i/><w:b w:val="0"/></w:rPr><w:t xml:space="preserve"> italic</w:t></w:r>
               </w:p>"#,
        )[0] else {
            panic!("expected a paragraph");
        };
        assert_eq!(
            text.spans,
            vec![
                Span::Text("plain ".into()),
                Span::Bold(vec![Span::Text("bold".into())]),
                Span::Italic(vec![Span::Text(" italic".into())]),
            ]
        );
    }

    #[test]
    fn line_breaks_and_tabs_survive_a_run() {
        let Block::Paragraph { text } =
            &blocks(r#"<w:p><w:r><w:t>a</w:t><w:br/><w:tab/><w:t>b</w:t></w:r></w:p>"#)[0]
        else {
            panic!("expected a paragraph");
        };
        assert_eq!(
            text.spans,
            vec![
                Span::Text("a".into()),
                Span::LineBreak,
                Span::Text("\t".into()),
                Span::Text("b".into()),
            ]
        );
    }

    #[test]
    fn hyperlinks_resolve_through_the_relationships_part() {
        let rels = r#"<Relationships><Relationship Id="rId4" Target="https://example.com"/></Relationships>"#;
        let parsed = parse(
            &document(
                r#"<w:p><w:hyperlink r:id="rId4"><w:r><w:t>site</w:t></w:r></w:hyperlink></w:p>"#,
            ),
            None,
            Some(rels),
        )
        .unwrap();

        let Block::Paragraph { text } = &parsed[0] else {
            panic!("expected a paragraph");
        };
        assert_eq!(
            text.spans,
            vec![Span::Link {
                href: "https://example.com".into(),
                text: vec![Span::Text("site".into())],
            }]
        );
    }

    #[test]
    fn an_unresolved_hyperlink_keeps_its_text() {
        let Block::Paragraph { text } = &blocks(
            r#"<w:p><w:hyperlink w:anchor="top"><w:r><w:t>jump</w:t></w:r></w:hyperlink></w:p>"#,
        )[0] else {
            panic!("expected a paragraph");
        };
        assert_eq!(text.spans, vec![Span::Text("jump".into())]);
    }

    #[test]
    fn deleted_text_stays_deleted() {
        assert_eq!(
            blocks(
                r#"<w:p><w:r><w:t>kept</w:t></w:r><w:del><w:r><w:delText>gone</w:delText></w:r></w:del></w:p>"#
            ),
            vec![Block::paragraph("kept")]
        );
    }

    #[test]
    fn numbered_paragraphs_become_nested_lists() {
        let parsed = parse(
            &document(
                r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>one</w:t></w:r></w:p>
                   <w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>inner</w:t></w:r></w:p>
                   <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>two</w:t></w:r></w:p>
                   <w:p><w:r><w:t>after</w:t></w:r></w:p>"#,
            ),
            Some(NUMBERING),
            None,
        )
        .unwrap();

        assert_eq!(
            parsed,
            vec![
                Block::List {
                    ordered: false,
                    items: vec![
                        vec![
                            Block::paragraph("one"),
                            // Level 1 of this numbering is decimal, not a bullet.
                            Block::List {
                                ordered: true,
                                items: vec![vec![Block::paragraph("inner")]],
                            },
                        ],
                        vec![Block::paragraph("two")],
                    ],
                },
                Block::paragraph("after"),
            ]
        );
    }

    #[test]
    fn lists_are_bulleted_without_a_numbering_part() {
        let parsed = blocks(
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="3"/></w:numPr></w:pPr><w:r><w:t>item</w:t></w:r></w:p>"#,
        );
        assert_eq!(
            parsed,
            vec![Block::List {
                ordered: false,
                items: vec![vec![Block::paragraph("item")]],
            }]
        );
    }

    #[test]
    fn tables_use_the_first_row_as_the_header() {
        assert_eq!(
            blocks(
                r#"<w:tbl>
                     <w:tr><w:tc><w:p><w:r><w:t>Segment</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Q1</w:t></w:r></w:p></w:tc></w:tr>
                     <w:tr><w:tc><w:p><w:r><w:t>Cloud</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>4.1</w:t></w:r></w:p></w:tc></w:tr>
                   </w:tbl>"#
            ),
            vec![Block::Table {
                header: Some(Row::from_texts(["Segment", "Q1"])),
                rows: vec![Row::from_texts(["Cloud", "4.1"])],
            }]
        );
    }

    #[test]
    fn images_keep_their_alt_text() {
        assert_eq!(
            blocks(
                r#"<w:p><w:r><w:drawing><wp:inline><wp:docPr id="1" name="Picture 1" descr="a chart"/></wp:inline></w:drawing></w:r></w:p>"#
            ),
            vec![Block::Image {
                alt: Some("a chart".into())
            }]
        );
    }

    #[test]
    fn renders_a_small_document() {
        let parsed = parse(
            &document(
                r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Report</w:t></w:r></w:p>
                   <w:p><w:r><w:t xml:space="preserve">Revenue grew </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>12%</w:t></w:r><w:r><w:t>.</w:t></w:r></w:p>
                   <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Cloud</w:t></w:r></w:p>"#,
            ),
            Some(NUMBERING),
            None,
        )
        .unwrap();

        assert_eq!(
            markdown(parsed),
            "# Report\n\nRevenue grew **12%**.\n\n- Cloud\n"
        );
    }

    #[test]
    fn a_document_without_a_body_is_not_an_error() {
        assert!(
            parse(r#"<w:document xmlns:w="w"/>"#, None, None)
                .unwrap()
                .is_empty()
        );
    }
}
