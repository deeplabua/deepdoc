//! RAG chunking: `Document` → chunks that carry their heading context.
//!
//! Two ideas carry the whole module:
//!
//! * **Cut on block boundaries.** A paragraph, a table or a code block is never
//!   split down the middle — only a single block bigger than the target size is,
//!   and then at the friendliest boundary it offers (line, sentence, word).
//! * **Every chunk knows where it sits.** The chain of enclosing headings
//!   (`Handbook > Onboarding > Day one`) travels with the text, so an embedding
//!   of "it takes two days" still carries what "it" was.
//!
//! Sizes are counted in **characters**, not tokens: a tokenizer means a model
//! file or a large table inside a binary whose whole promise is that there is
//! nothing to install. Roughly four characters per token is close enough to aim
//! at an embedding model's window.
//!
//! `byte_range` indexes the Markdown rendering of the document — exactly what
//! `deepdoc file --format md` prints, without front matter — and slicing it with
//! the range gives the chunk's text back.

use serde::{Deserialize, Serialize};

use crate::model::{Block, Document};
use crate::render;

/// Default chunk size, in characters (see `docs/PRD/02-cli-spec.md`).
pub const DEFAULT_CHUNK_SIZE: usize = 800;

/// Options for [`chunk`].
#[derive(Debug, Clone)]
pub struct ChunkOpts {
    /// Target chunk size, in characters.
    pub size: usize,
    /// Overlap between neighbouring chunks, in characters.
    pub overlap: usize,
}

impl Default for ChunkOpts {
    fn default() -> Self {
        Self {
            size: DEFAULT_CHUNK_SIZE,
            overlap: 0,
        }
    }
}

/// One RAG chunk.
///
/// The field names are the public JSON schema (`docs/PRD/02-cli-spec.md`).
/// `source` repeats the document path on purpose: chunks are usually flattened
/// into a vector store one by one, and a chunk that cannot say where it came
/// from is a citation you cannot follow back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub text: String,
    /// Enclosing headings, outermost first (`["Introduction", "Method"]`).
    pub heading_path: Vec<String>,
    /// Path the document was read from, when it is known.
    pub source: Option<String>,
    /// Byte range of the chunk inside the rendered Markdown.
    pub byte_range: (usize, usize),
}

/// Split a document into chunks. Pure.
pub fn chunk(doc: &Document, opts: &ChunkOpts) -> Vec<Chunk> {
    let size = opts.size.max(1);
    // The overlap must leave room for new text, or two neighbours would say
    // almost the same thing and the second one would carry the work forward
    // hardly at all.
    let overlap = opts.overlap.min(size / 2);

    let (markdown, ranges) = render::markdown_with_ranges(&doc.blocks);
    let segments = segments(&markdown, &doc.blocks, &ranges, size);
    let groups = pack(&markdown, &segments, size);

    let source = doc.meta.source_path.clone();
    let mut chunks = Vec::with_capacity(groups.len());

    for (position, &(first, end)) in groups.iter().enumerate() {
        let start = match position {
            0 => first,
            _ => overlap_start(&markdown, &segments, groups[position - 1].0, first, overlap),
        };
        let (from, to) = (segments[start].start, segments[end - 1].end);

        chunks.push(Chunk {
            text: markdown[from..to].to_string(),
            // The path of the chunk's own first block: text pulled in as
            // overlap belongs to the chunk before it.
            heading_path: segments[first].path.clone(),
            source: source.clone(),
            byte_range: (from, to),
        });
    }

    chunks
}

/// A unit that a chunk is built from: one block, or one piece of an oversized
/// block.
struct Segment {
    start: usize,
    end: usize,
    /// Length in characters — what `size` is measured in.
    chars: usize,
    /// Enclosing headings at this point in the document.
    path: Vec<String>,
    /// True when the segment is a heading, and so opens a section.
    heading: bool,
}

/// Walk the blocks, tracking the heading stack, and cut oversized blocks up.
fn segments(
    markdown: &str,
    blocks: &[Block],
    ranges: &[Option<(usize, usize)>],
    size: usize,
) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut stack: Vec<(u8, String)> = Vec::new();

    for (block, range) in blocks.iter().zip(ranges) {
        let Some((start, end)) = *range else { continue };

        if let Block::Heading { level, text } = block {
            // Levels are clamped the same way the Markdown renderer clamps
            // them, so `#######` and `######` close the same sections.
            let level = (*level).clamp(1, 6);
            while stack.last().is_some_and(|(open, _)| *open >= level) {
                stack.pop();
            }
            stack.push((level, render::one_line(&text.plain_text())));
        }

        let path: Vec<String> = stack.iter().map(|(_, text)| text.clone()).collect();
        let heading = matches!(block, Block::Heading { .. });

        for (index, (start, end)) in split_block(&markdown[start..end], start, size)
            .into_iter()
            .enumerate()
        {
            out.push(Segment {
                start,
                end,
                chars: markdown[start..end].chars().count(),
                path: path.clone(),
                heading: heading && index == 0,
            });
        }
    }

    out
}

/// Cut one oversized block into pieces of at most `size` characters.
///
/// Returns absolute ranges into the rendered Markdown. A block that already
/// fits comes back whole — the common case, and the one the block-boundary
/// promise is about.
fn split_block(text: &str, base: usize, size: usize) -> Vec<(usize, usize)> {
    if text.chars().count() <= size {
        return vec![(base, base + text.len())];
    }

    let mut out = Vec::new();
    let mut cursor = 0;

    while cursor < text.len() {
        let rest = &text[cursor..];
        if rest.chars().count() <= size {
            out.push((base + cursor, base + text.len()));
            break;
        }

        let window = &rest[..byte_len(rest, size)];
        let cut = cut_point(window);
        let piece = rest[..cut].trim_end();
        if !piece.is_empty() {
            out.push((base + cursor, base + cursor + piece.len()));
        }

        // Step over the whitespace at the seam so the next piece starts on text.
        let skipped = rest[cut..].len() - rest[cut..].trim_start().len();
        cursor += cut + skipped;
    }

    out
}

/// Where to cut a window of text: the last line break, else the last sentence
/// end, else the last word boundary, else the window itself.
fn cut_point(window: &str) -> usize {
    if let Some(index) = window.rfind('\n').filter(|index| *index > 0) {
        return index + 1;
    }

    let sentence_end = window.char_indices().rfind(|(index, ch)| {
        *index > 0
            && matches!(ch, '.' | '!' | '?' | '…')
            && window[index + ch.len_utf8()..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
    });
    if let Some((index, ch)) = sentence_end {
        return index + ch.len_utf8();
    }

    window
        .char_indices()
        .rfind(|(index, ch)| *index > 0 && ch.is_whitespace())
        .map_or(window.len(), |(index, _)| index)
}

/// Byte length of the first `chars` characters.
fn byte_len(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(index, _)| index)
}

/// Greedily group segments into chunks; returns half-open index ranges.
///
/// A chunk grows until the next segment would overflow it. Headings also break
/// chunks, but not every heading: a document of many two-line sections would
/// otherwise chunk into a crowd of fragments too small to embed. So a heading
/// starts a new chunk when
///
/// * the current chunk is already worth keeping on its own (half the target), or
/// * the heading climbs back out of the section running just before it —
///   sibling and nested sections can share a chunk, `## Payroll` after
///   `### Tools` cannot, or the chunk's heading path would lie about half its
///   own text.
fn pack(markdown: &str, segments: &[Segment], size: usize) -> Vec<(usize, usize)> {
    let mut groups = Vec::new();
    let mut start = 0;
    let mut chars = 0;

    for (index, segment) in segments.iter().enumerate() {
        if index > start {
            let gap = gap_chars(markdown, &segments[index - 1], segment);
            let overflows = chars + gap + segment.chars > size;
            let new_section = segment.heading
                && (chars * 2 >= size || segment.path.len() < segments[index - 1].path.len());

            if overflows || new_section {
                groups.push((start, index));
                start = index;
                chars = 0;
            } else {
                chars += gap;
            }
        }
        chars += segment.chars;
    }

    if start < segments.len() {
        groups.push((start, segments.len()));
    }
    groups
}

/// Where a chunk starts once `overlap` characters of the previous one are
/// pulled in: whole segments only, and never the previous chunk entire.
fn overlap_start(
    markdown: &str,
    segments: &[Segment],
    previous: usize,
    first: usize,
    overlap: usize,
) -> usize {
    let mut start = first;
    let mut taken = 0;

    while start > previous + 1 {
        let segment = &segments[start - 1];
        let gap = gap_chars(markdown, segment, &segments[start]);
        if taken + gap + segment.chars > overlap {
            break;
        }
        taken += gap + segment.chars;
        start -= 1;
    }

    start
}

/// Characters of separator (usually the blank line) between two segments.
fn gap_chars(markdown: &str, left: &Segment, right: &Segment) -> usize {
    markdown[left.end..right.start].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Block, Inline, Metadata, Row};
    use crate::render::{RenderOpts, to_markdown};

    fn doc(blocks: Vec<Block>) -> Document {
        Document {
            meta: Metadata::default(),
            blocks,
        }
    }

    fn heading(level: u8, text: &str) -> Block {
        Block::Heading {
            level,
            text: Inline::text(text),
        }
    }

    fn opts(size: usize, overlap: usize) -> ChunkOpts {
        ChunkOpts { size, overlap }
    }

    /// Every chunk must be a verbatim slice of the Markdown the CLI prints —
    /// that is what `byte_range` promises.
    fn check_ranges(doc: &Document, chunks: &[Chunk]) {
        let markdown = to_markdown(doc, &RenderOpts::default());
        for chunk in chunks {
            let (start, end) = chunk.byte_range;
            assert_eq!(
                &markdown[start..end],
                chunk.text,
                "byte_range does not point at the chunk's text"
            );
        }
    }

    #[test]
    fn empty_document_yields_no_chunks() {
        let doc = Document::new(Metadata::default());
        assert!(chunk(&doc, &ChunkOpts::default()).is_empty());
    }

    #[test]
    fn a_short_document_is_one_chunk() {
        let doc = doc(vec![Block::paragraph("hello")]);
        let chunks = chunk(&doc, &ChunkOpts::default());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "hello");
        assert_eq!(chunks[0].byte_range, (0, 5));
        check_ranges(&doc, &chunks);
    }

    #[test]
    fn chunks_cut_on_block_boundaries() {
        let doc = doc(vec![
            Block::paragraph("a".repeat(40)),
            Block::paragraph("b".repeat(40)),
            Block::paragraph("c".repeat(40)),
        ]);
        let chunks = chunk(&doc, &opts(90, 0));

        // Two paragraphs fit (40 + 2 + 40); the third opens the next chunk
        // rather than being cut in half.
        assert_eq!(chunks.len(), 2);
        assert_eq!(
            chunks[0].text,
            format!("{}\n\n{}", "a".repeat(40), "b".repeat(40))
        );
        assert_eq!(chunks[1].text, "c".repeat(40));
        check_ranges(&doc, &chunks);
    }

    #[test]
    fn a_table_is_never_split_when_it_fits() {
        let doc = doc(vec![
            Block::paragraph("intro"),
            Block::Table {
                header: Some(Row::from_texts(["part", "qty"])),
                rows: vec![Row::from_texts(["bolt", "4"])],
            },
        ]);
        // Room for the table (44 characters) but not for the paragraph too.
        let chunks = chunk(&doc, &opts(46, 0));
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text, "intro");
        assert!(chunks[1].text.starts_with("| part | qty |"));
        assert!(chunks[1].text.ends_with("| bolt | 4   |"));
        check_ranges(&doc, &chunks);
    }

    #[test]
    fn a_table_too_big_for_a_chunk_is_cut_between_rows() {
        let rows: Vec<Row> = (0..20)
            .map(|n| Row::from_texts([format!("row {n}"), n.to_string()]))
            .collect();
        let doc = doc(vec![Block::Table {
            header: Some(Row::from_texts(["part", "qty"])),
            rows,
        }]);
        let chunks = chunk(&doc, &opts(80, 0));

        assert!(chunks.len() > 1);
        for chunk in &chunks {
            for line in chunk.text.lines() {
                assert!(line.starts_with('|') && line.ends_with('|'), "{line:?}");
            }
        }
        check_ranges(&doc, &chunks);
    }

    #[test]
    fn heading_path_follows_the_outline() {
        let doc = doc(vec![
            heading(1, "Handbook"),
            Block::paragraph("intro"),
            heading(2, "Onboarding"),
            Block::paragraph("day one"),
            heading(3, "Tools"),
            Block::paragraph("laptop"),
            heading(2, "Payroll"),
            Block::paragraph("monthly"),
        ]);
        // One block per chunk, so every path is easy to read off.
        let chunks = chunk(&doc, &opts(12, 0));
        let paths: Vec<Vec<String>> = chunks.iter().map(|c| c.heading_path.clone()).collect();

        assert_eq!(paths[0], ["Handbook"]);
        assert_eq!(paths[1], ["Handbook"]);
        assert_eq!(paths[2], ["Handbook", "Onboarding"]);
        assert_eq!(paths[3], ["Handbook", "Onboarding"]);
        assert_eq!(paths[4], ["Handbook", "Onboarding", "Tools"]);
        assert_eq!(paths[5], ["Handbook", "Onboarding", "Tools"]);
        // A second `##` closes both `## Onboarding` and its `### Tools`.
        assert_eq!(paths[6], ["Handbook", "Payroll"]);
        check_ranges(&doc, &chunks);
    }

    #[test]
    fn a_heading_starts_a_new_chunk_once_the_current_one_is_worth_keeping() {
        let doc = doc(vec![
            heading(2, "One"),
            Block::paragraph("a".repeat(60)),
            heading(2, "Two"),
            Block::paragraph("b".repeat(20)),
        ]);
        let chunks = chunk(&doc, &opts(100, 0));

        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].text.starts_with("## One"));
        assert!(chunks[1].text.starts_with("## Two"));
        assert_eq!(chunks[1].heading_path, ["Two"]);
        check_ranges(&doc, &chunks);
    }

    #[test]
    fn short_sections_are_merged_rather_than_left_as_fragments() {
        let doc = doc(vec![
            heading(2, "One"),
            Block::paragraph("first"),
            heading(2, "Two"),
            Block::paragraph("second"),
        ]);
        let chunks = chunk(&doc, &opts(200, 0));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].heading_path, ["One"]);
    }

    #[test]
    fn a_heading_that_leaves_the_section_breaks_the_chunk() {
        let doc = doc(vec![
            heading(1, "Handbook"),
            heading(2, "Onboarding"),
            heading(3, "Tools"),
            Block::paragraph("laptop"),
            heading(2, "Payroll"),
            Block::paragraph("monthly"),
        ]);
        // Everything would fit in one chunk, but `## Payroll` climbs back out
        // of `### Tools`: keeping them together would leave half the chunk
        // filed under a heading path it has nothing to do with.
        let chunks = chunk(&doc, &opts(500, 0));
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].heading_path, ["Handbook"]);
        assert!(chunks[0].text.ends_with("laptop"));
        assert_eq!(chunks[1].heading_path, ["Handbook", "Payroll"]);
        check_ranges(&doc, &chunks);
    }

    #[test]
    fn overlap_repeats_whole_blocks() {
        let doc = doc(vec![
            Block::paragraph("a".repeat(30)),
            Block::paragraph("b".repeat(30)),
            Block::paragraph("c".repeat(30)),
        ]);
        let chunks = chunk(&doc, &opts(64, 32));

        assert_eq!(chunks.len(), 2);
        assert_eq!(
            chunks[0].text,
            format!("{}\n\n{}", "a".repeat(30), "b".repeat(30))
        );
        // The second chunk opens with the block that closed the first one.
        assert_eq!(
            chunks[1].text,
            format!("{}\n\n{}", "b".repeat(30), "c".repeat(30))
        );
        check_ranges(&doc, &chunks);
    }

    #[test]
    fn overlap_never_swallows_the_previous_chunk_whole() {
        let doc = doc(vec![
            Block::paragraph("a".repeat(20)),
            Block::paragraph("b".repeat(20)),
            Block::paragraph("c".repeat(20)),
            Block::paragraph("d".repeat(20)),
        ]);
        // An overlap as large as the chunk itself is clamped, and at least one
        // block always stays behind.
        let chunks = chunk(&doc, &opts(44, 1000));
        assert_eq!(chunks.len(), 2);
        assert!(chunks[1].text.starts_with(&"b".repeat(20)));
        check_ranges(&doc, &chunks);
    }

    #[test]
    fn an_oversized_paragraph_is_cut_at_a_sentence_end() {
        let sentence = format!("{}. ", "word ".repeat(9).trim_end());
        let doc = doc(vec![Block::paragraph(sentence.repeat(4).trim_end())]);
        let chunks = chunk(&doc, &opts(120, 0));

        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.text.chars().count() <= 120, "{}", chunk.text);
            assert!(
                chunk.text.ends_with('.'),
                "cut mid-sentence: {}",
                chunk.text
            );
        }
        check_ranges(&doc, &chunks);
    }

    #[test]
    fn an_oversized_block_without_sentences_is_cut_between_words() {
        let doc = doc(vec![Block::paragraph(
            "alpha bravo charlie delta echo foxtrot",
        )]);
        let chunks = chunk(&doc, &opts(20, 0));

        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(!chunk.text.starts_with(' ') && !chunk.text.ends_with(' '));
            assert!(chunk.text.split_whitespace().all(|word| {
                ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"].contains(&word)
            }));
        }
        check_ranges(&doc, &chunks);
    }

    #[test]
    fn a_single_unbreakable_run_is_cut_on_a_character_boundary() {
        let doc = doc(vec![Block::paragraph("é".repeat(50))]);
        let chunks = chunk(&doc, &opts(20, 0));
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.text.chars().count() <= 20));
        check_ranges(&doc, &chunks);
    }

    #[test]
    fn chunks_carry_the_source_path() {
        let doc = Document {
            meta: Metadata {
                source_path: Some("handbook.pdf".into()),
                ..Metadata::default()
            },
            blocks: vec![Block::paragraph("hello")],
        };
        let chunks = chunk(&doc, &ChunkOpts::default());
        assert_eq!(chunks[0].source.as_deref(), Some("handbook.pdf"));
    }

    /// The chunk JSON schema is a public contract — this is what breaks when a
    /// field is renamed.
    #[test]
    fn json_schema_snapshot() {
        let chunk = Chunk {
            text: "Body.".into(),
            heading_path: vec!["Introduction".into()],
            source: Some("paper.pdf".into()),
            byte_range: (0, 5),
        };
        assert_eq!(
            serde_json::to_value(&chunk).unwrap(),
            serde_json::json!({
                "text": "Body.",
                "heading_path": ["Introduction"],
                "source": "paper.pdf",
                "byte_range": [0, 5],
            })
        );
    }
}
