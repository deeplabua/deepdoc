//! Serializers: `Document` → Markdown | JSON | text.
//!
//! Pure functions — no I/O, no globals — so they are unit-testable on small
//! in-memory documents.
//!
//! Phase 0 status: `to_json` is complete (it is the serde derive on the model);
//! `to_markdown` and `to_text` handle headings, paragraphs and code only.
//! Full block coverage — lists, tables, inline markup, escaping, page markers —
//! is Phase 1 (session 002), together with their unit tests.

use crate::model::{Block, Document, Inline, Metadata};

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
    let mut out = String::new();

    if opts.metadata {
        out.push_str(&front_matter(&doc.meta));
    }

    for block in &doc.blocks {
        match block {
            Block::Heading { level, text } => {
                let hashes = "#".repeat((*level).clamp(1, 6) as usize);
                out.push_str(&format!("{hashes} {}\n\n", inline_to_markdown(text)));
            }
            Block::Paragraph { text } => {
                out.push_str(&inline_to_markdown(text));
                out.push_str("\n\n");
            }
            Block::Code { lang, text } => {
                out.push_str(&format!(
                    "```{}\n{}\n```\n\n",
                    lang.as_deref().unwrap_or(""),
                    text.trim_end()
                ));
            }
            // TODO(Phase 1): List, Table, Image and PageBreak.
            other => {
                let text = other.plain_text();
                if !text.trim().is_empty() {
                    out.push_str(&text);
                    out.push_str("\n\n");
                }
            }
        }
    }

    out.trim_end().to_string() + "\n"
}

/// `Document` → JSON. The schema is the model itself; it is frozen in Phase 1.
pub fn to_json(doc: &Document) -> serde_json::Value {
    serde_json::to_value(doc).expect("Document is always serializable")
}

/// `Document` → plain text, no markup.
pub fn to_text(doc: &Document) -> String {
    let mut out = String::new();
    for block in &doc.blocks {
        let text = block.plain_text();
        if text.trim().is_empty() {
            continue;
        }
        out.push_str(text.trim_end());
        out.push_str("\n\n");
    }
    out.trim_end().to_string() + "\n"
}

/// TODO(Phase 1): bold/italic/code/link, plus Markdown escaping.
fn inline_to_markdown(inline: &Inline) -> String {
    inline.plain_text()
}

/// Metadata as a YAML front-matter block, empty when there is nothing to say.
// TODO(Phase 4): the full metadata set, once the extractors start filling it in.
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
    use crate::model::{Block, Metadata};

    fn doc(blocks: Vec<Block>) -> Document {
        Document {
            meta: Metadata::default(),
            blocks,
        }
    }

    #[test]
    fn renders_headings_and_paragraphs() {
        let rendered = to_markdown(
            &doc(vec![
                Block::Heading {
                    level: 2,
                    text: Inline::text("Summary"),
                },
                Block::paragraph("Revenue grew."),
            ]),
            &RenderOpts::default(),
        );
        assert_eq!(rendered, "## Summary\n\nRevenue grew.\n");
    }

    #[test]
    fn renders_fenced_code() {
        let rendered = to_markdown(
            &doc(vec![Block::Code {
                lang: Some("rust".into()),
                text: "fn main() {}".into(),
            }]),
            &RenderOpts::default(),
        );
        assert_eq!(rendered, "```rust\nfn main() {}\n```\n");
    }

    #[test]
    fn text_output_carries_no_markup() {
        let rendered = to_text(&doc(vec![Block::Heading {
            level: 1,
            text: Inline::text("Title"),
        }]));
        assert_eq!(rendered, "Title\n");
    }

    #[test]
    fn metadata_becomes_front_matter() {
        let doc = Document {
            meta: Metadata {
                title: Some(r#"A "quoted" title"#.into()),
                page_count: Some(8),
                source_format: Some(crate::detect::Format::Pdf),
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

    #[test]
    fn json_carries_metadata_and_blocks() {
        let value = to_json(&doc(vec![Block::paragraph("hi")]));
        assert!(value.get("meta").is_some());
        assert_eq!(value["blocks"][0]["type"], "paragraph");
    }
}
