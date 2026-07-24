//! HTML extractor — a real HTML5 parse (html5ever), then a walk into the model.
//!
//! Going through the `Document` model rather than straight to Markdown is what
//! keeps `--format json` and `--chunk` working on web pages, and what lets the
//! same table/list rendering serve every format.

use std::path::Path;

use html5ever::tendril::TendrilSink;
use html5ever::{local_name, parse_document};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use crate::detect::{Format, Sniff};
use crate::error::{Error, Result};
use crate::extract::{ExtractOpts, Extractor};
use crate::model::{Block, Document, Inline, Metadata, Row, Span};

pub struct HtmlExtractor;

impl Extractor for HtmlExtractor {
    fn name(&self) -> &'static str {
        "html"
    }

    fn supports(&self, _path: &Path, sniff: &Sniff) -> bool {
        crate::detect::detect(sniff) == Some(Format::Html)
    }

    fn extract(&self, path: &Path, _opts: &ExtractOpts) -> Result<Document> {
        let raw = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        let parsed = parse(&String::from_utf8_lossy(&raw));

        Ok(Document {
            meta: Metadata {
                title: parsed.title,
                author: parsed.author,
                source_format: Some(Format::Html),
                source_path: Some(path.display().to_string()),
                ..Metadata::default()
            },
            blocks: parsed.blocks,
        })
    }
}

/// What a parsed page yields.
#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub title: Option<String>,
    pub author: Option<String>,
    pub blocks: Vec<Block>,
}

/// Parse an HTML document. Pure.
pub fn parse(html: &str) -> Parsed {
    let dom = parse_document(RcDom::default(), Default::default()).one(html);

    let mut parsed = Parsed::default();
    let mut builder = Builder::default();
    walk(&dom.document, &mut builder, &mut parsed);
    parsed.blocks = builder.finish();
    parsed
}

/// Collects blocks, buffering loose inline content into implicit paragraphs.
#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    inline: Vec<Span>,
}

impl Builder {
    /// Close the paragraph being accumulated, if it holds anything.
    fn flush(&mut self) {
        let spans = trim_spans(std::mem::take(&mut self.inline));
        if !spans.is_empty() {
            self.blocks.push(Block::Paragraph {
                text: Inline::new(spans),
            });
        }
    }

    fn push_block(&mut self, block: Block) {
        self.flush();
        self.blocks.push(block);
    }

    fn push_spans(&mut self, spans: Vec<Span>) {
        self.inline.extend(spans);
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush();
        self.blocks
    }
}

/// Elements whose content is never document text.
fn is_dropped(tag: &str) -> bool {
    matches!(
        tag,
        "script"
            | "style"
            | "noscript"
            | "template"
            | "svg"
            | "canvas"
            | "iframe"
            | "object"
            | "embed"
            | "audio"
            | "video"
            | "form"
            | "button"
            | "select"
            | "textarea"
            | "input"
            // Page furniture, not content — the usual reason HTML → Markdown
            // output is full of menus.
            | "nav"
            | "footer"
            | "aside"
    )
}

fn is_inline(tag: &str) -> bool {
    matches!(
        tag,
        "a" | "abbr"
            | "b"
            | "cite"
            | "code"
            | "del"
            | "em"
            | "i"
            | "ins"
            | "kbd"
            | "mark"
            | "q"
            | "s"
            | "samp"
            | "small"
            | "span"
            | "strong"
            | "sub"
            | "sup"
            | "time"
            | "tt"
            | "u"
            | "var"
    )
}

fn walk(node: &Handle, builder: &mut Builder, parsed: &mut Parsed) {
    for child in node.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => {
                let text = normalize_whitespace(&contents.borrow());
                if !text.is_empty() {
                    builder.push_spans(vec![Span::Text(text)]);
                }
            }
            NodeData::Element { name, attrs, .. } => {
                let tag = name.local.as_ref();
                if is_dropped(tag) {
                    continue;
                }

                match tag {
                    "title" => {
                        let title = text_content(child).trim().to_string();
                        if parsed.title.is_none() && !title.is_empty() {
                            parsed.title = Some(title);
                        }
                    }
                    "meta" => {
                        let attrs = attrs.borrow();
                        let name = attrs
                            .iter()
                            .find(|a| a.name.local == local_name!("name"))
                            .map(|a| a.value.to_ascii_lowercase());
                        if name.as_deref() == Some("author") {
                            let author = attrs
                                .iter()
                                .find(|a| a.name.local == local_name!("content"))
                                .map(|a| a.value.trim().to_string());
                            parsed.author = author.filter(|a| !a.is_empty());
                        }
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        let level = tag[1..].parse().unwrap_or(1);
                        let spans = trim_spans(collect_inline(child));
                        if !spans.is_empty() {
                            builder.push_block(Block::Heading {
                                level,
                                text: Inline::new(spans),
                            });
                        }
                    }
                    "p" => {
                        builder.flush();
                        let spans = trim_spans(collect_inline(child));
                        if !spans.is_empty() {
                            builder.blocks.push(Block::Paragraph {
                                text: Inline::new(spans),
                            });
                        }
                    }
                    "br" => builder.push_spans(vec![Span::LineBreak]),
                    "hr" => builder.flush(),
                    "pre" => {
                        let text = text_content(child);
                        if !text.trim().is_empty() {
                            builder.push_block(Block::Code {
                                lang: code_language(child),
                                text: text.trim_matches('\n').to_string(),
                            });
                        }
                    }
                    "ul" | "ol" => {
                        let items = list_items(child, parsed);
                        if !items.is_empty() {
                            builder.push_block(Block::List {
                                ordered: tag == "ol",
                                items,
                            });
                        }
                    }
                    "table" => {
                        if let Some(table) = table_block(child) {
                            builder.push_block(table);
                        }
                    }
                    "img" => {
                        if let Some(alt) = attr(child, "alt").filter(|alt| !alt.trim().is_empty()) {
                            builder.push_block(Block::Image {
                                alt: Some(alt.trim().to_string()),
                            });
                        }
                    }
                    tag if is_inline(tag) => {
                        let spans = collect_inline(child);
                        builder.push_spans(spans);
                    }
                    // Everything else (div, section, article, blockquote, …) is
                    // a transparent container: recurse and keep the content.
                    _ => {
                        builder.flush();
                        walk(child, builder, parsed);
                        builder.flush();
                    }
                }
            }
            _ => {}
        }
    }
}

/// Build one item per `<li>`, recursing so nested lists survive.
fn list_items(list: &Handle, parsed: &mut Parsed) -> Vec<Vec<Block>> {
    let mut items = Vec::new();
    for child in list.children.borrow().iter() {
        let NodeData::Element { name, .. } = &child.data else {
            continue;
        };
        if name.local.as_ref() != "li" {
            continue;
        }

        let mut builder = Builder::default();
        walk(child, &mut builder, parsed);
        let blocks = builder.finish();
        if !blocks.is_empty() {
            items.push(blocks);
        }
    }
    items
}

fn table_block(table: &Handle) -> Option<Block> {
    let mut header: Option<Row> = None;
    let mut rows = Vec::new();

    for row in table_rows(table) {
        let mut cells = Vec::new();
        let mut all_header_cells = true;

        for cell in row.children.borrow().iter() {
            let NodeData::Element { name, .. } = &cell.data else {
                continue;
            };
            match name.local.as_ref() {
                "th" => cells.push(Inline::new(trim_spans(collect_inline(cell)))),
                "td" => {
                    all_header_cells = false;
                    cells.push(Inline::new(trim_spans(collect_inline(cell))));
                }
                _ => {}
            }
        }

        if cells.is_empty() {
            continue;
        }
        if header.is_none() && all_header_cells {
            header = Some(Row::new(cells));
        } else {
            rows.push(Row::new(cells));
        }
    }

    (header.is_some() || !rows.is_empty()).then_some(Block::Table { header, rows })
}

/// Every `<tr>` under a table, whatever section it sits in.
fn table_rows(node: &Handle) -> Vec<Handle> {
    let mut rows = Vec::new();
    for child in node.children.borrow().iter() {
        if let NodeData::Element { name, .. } = &child.data {
            match name.local.as_ref() {
                "tr" => rows.push(child.clone()),
                "thead" | "tbody" | "tfoot" => rows.extend(table_rows(child)),
                _ => {}
            }
        }
    }
    rows
}

/// Collect the inline content of an element, flattening what the model cannot hold.
fn collect_inline(node: &Handle) -> Vec<Span> {
    let mut spans = Vec::new();

    for child in node.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => {
                let text = normalize_whitespace(&contents.borrow());
                if !text.is_empty() {
                    spans.push(Span::Text(text));
                }
            }
            NodeData::Element { name, .. } => {
                let tag = name.local.as_ref();
                if is_dropped(tag) {
                    continue;
                }
                match tag {
                    "strong" | "b" => push_styled(&mut spans, child, Span::Bold),
                    "em" | "i" => push_styled(&mut spans, child, Span::Italic),
                    "code" | "kbd" | "samp" | "tt" | "var" => {
                        let text = text_content(child);
                        if !text.trim().is_empty() {
                            spans.push(Span::Code(text.trim().to_string()));
                        }
                    }
                    "a" => {
                        let inner = collect_inline(child);
                        match attr(child, "href") {
                            Some(href) if !href.trim().is_empty() && !inner.is_empty() => {
                                spans.push(Span::Link {
                                    href: href.trim().to_string(),
                                    text: inner,
                                });
                            }
                            _ => spans.extend(inner),
                        }
                    }
                    "br" => spans.push(Span::LineBreak),
                    "img" => {
                        if let Some(alt) = attr(child, "alt").filter(|alt| !alt.trim().is_empty()) {
                            spans.push(Span::Text(alt.trim().to_string()));
                        }
                    }
                    _ => spans.extend(collect_inline(child)),
                }
            }
            _ => {}
        }
    }

    spans
}

fn push_styled(spans: &mut Vec<Span>, node: &Handle, wrap: fn(Vec<Span>) -> Span) {
    let inner = trim_spans(collect_inline(node));
    if !inner.is_empty() {
        spans.push(wrap(inner));
    }
}

/// `<pre><code class="language-rust">` → `Some("rust")`.
fn code_language(pre: &Handle) -> Option<String> {
    let class = attr(pre, "class").or_else(|| {
        pre.children
            .borrow()
            .iter()
            .find(|child| matches!(&child.data, NodeData::Element { name, .. } if name.local.as_ref() == "code"))
            .and_then(|code| attr(code, "class"))
    })?;

    class.split_whitespace().find_map(|token| {
        token
            .strip_prefix("language-")
            .or_else(|| token.strip_prefix("lang-"))
            .map(str::to_string)
    })
}

fn attr(node: &Handle, wanted: &str) -> Option<String> {
    let NodeData::Element { attrs, .. } = &node.data else {
        return None;
    };
    attrs
        .borrow()
        .iter()
        .find(|attr| attr.name.local.as_ref() == wanted)
        .map(|attr| attr.value.to_string())
}

/// All text under a node, with no whitespace normalisation (`<pre>`, `<title>`).
fn text_content(node: &Handle) -> String {
    let mut out = String::new();
    for child in node.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => out.push_str(&contents.borrow()),
            NodeData::Element { name, .. } if !is_dropped(name.local.as_ref()) => {
                out.push_str(&text_content(child));
            }
            _ => {}
        }
    }
    out
}

/// HTML collapses every run of whitespace into a single space.
///
/// Leading and trailing spaces are kept: a text node that is nothing but
/// whitespace is what separates `</strong>` from the next word. The block's
/// outer edges are trimmed later, by [`trim_spans`].
fn normalize_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            in_space = true;
        } else {
            if in_space {
                out.push(' ');
            }
            in_space = false;
            out.push(ch);
        }
    }
    if in_space {
        out.push(' ');
    }
    out
}

/// Drop the leading and trailing whitespace of a span run.
fn trim_spans(spans: Vec<Span>) -> Vec<Span> {
    let mut spans = spans;

    while let Some(first) = spans.first_mut() {
        if let Span::Text(text) = first {
            let trimmed = text.trim_start();
            if trimmed.is_empty() {
                spans.remove(0);
                continue;
            }
            *text = trimmed.to_string();
        }
        break;
    }

    while let Some(last) = spans.last_mut() {
        match last {
            Span::Text(text) => {
                let trimmed = text.trim_end();
                if trimmed.is_empty() {
                    spans.pop();
                    continue;
                }
                *text = trimmed.to_string();
            }
            Span::LineBreak => {
                spans.pop();
                continue;
            }
            _ => {}
        }
        break;
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{RenderOpts, to_markdown};

    fn blocks(html: &str) -> Vec<Block> {
        parse(html).blocks
    }

    fn markdown(html: &str) -> String {
        let doc = Document {
            meta: Metadata::default(),
            blocks: blocks(html),
        };
        to_markdown(&doc, &RenderOpts::default())
    }

    #[test]
    fn reads_title_and_author() {
        let parsed = parse(
            "<html><head><title>A page</title><meta name=\"author\" content=\"Ada\"></head>\
             <body><p>hi</p></body></html>",
        );
        assert_eq!(parsed.title.as_deref(), Some("A page"));
        assert_eq!(parsed.author.as_deref(), Some("Ada"));
    }

    #[test]
    fn headings_and_paragraphs() {
        assert_eq!(
            blocks("<h2>Summary</h2><p>Revenue grew.</p>"),
            vec![
                Block::Heading {
                    level: 2,
                    text: Inline::text("Summary")
                },
                Block::paragraph("Revenue grew."),
            ]
        );
    }

    #[test]
    fn collapses_whitespace_like_a_browser() {
        assert_eq!(
            blocks("<p>one   two\n\n   three</p>"),
            vec![Block::paragraph("one two three")]
        );
    }

    #[test]
    fn inline_markup_becomes_spans() {
        let Block::Paragraph { text } = &blocks(
            "<p>a <strong>bold</strong> <em>it</em> <code>x</code> <a href=\"/l\">link</a></p>",
        )[0] else {
            panic!("expected a paragraph");
        };
        assert_eq!(
            text.spans,
            vec![
                Span::Text("a ".into()),
                Span::Bold(vec![Span::Text("bold".into())]),
                Span::Text(" ".into()),
                Span::Italic(vec![Span::Text("it".into())]),
                Span::Text(" ".into()),
                Span::Code("x".into()),
                Span::Text(" ".into()),
                Span::Link {
                    href: "/l".into(),
                    text: vec![Span::Text("link".into())],
                },
            ]
        );
    }

    #[test]
    fn script_style_and_page_furniture_are_dropped() {
        assert_eq!(
            blocks(
                "<nav><a href=\"/\">menu</a></nav>\
                 <script>alert(1)</script><style>p{}</style>\
                 <p>content</p><footer>copyright</footer>"
            ),
            vec![Block::paragraph("content")]
        );
    }

    #[test]
    fn nested_lists_survive() {
        assert_eq!(
            blocks("<ul><li>one<ol><li>inner</li></ol></li><li>two</li></ul>"),
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
    fn tables_use_th_rows_as_the_header() {
        assert_eq!(
            blocks(
                "<table><thead><tr><th>a</th><th>b</th></tr></thead>\
                 <tbody><tr><td>1</td><td>2</td></tr></tbody></table>"
            ),
            vec![Block::Table {
                header: Some(Row::from_texts(["a", "b"])),
                rows: vec![Row::from_texts(["1", "2"])],
            }]
        );
    }

    #[test]
    fn headerless_tables_keep_every_row() {
        assert_eq!(
            blocks("<table><tr><td>1</td></tr><tr><td>2</td></tr></table>"),
            vec![Block::Table {
                header: None,
                rows: vec![Row::from_texts(["1"]), Row::from_texts(["2"])],
            }]
        );
    }

    #[test]
    fn pre_becomes_a_code_block_with_its_language() {
        assert_eq!(
            blocks("<pre><code class=\"language-rust\">fn main() {}\n</code></pre>"),
            vec![Block::Code {
                lang: Some("rust".into()),
                text: "fn main() {}".into(),
            }]
        );
    }

    #[test]
    fn images_keep_their_alt_text() {
        assert_eq!(
            blocks("<img src=\"a.png\" alt=\"a diagram\">"),
            vec![Block::Image {
                alt: Some("a diagram".into())
            }]
        );
        // Decorative images carry no text and are dropped.
        assert!(blocks("<img src=\"a.png\" alt=\"\">").is_empty());
    }

    #[test]
    fn loose_text_in_a_div_still_becomes_a_paragraph() {
        assert_eq!(
            blocks("<div>loose text<br>second line</div>"),
            vec![Block::Paragraph {
                text: Inline::new(vec![
                    Span::Text("loose text".into()),
                    Span::LineBreak,
                    Span::Text("second line".into()),
                ])
            }]
        );
    }

    #[test]
    fn renders_a_small_page_to_markdown() {
        let rendered = markdown(
            "<h1>Report</h1><p>Revenue grew <strong>12%</strong>.</p>\
             <table><tr><th>Segment</th><th>Q1</th></tr><tr><td>Cloud</td><td>4.1</td></tr></table>",
        );
        assert_eq!(
            rendered,
            "# Report\n\n\
             Revenue grew **12%**.\n\n\
             | Segment | Q1  |\n\
             | ------- | --- |\n\
             | Cloud   | 4.1 |\n"
        );
    }
}
