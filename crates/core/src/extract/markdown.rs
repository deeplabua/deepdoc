//! Markdown extractor — normalises Markdown through the `Document` model.
//!
//! Round-tripping Markdown may look pointless, but it is what makes `deepdoc`
//! usable on mixed folders: the same normalisation, escaping and chunking apply
//! whatever the input was.

use std::path::Path;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::detect::{Format, Sniff};
use crate::error::{Error, Result};
use crate::extract::{ExtractOpts, Extractor};
use crate::model::{Block, Document, Inline, Metadata, Row, Span};

pub struct MarkdownExtractor;

impl Extractor for MarkdownExtractor {
    fn name(&self) -> &'static str {
        "markdown"
    }

    fn supports(&self, _path: &Path, sniff: &Sniff) -> bool {
        crate::detect::detect(sniff) == Some(Format::Markdown)
    }

    fn extract(&self, path: &Path, _opts: &ExtractOpts) -> Result<Document> {
        let raw = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        let text = String::from_utf8_lossy(&raw);

        let mut doc = Document {
            meta: Metadata {
                source_format: Some(Format::Markdown),
                source_path: Some(path.display().to_string()),
                ..Metadata::default()
            },
            blocks: parse(&text),
        };

        // A leading level-1 heading is the document's title, by convention.
        if let Some(Block::Heading { level: 1, text }) = doc.blocks.first() {
            doc.meta.title = Some(text.plain_text());
        }
        Ok(doc)
    }
}

/// Parse Markdown into blocks. Pure.
pub fn parse(text: &str) -> Vec<Block> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut builder = Builder::default();
    for event in Parser::new_ext(text, options) {
        builder.event(event);
    }
    builder.finish()
}

/// What the parser is in the middle of building.
#[derive(Default)]
struct Builder {
    /// Finished blocks per open container: index 0 is the document itself,
    /// deeper entries are list items and table cells.
    blocks: Vec<Vec<Block>>,
    /// Inline spans per open inline scope (paragraph, emphasis, link, …).
    spans: Vec<Vec<Span>>,
    lists: Vec<ListFrame>,
    links: Vec<String>,
    tables: Vec<TableFrame>,
    code: Option<CodeFrame>,
    /// Images seen inside the current paragraph; emitted as blocks after it.
    images: Vec<Block>,
}

struct ListFrame {
    ordered: bool,
    items: Vec<Vec<Block>>,
}

#[derive(Default)]
struct TableFrame {
    header: Option<Row>,
    rows: Vec<Row>,
    cells: Vec<Inline>,
    in_head: bool,
}

struct CodeFrame {
    lang: Option<String>,
    text: String,
}

impl Builder {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match &mut self.code {
                Some(code) => code.text.push_str(&text),
                None => self.push_span(Span::Text(text.to_string())),
            },
            Event::Code(text) => self.push_span(Span::Code(text.to_string())),
            Event::SoftBreak => self.push_span(Span::Text(" ".into())),
            Event::HardBreak => self.push_span(Span::LineBreak),
            Event::TaskListMarker(done) => {
                let marker = if done { "[x] " } else { "[ ] " };
                self.push_span(Span::Text(marker.into()));
            }
            // Raw HTML, footnote references, rules and math have no place in
            // the model — dropping them keeps the Markdown clean.
            Event::Html(_)
            | Event::InlineHtml(_)
            | Event::FootnoteReference(_)
            | Event::Rule
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        // A block opening closes whatever loose inline content preceded it —
        // in a tight nested list, `one` arrives before the inner list starts.
        if is_block_start(&tag) {
            self.flush_loose_inline();
        }

        match tag {
            Tag::Paragraph | Tag::Heading { .. } | Tag::TableCell => self.spans.push(Vec::new()),
            Tag::Emphasis | Tag::Strong | Tag::Strikethrough => self.spans.push(Vec::new()),
            Tag::Link { dest_url, .. } => {
                self.links.push(dest_url.to_string());
                self.spans.push(Vec::new());
            }
            Tag::Image { .. } => self.spans.push(Vec::new()),
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                        Some(lang.split_whitespace().next().unwrap_or("").to_string())
                    }
                    _ => None,
                };
                self.code = Some(CodeFrame {
                    lang,
                    text: String::new(),
                });
            }
            Tag::List(start) => self.lists.push(ListFrame {
                ordered: start.is_some(),
                items: Vec::new(),
            }),
            Tag::Item => self.blocks.push(Vec::new()),
            Tag::Table(_) => self.tables.push(TableFrame::default()),
            Tag::TableHead => {
                if let Some(table) = self.tables.last_mut() {
                    table.in_head = true;
                }
            }
            // Block quotes lose their marker: the model has no quote block, and
            // the text matters more than the decoration for extraction.
            Tag::BlockQuote(_)
            | Tag::TableRow
            | Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                let spans = self.spans.pop().unwrap_or_default();
                if !Inline::new(spans.clone()).plain_text().trim().is_empty() {
                    self.push_block(Block::Paragraph {
                        text: Inline::new(spans),
                    });
                }
                for image in std::mem::take(&mut self.images) {
                    self.push_block(image);
                }
            }
            TagEnd::Heading(level) => {
                let spans = self.spans.pop().unwrap_or_default();
                self.push_block(Block::Heading {
                    level: heading_level(level),
                    text: Inline::new(spans),
                });
            }
            TagEnd::Emphasis => self.wrap_spans(Span::Italic),
            TagEnd::Strong => self.wrap_spans(Span::Bold),
            // No strikethrough in the model — keep the words, drop the styling.
            TagEnd::Strikethrough => {
                let spans = self.spans.pop().unwrap_or_default();
                for span in spans {
                    self.push_span(span);
                }
            }
            TagEnd::Link => {
                let spans = self.spans.pop().unwrap_or_default();
                let href = self.links.pop().unwrap_or_default();
                self.push_span(Span::Link { href, text: spans });
            }
            TagEnd::Image => {
                let alt = Inline::new(self.spans.pop().unwrap_or_default()).plain_text();
                self.images.push(Block::Image {
                    alt: (!alt.trim().is_empty()).then(|| alt.trim().to_string()),
                });
            }
            TagEnd::CodeBlock => {
                if let Some(code) = self.code.take() {
                    self.push_block(Block::Code {
                        lang: code.lang,
                        text: code.text.trim_end().to_string(),
                    });
                }
            }
            TagEnd::List(_) => {
                if let Some(list) = self.lists.pop() {
                    self.push_block(Block::List {
                        ordered: list.ordered,
                        items: list.items,
                    });
                }
            }
            TagEnd::Item => {
                self.flush_loose_inline();
                let blocks = self.blocks.pop().unwrap_or_default();
                if let Some(list) = self.lists.last_mut() {
                    list.items.push(blocks);
                }
            }
            TagEnd::TableCell => {
                let spans = self.spans.pop().unwrap_or_default();
                if let Some(table) = self.tables.last_mut() {
                    table.cells.push(Inline::new(spans));
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = self.tables.last_mut() {
                    table.header = Some(Row::new(std::mem::take(&mut table.cells)));
                    table.in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = self.tables.last_mut() {
                    let cells = std::mem::take(&mut table.cells);
                    table.rows.push(Row::new(cells));
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.tables.pop() {
                    self.push_block(Block::Table {
                        header: table.header,
                        rows: table.rows,
                    });
                }
            }
            TagEnd::BlockQuote(_)
            | TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
        }
    }

    /// Close the innermost inline scope and wrap it in `span`.
    fn wrap_spans(&mut self, span: fn(Vec<Span>) -> Span) {
        let spans = self.spans.pop().unwrap_or_default();
        self.push_span(span(spans));
    }

    fn push_span(&mut self, span: Span) {
        match self.spans.last_mut() {
            Some(spans) => spans.push(span),
            // Inline content outside any block (loose text in a table row, say)
            // still deserves a home.
            None => {
                self.spans.push(vec![span]);
            }
        }
    }

    fn push_block(&mut self, block: Block) {
        self.flush_loose_inline();
        self.push_block_raw(block);
    }

    fn push_block_raw(&mut self, block: Block) {
        match self.blocks.last_mut() {
            Some(blocks) => blocks.push(block),
            None => self.blocks.push(vec![block]),
        }
    }

    /// Turn inline content that never got a `Paragraph` tag into one.
    ///
    /// A tight list (`- one`) emits `Start(Item)`, `Text`, `End(Item)` with no
    /// paragraph in between, so the text would otherwise be dropped.
    fn flush_loose_inline(&mut self) {
        if self.spans.len() != 1 {
            return;
        }
        let spans = self.spans.pop().unwrap_or_default();
        if !Inline::new(spans.clone()).plain_text().trim().is_empty() {
            self.push_block_raw(Block::Paragraph {
                text: Inline::new(spans),
            });
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush_loose_inline();
        // Flatten anything still open — malformed input should not lose text.
        while self.blocks.len() > 1 {
            let blocks = self.blocks.pop().unwrap_or_default();
            if let Some(parent) = self.blocks.last_mut() {
                parent.extend(blocks);
            }
        }
        self.blocks.pop().unwrap_or_default()
    }
}

/// Tags that open a block-level container.
fn is_block_start(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::List(_)
            | Tag::Item
            | Tag::CodeBlock(_)
            | Tag::Table(_)
            | Tag::BlockQuote(_)
            | Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
    )
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{RenderOpts, to_markdown};

    fn round_trip(markdown: &str) -> String {
        let doc = Document {
            meta: Metadata::default(),
            blocks: parse(markdown),
        };
        to_markdown(&doc, &RenderOpts::default())
    }

    #[test]
    fn parses_headings_and_paragraphs() {
        let blocks = parse("# Title\n\nSome text.\n");
        assert_eq!(
            blocks,
            vec![
                Block::Heading {
                    level: 1,
                    text: Inline::text("Title")
                },
                Block::paragraph("Some text."),
            ]
        );
    }

    #[test]
    fn parses_inline_markup() {
        let blocks = parse("**bold** *italic* `code` [site](https://example.com)");
        let Block::Paragraph { text } = &blocks[0] else {
            panic!("expected a paragraph, got {blocks:?}");
        };
        assert_eq!(
            text.spans,
            vec![
                Span::Bold(vec![Span::Text("bold".into())]),
                Span::Text(" ".into()),
                Span::Italic(vec![Span::Text("italic".into())]),
                Span::Text(" ".into()),
                Span::Code("code".into()),
                Span::Text(" ".into()),
                Span::Link {
                    href: "https://example.com".into(),
                    text: vec![Span::Text("site".into())],
                },
            ]
        );
    }

    #[test]
    fn parses_nested_lists() {
        let blocks = parse("- one\n  1. inner\n- two\n");
        assert_eq!(
            blocks,
            vec![Block::List {
                ordered: false,
                items: vec![
                    vec![
                        Block::paragraph("one"),
                        Block::List {
                            ordered: true,
                            items: vec![vec![Block::paragraph("inner")]],
                        },
                    ],
                    vec![Block::paragraph("two")],
                ],
            }]
        );
    }

    #[test]
    fn parses_tables() {
        let blocks = parse("| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        assert_eq!(
            blocks,
            vec![Block::Table {
                header: Some(Row::from_texts(["a", "b"])),
                rows: vec![Row::from_texts(["1", "2"])],
            }]
        );
    }

    #[test]
    fn parses_fenced_code_with_a_language() {
        let blocks = parse("```rust\nfn main() {}\n```\n");
        assert_eq!(
            blocks,
            vec![Block::Code {
                lang: Some("rust".into()),
                text: "fn main() {}".into(),
            }]
        );
    }

    #[test]
    fn images_become_blocks_and_html_is_dropped() {
        let blocks = parse("![a diagram](diagram.png)\n\n<div>raw</div>\n");
        assert_eq!(
            blocks,
            vec![Block::Image {
                alt: Some("a diagram".into())
            }]
        );
    }

    #[test]
    fn block_quotes_keep_their_text() {
        assert_eq!(round_trip("> quoted\n"), "quoted\n");
    }

    #[test]
    fn markdown_survives_a_round_trip() {
        let source = "# Title\n\n\
                      Text with **bold** and a [link](https://example.com).\n\n\
                      - one\n\
                      - two\n\n\
                      | a   | b   |\n\
                      | --- | --- |\n\
                      | 1   | 2   |\n\n\
                      ```rust\n\
                      fn main() {}\n\
                      ```\n";
        assert_eq!(round_trip(source), source);
    }
}
