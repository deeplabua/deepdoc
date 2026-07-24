//! Serializers: `Document` → Markdown | JSON | text.
//!
//! Pure functions — no I/O, no globals — so they are unit-testable on small
//! in-memory documents. Every block type is covered here; extractors only have
//! to produce a sane [`Document`].

use crate::model::{Block, Document, Inline, Metadata, Row, Span};

/// Output format for the renderers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Markdown,
    Json,
    Text,
}

/// Options that influence rendering.
#[derive(Debug, Clone, Default)]
pub struct RenderOpts {
    /// Emit document metadata as YAML front-matter (`--metadata`).
    pub metadata: bool,
}

/// Render a document in the requested format.
pub fn render(doc: &Document, format: OutputFormat, opts: &RenderOpts) -> String {
    match format {
        OutputFormat::Markdown => to_markdown(doc, opts),
        OutputFormat::Json => to_json(doc).to_string(),
        OutputFormat::Text => to_text(doc),
    }
}

/// `Document` → Markdown.
pub fn to_markdown(doc: &Document, opts: &RenderOpts) -> String {
    let body = blocks_to_markdown(&doc.blocks);
    let front = if opts.metadata {
        front_matter(&doc.meta)
    } else {
        String::new()
    };

    if body.trim().is_empty() {
        return front;
    }
    format!("{front}{}\n", body.trim_end())
}

/// `Document` → JSON.
///
/// The schema is the model's serde derive and is a public contract from here
/// on: `{"meta": {…}, "blocks": [{"type": …}]}`, with inline spans adjacently
/// tagged as `{"type": …, "value": …}`. See the snapshot test below.
pub fn to_json(doc: &Document) -> serde_json::Value {
    serde_json::to_value(doc).expect("Document is always serializable")
}

/// `Document` → plain text: structure without markup.
pub fn to_text(doc: &Document) -> String {
    let body = blocks_to_text(&doc.blocks);
    if body.trim().is_empty() {
        return String::new();
    }
    format!("{}\n", body.trim_end())
}

// ---------------------------------------------------------------- markdown --

/// Render a run of blocks, each separated by a blank line.
fn blocks_to_markdown(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        let rendered = block_to_markdown(block);
        if rendered.trim().is_empty() {
            continue;
        }
        out.push_str(rendered.trim_end());
        out.push_str("\n\n");
    }
    out
}

fn block_to_markdown(block: &Block) -> String {
    match block {
        Block::Heading { level, text } => {
            let hashes = "#".repeat((*level).clamp(1, 6) as usize);
            format!("{hashes} {}", one_line(&inline_to_markdown(text)))
        }
        Block::Paragraph { text } => escape_leading_marker(&inline_to_markdown(text)),
        Block::List { ordered, items } => list_to_markdown(*ordered, items),
        Block::Table { header, rows } => table_to_markdown(header.as_ref(), rows),
        Block::Code { lang, text } => code_to_markdown(lang.as_deref(), text),
        // The model carries no pixels — only the alt text is worth keeping.
        Block::Image { alt } => match alt {
            Some(alt) if !alt.trim().is_empty() => {
                format!("![{}]()", escape_markdown(alt.trim()))
            }
            _ => String::new(),
        },
        // Page markers survive as comments: invisible when rendered, greppable in the source.
        Block::PageBreak { page } => format!("<!-- page {page} -->"),
    }
}

fn list_to_markdown(ordered: bool, items: &[Vec<Block>]) -> String {
    let mut out = String::new();

    for (index, item) in items.iter().enumerate() {
        let marker = if ordered {
            format!("{}. ", index + 1)
        } else {
            "- ".to_string()
        };
        let body = item_to_markdown(item);
        let body = body.trim_end();

        if body.is_empty() {
            out.push_str(marker.trim_end());
            out.push('\n');
            continue;
        }

        let indent = " ".repeat(marker.chars().count());
        for (line_index, line) in body.lines().enumerate() {
            if line_index == 0 {
                out.push_str(&marker);
            } else if !line.is_empty() {
                out.push_str(&indent);
            }
            out.push_str(line);
            out.push('\n');
        }
    }

    out
}

/// Render the blocks of one list item.
///
/// Same as [`blocks_to_markdown`], except that a nested list follows its
/// item's text directly: the blank line a normal block separation would add
/// turns a tight list into a loose one.
fn item_to_markdown(blocks: &[Block]) -> String {
    let mut out = String::new();

    for (index, block) in blocks.iter().enumerate() {
        let rendered = block_to_markdown(block);
        if rendered.trim().is_empty() {
            continue;
        }
        out.push_str(rendered.trim_end());

        let tight = matches!(block, Block::Paragraph { .. })
            && matches!(blocks.get(index + 1), Some(Block::List { .. }));
        out.push_str(if tight { "\n" } else { "\n\n" });
    }

    out
}

fn table_to_markdown(header: Option<&Row>, rows: &[Row]) -> String {
    let width = header
        .iter()
        .copied()
        .chain(rows.iter())
        .map(|row| row.cells.len())
        .max()
        .unwrap_or(0);
    if width == 0 {
        return String::new();
    }

    let cells = |row: Option<&Row>| -> Vec<String> {
        let mut cells: Vec<String> = row
            .map(|row| row.cells.iter().map(cell_to_markdown).collect())
            .unwrap_or_default();
        cells.resize(width, String::new());
        cells
    };

    // A Markdown table always needs a header row; synthesise an empty one when
    // the source had none (a headerless spreadsheet range, say).
    let header_cells = cells(header);
    let body_cells: Vec<Vec<String>> = rows.iter().map(|row| cells(Some(row))).collect();

    // Pad the columns so the source table stays readable, as in the CLI spec.
    let mut widths = vec![3usize; width];
    for row in std::iter::once(&header_cells).chain(body_cells.iter()) {
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column].max(cell.chars().count());
        }
    }

    let render_row = |cells: &[String]| -> String {
        let padded: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(column, cell)| {
                let padding = widths[column].saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(padding))
            })
            .collect();
        format!("| {} |", padded.join(" | "))
    };

    let separator: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();

    let mut out = render_row(&header_cells);
    out.push('\n');
    out.push_str(&render_row(&separator));
    for row in &body_cells {
        out.push('\n');
        out.push_str(&render_row(row));
    }
    out
}

fn code_to_markdown(lang: Option<&str>, text: &str) -> String {
    // The fence must be longer than the longest backtick run inside the code.
    let longest_run = text
        .split(|c| c != '`')
        .map(|run| run.len())
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest_run.max(2) + 1);
    format!(
        "{fence}{}\n{}\n{fence}",
        lang.unwrap_or(""),
        text.trim_end()
    )
}

/// A table cell: inline markup, but never anything that breaks the row.
fn cell_to_markdown(cell: &Inline) -> String {
    one_line(&inline_to_markdown(cell)).replace('|', "\\|")
}

fn inline_to_markdown(inline: &Inline) -> String {
    spans_to_markdown(&inline.spans)
}

fn spans_to_markdown(spans: &[Span]) -> String {
    let mut out = String::new();
    for span in spans {
        match span {
            Span::Text(text) => out.push_str(&escape_markdown(text)),
            Span::Bold(spans) => {
                let inner = spans_to_markdown(spans);
                if !inner.trim().is_empty() {
                    out.push_str(&format!("**{}**", inner.trim()));
                }
            }
            Span::Italic(spans) => {
                let inner = spans_to_markdown(spans);
                if !inner.trim().is_empty() {
                    out.push_str(&format!("*{}*", inner.trim()));
                }
            }
            Span::Code(text) => {
                let longest_run = text
                    .split(|c| c != '`')
                    .map(|run| run.len())
                    .max()
                    .unwrap_or(0);
                let fence = "`".repeat(longest_run + 1);
                let padding = if text.starts_with('`') || text.ends_with('`') {
                    " "
                } else {
                    ""
                };
                out.push_str(&format!("{fence}{padding}{text}{padding}{fence}"));
            }
            Span::Link { href, text } => {
                let label = spans_to_markdown(text);
                let label = if label.trim().is_empty() {
                    escape_markdown(href)
                } else {
                    label
                };
                out.push_str(&format!("[{}]({})", label.trim(), escape_href(href)));
            }
            // Two trailing spaces are Markdown's hard line break.
            Span::LineBreak => out.push_str("  \n"),
        }
    }
    out
}

/// Escape the characters that would otherwise turn text into markup.
///
/// Deliberately narrow: over-escaping (`snake\_case`, `1\. one`) makes the
/// output worse to read than the odd stray asterisk. `_` is escaped only where
/// CommonMark would actually treat it as emphasis — at a word boundary.
fn escape_markdown(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());

    for (index, ch) in chars.iter().enumerate() {
        match ch {
            '\\' | '`' | '*' | '[' | ']' | '<' => {
                out.push('\\');
                out.push(*ch);
            }
            '_' => {
                let before_is_word = index
                    .checked_sub(1)
                    .and_then(|i| chars.get(i))
                    .is_some_and(|c| c.is_alphanumeric());
                let after_is_word = chars.get(index + 1).is_some_and(|c| c.is_alphanumeric());
                if before_is_word && after_is_word {
                    out.push('_');
                } else {
                    out.push_str("\\_");
                }
            }
            _ => out.push(*ch),
        }
    }

    out
}

/// Escape a paragraph that would otherwise read as a heading, quote or list item.
fn escape_leading_marker(text: &str) -> String {
    let trimmed = text.trim_start();
    let starts_ordered_item = || {
        let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
        !digits.is_empty()
            && matches!(
                trimmed.chars().nth(digits.len()),
                Some('.') | Some(')') if trimmed.chars().nth(digits.len() + 1) == Some(' ')
            )
    };

    let needs_escape = trimmed.starts_with('#')
        || trimmed.starts_with('>')
        || trimmed.starts_with('|')
        || trimmed.starts_with("- ")
        || trimmed.starts_with("+ ")
        || starts_ordered_item();

    if needs_escape {
        let offset = text.len() - trimmed.len();
        format!("{}\\{}", &text[..offset], trimmed)
    } else {
        text.to_string()
    }
}

/// A link target that survives Markdown's `[text](href)` syntax.
fn escape_href(href: &str) -> String {
    if href.contains([' ', '(', ')']) {
        format!("<{}>", href.replace('>', "%3E"))
    } else {
        href.to_string()
    }
}

/// Collapse a string onto a single line — for headings and table cells.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// -------------------------------------------------------------------- text --

fn blocks_to_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        let rendered = block_to_text(block);
        if rendered.trim().is_empty() {
            continue;
        }
        out.push_str(rendered.trim_end());
        out.push_str("\n\n");
    }
    out
}

fn block_to_text(block: &Block) -> String {
    match block {
        Block::Heading { text, .. } => one_line(&text.plain_text()),
        Block::Paragraph { text } => text.plain_text(),
        Block::List { ordered, items } => list_to_text(*ordered, items),
        Block::Table { header, rows } => header
            .iter()
            .chain(rows.iter())
            .map(Row::plain_text)
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Code { text, .. } => text.trim_end().to_string(),
        Block::Image { alt } => alt.clone().unwrap_or_default(),
        Block::PageBreak { .. } => String::new(),
    }
}

fn list_to_text(ordered: bool, items: &[Vec<Block>]) -> String {
    let mut out = String::new();

    for (index, item) in items.iter().enumerate() {
        let marker = if ordered {
            format!("{}. ", index + 1)
        } else {
            "- ".to_string()
        };
        let body = blocks_to_text(item);
        let body = body.trim_end();
        if body.is_empty() {
            continue;
        }

        let indent = " ".repeat(marker.chars().count());
        for (line_index, line) in body.lines().enumerate() {
            if line_index == 0 {
                out.push_str(&marker);
            } else if !line.is_empty() {
                out.push_str(&indent);
            }
            out.push_str(line);
            out.push('\n');
        }
    }

    out
}

// ---------------------------------------------------------- front-matter --

/// Metadata as a YAML front-matter block, empty when there is nothing to say.
fn front_matter(meta: &Metadata) -> String {
    let mut fields: Vec<(&str, String)> = Vec::new();
    if let Some(title) = &meta.title {
        fields.push(("title", yaml_string(title)));
    }
    if let Some(author) = &meta.author {
        fields.push(("author", yaml_string(author)));
    }
    if let Some(created) = &meta.created {
        fields.push(("created", yaml_string(created)));
    }
    if let Some(pages) = meta.page_count {
        fields.push(("pages", pages.to_string()));
    }
    if let Some(format) = meta.source_format {
        fields.push(("format", yaml_string(format.as_str())));
    }

    if fields.is_empty() {
        return String::new();
    }

    let body: String = fields
        .iter()
        .map(|(key, value)| format!("{key}: {value}\n"))
        .collect();
    format!("---\n{body}---\n\n")
}

/// Quote a string for YAML — double quotes, backslash and quote escaped.
fn yaml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Format;

    fn doc(blocks: Vec<Block>) -> Document {
        Document {
            meta: Metadata::default(),
            blocks,
        }
    }

    fn md(blocks: Vec<Block>) -> String {
        to_markdown(&doc(blocks), &RenderOpts::default())
    }

    #[test]
    fn renders_headings() {
        assert_eq!(
            md(vec![
                Block::Heading {
                    level: 1,
                    text: Inline::text("Title")
                },
                Block::Heading {
                    level: 6,
                    text: Inline::text("Deep")
                },
                // Levels outside 1..=6 are clamped, not dropped.
                Block::Heading {
                    level: 9,
                    text: Inline::text("Deeper")
                },
            ]),
            "# Title\n\n###### Deep\n\n###### Deeper\n"
        );
    }

    #[test]
    fn renders_paragraphs_and_escapes_markup() {
        assert_eq!(
            md(vec![Block::paragraph("a * b [c] _d_ `e` <f>")]),
            "a \\* b \\[c\\] \\_d\\_ \\`e\\` \\<f>\n"
        );
    }

    #[test]
    fn leaves_intra_word_underscores_alone() {
        assert_eq!(
            md(vec![Block::paragraph("snake_case_name")]),
            "snake_case_name\n"
        );
    }

    #[test]
    fn escapes_paragraphs_that_would_read_as_block_markup() {
        assert_eq!(
            md(vec![Block::paragraph("# not a heading")]),
            "\\# not a heading\n"
        );
        assert_eq!(
            md(vec![Block::paragraph("- not a list")]),
            "\\- not a list\n"
        );
        assert_eq!(
            md(vec![Block::paragraph("1. not a list")]),
            "\\1. not a list\n"
        );
        assert_eq!(
            md(vec![Block::paragraph("> not a quote")]),
            "\\> not a quote\n"
        );
        // A bare number is not a list item.
        assert_eq!(
            md(vec![Block::paragraph("2026 was fine")]),
            "2026 was fine\n"
        );
    }

    #[test]
    fn renders_inline_markup() {
        let inline = Inline::new(vec![
            Span::Bold(vec![Span::Text("bold".into())]),
            Span::Text(" ".into()),
            Span::Italic(vec![Span::Text("italic".into())]),
            Span::Text(" ".into()),
            Span::Code("let x = 1;".into()),
            Span::LineBreak,
            Span::Link {
                href: "https://example.com".into(),
                text: vec![Span::Text("site".into())],
            },
        ]);
        assert_eq!(
            md(vec![Block::Paragraph { text: inline }]),
            "**bold** *italic* `let x = 1;`  \n[site](https://example.com)\n"
        );
    }

    #[test]
    fn inline_code_picks_a_longer_fence_when_needed() {
        let inline = Inline::new(vec![Span::Code("a ` b".into())]);
        assert_eq!(md(vec![Block::Paragraph { text: inline }]), "``a ` b``\n");
    }

    #[test]
    fn link_targets_with_spaces_are_bracketed() {
        let inline = Inline::new(vec![Span::Link {
            href: "./my file.pdf".into(),
            text: vec![Span::Text("file".into())],
        }]);
        assert_eq!(
            md(vec![Block::Paragraph { text: inline }]),
            "[file](<./my file.pdf>)\n"
        );
    }

    #[test]
    fn renders_unordered_and_ordered_lists() {
        assert_eq!(
            md(vec![Block::List {
                ordered: false,
                items: vec![vec![Block::paragraph("one")], vec![Block::paragraph("two")]],
            }]),
            "- one\n- two\n"
        );
        assert_eq!(
            md(vec![Block::List {
                ordered: true,
                items: vec![vec![Block::paragraph("one")], vec![Block::paragraph("two")]],
            }]),
            "1. one\n2. two\n"
        );
    }

    #[test]
    fn renders_nested_lists_and_multi_block_items() {
        let rendered = md(vec![Block::List {
            ordered: false,
            items: vec![vec![
                Block::paragraph("outer"),
                Block::List {
                    ordered: true,
                    items: vec![vec![Block::paragraph("inner")]],
                },
            ]],
        }]);
        // A nested list stays tight: no blank line after its item's text.
        assert_eq!(rendered, "- outer\n  1. inner\n");
    }

    #[test]
    fn renders_tables_with_padded_columns() {
        let rendered = md(vec![Block::Table {
            header: Some(Row::from_texts(["Segment", "Q1"])),
            rows: vec![
                Row::from_texts(["Cloud", "4.1"]),
                Row::from_texts(["Devices", "1.2"]),
            ],
        }]);
        assert_eq!(
            rendered,
            "| Segment | Q1  |\n\
             | ------- | --- |\n\
             | Cloud   | 4.1 |\n\
             | Devices | 1.2 |\n"
        );
    }

    #[test]
    fn tables_escape_pipes_and_ragged_rows() {
        let rendered = md(vec![Block::Table {
            header: None,
            rows: vec![
                Row::from_texts(["a|b", "c"]),
                // A short row is padded, not dropped.
                Row::from_texts(["d"]),
            ],
        }]);
        assert_eq!(
            rendered,
            "|      |     |\n\
             | ---- | --- |\n\
             | a\\|b | c   |\n\
             | d    |     |\n"
        );
    }

    #[test]
    fn renders_fenced_code() {
        assert_eq!(
            md(vec![Block::Code {
                lang: Some("rust".into()),
                text: "fn main() {}\n".into(),
            }]),
            "```rust\nfn main() {}\n```\n"
        );
    }

    #[test]
    fn code_fence_grows_past_inner_backticks() {
        assert_eq!(
            md(vec![Block::Code {
                lang: None,
                text: "```\nnested\n```".into(),
            }]),
            "````\n```\nnested\n```\n````\n"
        );
    }

    #[test]
    fn renders_images_and_page_breaks() {
        assert_eq!(
            md(vec![
                Block::Image {
                    alt: Some("a [diagram]".into())
                },
                Block::PageBreak { page: 2 },
                Block::Image { alt: None },
            ]),
            "![a \\[diagram\\]]()\n\n<!-- page 2 -->\n"
        );
    }

    #[test]
    fn empty_document_renders_to_nothing() {
        assert_eq!(md(vec![]), "");
        assert_eq!(to_text(&doc(vec![])), "");
    }

    #[test]
    fn text_output_keeps_structure_without_markup() {
        let rendered = to_text(&doc(vec![
            Block::Heading {
                level: 2,
                text: Inline::text("Summary"),
            },
            Block::Paragraph {
                text: Inline::new(vec![
                    Span::Bold(vec![Span::Text("bold".into())]),
                    Span::Text(" and ".into()),
                    Span::Link {
                        href: "https://example.com".into(),
                        text: vec![Span::Text("a link".into())],
                    },
                ]),
            },
            Block::List {
                ordered: true,
                items: vec![vec![Block::paragraph("one")], vec![Block::paragraph("two")]],
            },
            Block::Table {
                header: Some(Row::from_texts(["a", "b"])),
                rows: vec![Row::from_texts(["1", "2"])],
            },
        ]));
        assert_eq!(
            rendered,
            "Summary\n\nbold and a link\n\n1. one\n2. two\n\na\tb\n1\t2\n"
        );
    }

    #[test]
    fn metadata_becomes_front_matter() {
        let doc = Document {
            meta: Metadata {
                title: Some(r#"A "quoted" title"#.into()),
                page_count: Some(8),
                source_format: Some(Format::Pdf),
                ..Metadata::default()
            },
            blocks: vec![Block::paragraph("body")],
        };
        let rendered = to_markdown(&doc, &RenderOpts { metadata: true });
        assert_eq!(
            rendered,
            "---\ntitle: \"A \\\"quoted\\\" title\"\npages: 8\nformat: \"pdf\"\n---\n\nbody\n"
        );
    }

    #[test]
    fn front_matter_is_skipped_when_there_is_no_metadata() {
        let doc = doc(vec![Block::paragraph("body")]);
        assert_eq!(to_markdown(&doc, &RenderOpts { metadata: true }), "body\n");
    }

    /// The JSON schema is a public contract — this snapshot is what breaks when
    /// a field is renamed.
    #[test]
    fn json_schema_snapshot() {
        let doc = Document {
            meta: Metadata {
                title: Some("Report".into()),
                source_format: Some(Format::Csv),
                ..Metadata::default()
            },
            blocks: vec![
                Block::Heading {
                    level: 1,
                    text: Inline::text("Title"),
                },
                Block::Paragraph {
                    text: Inline::new(vec![Span::Bold(vec![Span::Text("hi".into())])]),
                },
                Block::Table {
                    header: Some(Row::from_texts(["a"])),
                    rows: vec![Row::from_texts(["1"])],
                },
            ],
        };

        let expected = serde_json::json!({
            "meta": { "title": "Report", "source_format": "csv" },
            "blocks": [
                { "type": "heading", "level": 1, "text": [{ "type": "text", "value": "Title" }] },
                {
                    "type": "paragraph",
                    "text": [{
                        "type": "bold",
                        "value": [{ "type": "text", "value": "hi" }],
                    }],
                },
                {
                    "type": "table",
                    "header": { "cells": [[{ "type": "text", "value": "a" }]] },
                    "rows": [{ "cells": [[{ "type": "text", "value": "1" }]] }],
                },
            ],
        });
        assert_eq!(to_json(&doc), expected);
    }
}
