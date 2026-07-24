//! The `Document` IR — the format-neutral model every extractor produces and
//! every renderer consumes.
//!
//! Deliberately modest: the goal is clean Markdown, not pixel-perfect layout.
//! Anything that cannot be expressed here (fonts, colours, absolute positions)
//! is intentionally dropped during extraction.

use serde::{Deserialize, Serialize};

use crate::detect::Format;

/// A parsed document: metadata plus an ordered list of block-level elements.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub meta: Metadata,
    pub blocks: Vec<Block>,
}

impl Document {
    /// An empty document with the given metadata.
    pub fn new(meta: Metadata) -> Self {
        Self {
            meta,
            blocks: Vec::new(),
        }
    }

    /// True when the document carries no extractable text at all.
    ///
    /// Used by the "looks like a scan, needs `--ocr`" heuristic (exit code 4).
    pub fn is_empty(&self) -> bool {
        self.blocks.iter().all(|b| b.plain_text().trim().is_empty())
    }
}

/// Document-level metadata. Every field is optional — most formats supply only a subset.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Creation date as found in the source, ISO-8601 when the format allows it.
    /// Kept as a string on purpose: no date-time dependency in the core.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_format: Option<Format>,
    /// Path the document was read from, as given on the command line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

/// A block-level element.
///
/// `Paragraph` is a struct variant rather than the newtype the plan sketches:
/// serde's internally-tagged representation cannot tag a newtype wrapping a
/// sequence, and `Inline` is transparent over `Vec<Span>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Heading {
        level: u8,
        text: Inline,
    },
    Paragraph {
        text: Inline,
    },
    List {
        ordered: bool,
        items: Vec<Vec<Block>>,
    },
    Table {
        header: Option<Row>,
        rows: Vec<Row>,
    },
    Code {
        lang: Option<String>,
        text: String,
    },
    /// Images carry alt text only — DeepDoc never extracts pixels (that is DeepThumb).
    Image {
        alt: Option<String>,
    },
    /// Page boundary marker; `page` is the number of the page that starts here.
    PageBreak {
        page: u32,
    },
}

impl Block {
    /// A paragraph of unstyled text — convenience for extractors and tests.
    pub fn paragraph(text: impl Into<String>) -> Self {
        Block::Paragraph {
            text: Inline::text(text),
        }
    }

    /// The block's text with all markup removed.
    pub fn plain_text(&self) -> String {
        match self {
            Block::Heading { text, .. } | Block::Paragraph { text } => text.plain_text(),
            Block::List { items, .. } => items
                .iter()
                .flat_map(|item| item.iter().map(Block::plain_text))
                .collect::<Vec<_>>()
                .join("\n"),
            Block::Table { header, rows } => header
                .iter()
                .chain(rows.iter())
                .map(Row::plain_text)
                .collect::<Vec<_>>()
                .join("\n"),
            Block::Code { text, .. } => text.clone(),
            Block::Image { alt } => alt.clone().unwrap_or_default(),
            Block::PageBreak { .. } => String::new(),
        }
    }
}

/// A single table row.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub cells: Vec<Inline>,
}

impl Row {
    pub fn new(cells: Vec<Inline>) -> Self {
        Self { cells }
    }

    /// Build a row from plain strings — convenience for extractors and tests.
    pub fn from_texts<I, S>(cells: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::new(cells.into_iter().map(Inline::text).collect())
    }

    pub fn plain_text(&self) -> String {
        self.cells
            .iter()
            .map(Inline::plain_text)
            .collect::<Vec<_>>()
            .join("\t")
    }
}

/// Inline content: a sequence of spans.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Inline {
    pub spans: Vec<Span>,
}

impl Inline {
    pub fn new(spans: Vec<Span>) -> Self {
        Self { spans }
    }

    /// A single run of unstyled text.
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(vec![Span::Text(text.into())])
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The inline content with all markup removed.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for span in &self.spans {
            span.write_plain_text(&mut out);
        }
        out
    }
}

/// Inline markup. Kept minimal on purpose — bold, italic, code, links, breaks.
///
/// Adjacently tagged (`{"type": "text", "value": "…"}`) because the variants
/// wrap strings and sequences, which serde's internal tagging cannot carry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Span {
    Text(String),
    Bold(Vec<Span>),
    Italic(Vec<Span>),
    Code(String),
    Link { href: String, text: Vec<Span> },
    LineBreak,
}

impl Span {
    fn write_plain_text(&self, out: &mut String) {
        match self {
            Span::Text(text) | Span::Code(text) => out.push_str(text),
            Span::Bold(spans) | Span::Italic(spans) | Span::Link { text: spans, .. } => {
                for span in spans {
                    span.write_plain_text(out);
                }
            }
            Span::LineBreak => out.push('\n'),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_strips_nested_markup() {
        let inline = Inline::new(vec![
            Span::Text("a ".into()),
            Span::Bold(vec![Span::Italic(vec![Span::Text("b".into())])]),
            Span::Text(" ".into()),
            Span::Link {
                href: "https://example.com".into(),
                text: vec![Span::Text("c".into())],
            },
        ]);
        assert_eq!(inline.plain_text(), "a b c");
    }

    /// Every variant must survive a JSON round trip — serde's tagged
    /// representations reject some shapes only at runtime.
    #[test]
    fn every_block_round_trips_through_json() {
        let doc = Document {
            meta: Metadata {
                title: Some("Title".into()),
                page_count: Some(2),
                source_format: Some(Format::Pdf),
                ..Metadata::default()
            },
            blocks: vec![
                Block::Heading {
                    level: 1,
                    text: Inline::text("Heading"),
                },
                Block::Paragraph {
                    text: Inline::new(vec![
                        Span::Text("plain ".into()),
                        Span::Bold(vec![Span::Text("bold".into())]),
                        Span::Italic(vec![Span::Text("italic".into())]),
                        Span::Code("code".into()),
                        Span::Link {
                            href: "https://example.com".into(),
                            text: vec![Span::Text("link".into())],
                        },
                        Span::LineBreak,
                    ]),
                },
                Block::List {
                    ordered: true,
                    items: vec![vec![Block::paragraph("item")]],
                },
                Block::Table {
                    header: Some(Row::from_texts(["a", "b"])),
                    rows: vec![Row::from_texts(["1", "2"])],
                },
                Block::Code {
                    lang: Some("rust".into()),
                    text: "fn main() {}".into(),
                },
                Block::Image {
                    alt: Some("diagram".into()),
                },
                Block::PageBreak { page: 2 },
            ],
        };

        let json = serde_json::to_string(&doc).expect("Document must serialize");
        let back: Document = serde_json::from_str(&json).expect("Document must deserialize");
        assert_eq!(doc, back);
    }

    #[test]
    fn document_without_text_is_empty() {
        let doc = Document {
            meta: Metadata::default(),
            blocks: vec![Block::PageBreak { page: 1 }, Block::paragraph("  ")],
        };
        assert!(doc.is_empty());
    }
}
