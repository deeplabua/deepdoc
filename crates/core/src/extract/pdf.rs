//! PDF extractor — born-digital text, reconstructed from glyph positions.
//!
//! A PDF has no paragraphs, no headings and no reading order: it has glyphs at
//! coordinates. Everything structural here is a reconstruction, so the shape of
//! this module is a pipeline of increasingly opinionated steps:
//!
//! 1. `Collector` receives every character with its position and size from
//!    `pdf-extract`, which has already done the hard part — resolving font
//!    encodings and CMaps into real text.
//! 2. characters group into lines by their baseline;
//! 3. lines group into columns by their horizontal spans;
//! 4. lines group into paragraphs by vertical gaps and ragged right edges;
//! 5. a line noticeably larger than the page's body text becomes a heading.
//!
//! Tables are deliberately not reconstructed: a wrong table is worse to read
//! than the paragraphs it was made from, and the PRD puts complex tabular PDF
//! outside v0.1's lane. Scanned pages yield no characters at all, which the
//! caller turns into the "looks like a scan" exit code.

use std::path::Path;

use pdf_extract::{Document, MediaBox, OutputDev, OutputError, Transform};

use crate::detect::{Format, Sniff};
use crate::error::{Error, Result};
use crate::extract::{ExtractOpts, Extractor, PageRange};
use crate::model::{Block, Document as Doc, Inline, Metadata};

pub struct PdfExtractor;

impl Extractor for PdfExtractor {
    fn name(&self) -> &'static str {
        "pdf"
    }

    fn supports(&self, _path: &Path, sniff: &Sniff) -> bool {
        crate::detect::detect(sniff) == Some(Format::Pdf)
    }

    fn extract(&self, path: &Path, opts: &ExtractOpts) -> Result<Doc> {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        let parsed = parse(&bytes, opts.pages).map_err(|m| Error::parse(path, m))?;

        let mut meta = parsed.meta;
        meta.source_format = Some(Format::Pdf);
        meta.source_path = Some(path.display().to_string());

        Ok(Doc {
            meta,
            blocks: parsed.blocks,
        })
    }
}

/// What a parsed PDF yields.
#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub meta: Metadata,
    pub blocks: Vec<Block>,
}

/// Parse PDF bytes, optionally limited to a page range.
pub fn parse(bytes: &[u8], pages: Option<PageRange>) -> std::result::Result<Parsed, String> {
    let document = Document::load_mem(bytes).map_err(|e| e.to_string())?;

    let numbers: Vec<u32> = document
        .get_pages()
        .keys()
        .copied()
        .filter(|number| pages.is_none_or(|range| range.contains(*number)))
        .collect();

    let mut collector = Collector::default();
    for number in &numbers {
        // A page that cannot be rendered should not lose the rest of the file.
        let _ = pdf_extract::output_doc_page(&document, &mut collector, *number);
    }

    let mut blocks = Vec::new();
    for page in collector.pages {
        if page.number != numbers.first().copied().unwrap_or(1) {
            blocks.push(Block::PageBreak { page: page.number });
        }
        blocks.extend(page_blocks(&page.characters));
    }

    Ok(Parsed {
        meta: metadata(&document),
        blocks,
    })
}

// ------------------------------------------------------------- collection --

/// One character, placed on the page.
#[derive(Debug, Clone)]
struct Glyph {
    x: f64,
    /// Distance from the top of the page: sorting by it reads top to bottom.
    y: f64,
    size: f64,
    /// Where the pen ended up after drawing this character.
    end: f64,
    text: String,
}

struct PageGlyphs {
    number: u32,
    characters: Vec<Glyph>,
}

#[derive(Default)]
struct Collector {
    pages: Vec<PageGlyphs>,
    current: Vec<Glyph>,
    number: u32,
    flip: Transform,
}

impl OutputDev for Collector {
    fn begin_page(
        &mut self,
        page_num: u32,
        media_box: &MediaBox,
        _art_box: Option<(f64, f64, f64, f64)>,
    ) -> std::result::Result<(), OutputError> {
        self.number = page_num;
        self.current = Vec::new();
        // PDF's origin is the bottom-left corner; flipping y makes "smaller
        // means higher up the page", which is the order text is read in.
        self.flip = Transform::row_major(1., 0., 0., -1., 0., media_box.ury - media_box.lly);
        Ok(())
    }

    fn end_page(&mut self) -> std::result::Result<(), OutputError> {
        self.pages.push(PageGlyphs {
            number: self.number,
            characters: std::mem::take(&mut self.current),
        });
        Ok(())
    }

    fn output_character(
        &mut self,
        trm: &Transform,
        width: f64,
        _spacing: f64,
        font_size: f64,
        char: &str,
    ) -> std::result::Result<(), OutputError> {
        let position = trm.post_transform(&self.flip);

        // The text matrix may scale and skew, so the visual size is the side of
        // a square with the same area as the transformed em box. This is the
        // same calculation pdf-extract makes for its own plain-text output,
        // written out from the matrix so its geometry crate stays its own
        // business rather than a dependency of ours.
        let width_vector = font_size * (trm.m11 + trm.m21);
        let height_vector = font_size * (trm.m12 + trm.m22);
        let size = (width_vector * height_vector).abs().sqrt();

        let x = position.m31;
        self.current.push(Glyph {
            x,
            y: position.m32,
            size,
            end: x + width * size,
            text: char.to_string(),
        });
        Ok(())
    }

    fn begin_word(&mut self) -> std::result::Result<(), OutputError> {
        Ok(())
    }

    fn end_word(&mut self) -> std::result::Result<(), OutputError> {
        Ok(())
    }

    fn end_line(&mut self) -> std::result::Result<(), OutputError> {
        Ok(())
    }
}

// ----------------------------------------------------------------- layout --

/// A run of characters sharing a baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub x: f64,
    pub right: f64,
    pub y: f64,
    pub size: f64,
}

/// Turn one page's glyphs into blocks.
///
/// The page is segmented before any lines are assembled: two columns share
/// baselines, so grouping by baseline first would weld them into single lines.
fn page_blocks(characters: &[Glyph]) -> Vec<Block> {
    let body = glyph_size(characters);

    let regions: Vec<Vec<Line>> = segment(characters, body, 0)
        .iter()
        .map(|glyphs| lines(glyphs))
        .filter(|lines| !lines.is_empty())
        .collect();

    let all: Vec<Line> = regions.iter().flatten().cloned().collect();
    if all.is_empty() {
        return Vec::new();
    }
    let body_size = body_size(&all);

    regions
        .iter()
        .flat_map(|region| paragraphs(region, body_size))
        .collect()
}

/// Cut a page into regions, in reading order — the classic XY-cut.
///
/// Horizontal cuts come first, so a full-width heading or introduction above a
/// two-column layout is separated from the columns instead of bridging their
/// gutter and hiding it. Each band is then cut vertically where a gutter is
/// wide enough to be deliberate, and the halves are cut again.
fn segment(characters: &[Glyph], body: f64, depth: usize) -> Vec<Vec<Glyph>> {
    let single = || vec![characters.to_vec()];
    // Deep enough, or too little text to tell structure from coincidence.
    if depth >= 6 || characters.len() < 40 {
        return single();
    }

    if let Some((top, bottom)) = split_horizontally(characters, body) {
        let mut regions = segment(&top, body, depth + 1);
        regions.extend(segment(&bottom, body, depth + 1));
        return regions;
    }

    match split_vertically(characters, body) {
        Some(columns) => columns
            .iter()
            .flat_map(|column| segment(column, body, depth + 1))
            .collect(),
        None => single(),
    }
}

/// Split at the first horizontal gap wider than the space between lines.
fn split_horizontally(characters: &[Glyph], body: f64) -> Option<(Vec<Glyph>, Vec<Glyph>)> {
    let mut baselines: Vec<f64> = characters.iter().map(|glyph| glyph.y).collect();
    baselines.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    baselines.dedup_by(|a, b| (*a - *b).abs() < body * 0.5);

    // Comfortably more than the leading between lines of a paragraph.
    let threshold = body * 2.5;
    let cut = baselines
        .windows(2)
        .find(|pair| pair[1] - pair[0] > threshold)
        .map(|pair| (pair[0] + pair[1]) / 2.0)?;

    let (top, bottom): (Vec<Glyph>, Vec<Glyph>) =
        characters.iter().cloned().partition(|glyph| glyph.y < cut);
    (!top.is_empty() && !bottom.is_empty()).then_some((top, bottom))
}

/// Split into columns where a gutter runs down the whole region.
fn split_vertically(characters: &[Glyph], body: f64) -> Option<Vec<Vec<Glyph>>> {
    let spans: Vec<(f64, f64)> = characters.iter().map(|g| (g.x, g.end)).collect();
    let bounds = column_bounds(&spans, body);
    if bounds.len() < 2 {
        return None;
    }

    let mut columns: Vec<Vec<Glyph>> = vec![Vec::new(); bounds.len()];
    for glyph in characters {
        let index = bounds
            .iter()
            .position(|(start, end)| glyph.x >= *start && glyph.x <= *end)
            .unwrap_or(0);
        columns[index].push(glyph.clone());
    }

    // A sliver holding a page number or a margin note is not a column.
    let minimum = (characters.len() / 10).max(20);
    columns
        .iter()
        .all(|column| column.len() >= minimum)
        .then_some(columns)
}

/// The typical glyph size in a region — a per-character median, so the sheer
/// amount of body text outweighs a heading.
fn glyph_size(characters: &[Glyph]) -> f64 {
    if characters.is_empty() {
        return 0.0;
    }
    let mut sizes: Vec<f64> = characters.iter().map(|glyph| glyph.size).collect();
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sizes[sizes.len() / 2]
}

/// Group characters into lines, and each line's characters into words.
fn lines(characters: &[Glyph]) -> Vec<Line> {
    if characters.is_empty() {
        return Vec::new();
    }

    let mut sorted = characters.to_vec();
    sorted.sort_by(|a, b| {
        a.y.partial_cmp(&b.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut lines: Vec<Vec<Glyph>> = Vec::new();
    for glyph in sorted {
        // Baselines wobble; anything within a fraction of the type size is the
        // same line, including the odd superscript.
        let same_line = lines.last().is_some_and(|line| {
            line.last()
                .is_some_and(|last| (glyph.y - last.y).abs() <= last.size.max(glyph.size) * 0.5)
        });
        if same_line {
            lines.last_mut().expect("checked").push(glyph);
        } else {
            lines.push(vec![glyph]);
        }
    }

    lines.iter().filter_map(|line| assemble(line)).collect()
}

/// Join a line's characters, inserting the spaces the PDF only implies.
fn assemble(characters: &[Glyph]) -> Option<Line> {
    let mut sorted = characters.to_vec();
    sorted.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));

    let mut text = String::new();
    let mut previous_end: Option<f64> = None;
    for glyph in &sorted {
        if let Some(end) = previous_end
            && glyph.x > end + glyph.size * 0.2
            && !text.ends_with(' ')
            && !glyph.text.starts_with(' ')
        {
            text.push(' ');
        }
        text.push_str(&glyph.text);
        previous_end = Some(glyph.end);
    }

    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }

    // The tallest glyph decides the line's size: a dropped capital or a
    // superscript should not shrink a heading.
    let size = sorted
        .iter()
        .map(|glyph| glyph.size)
        .fold(0.0f64, f64::max)
        .max(1.0);

    Some(Line {
        text,
        x: sorted.first()?.x,
        right: sorted.iter().map(|g| g.end).fold(f64::MIN, f64::max),
        y: sorted.first()?.y,
        size,
    })
}

/// Find column boundaries in a set of horizontal spans.
///
/// The spans are merged, and a gap wide enough to be deliberate rather than a
/// wide word space becomes a gutter. One entry back means a single column.
pub fn column_bounds(spans: &[(f64, f64)], body_size: f64) -> Vec<(f64, f64)> {
    if spans.is_empty() {
        return Vec::new();
    }

    let mut spans = spans.to_vec();
    spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }

    let width = merged.last().map(|span| span.1).unwrap_or_default() - merged[0].0;
    let gutter = (width * 0.04).max(body_size * 2.0);

    let mut bounds: Vec<(f64, f64)> = vec![merged[0]];
    for span in &merged[1..] {
        let last = bounds.last_mut().expect("seeded above");
        if span.0 - last.1 >= gutter {
            bounds.push(*span);
        } else {
            last.1 = last.1.max(span.1);
        }
    }
    bounds
}

/// Group a column's lines into paragraphs and headings.
fn paragraphs(lines: &[Line], body_size: f64) -> Vec<Block> {
    let column_right = lines.iter().map(|line| line.right).fold(f64::MIN, f64::max);

    let mut blocks = Vec::new();
    let mut current: Vec<&Line> = Vec::new();

    for line in lines {
        let starts_new = match current.last() {
            None => true,
            Some(previous) => {
                let gap = line.y - previous.y;
                // A wider than usual gap, a line that stopped short of the
                // column edge, or a change of type size all end a paragraph.
                gap > previous.size * 1.8
                    || previous.right < column_right - previous.size * 2.0
                    || (line.size - previous.size).abs() > previous.size * 0.2
            }
        };

        if starts_new && !current.is_empty() {
            blocks.push(block(&current, body_size));
            current.clear();
        }
        current.push(line);
    }
    if !current.is_empty() {
        blocks.push(block(&current, body_size));
    }

    blocks
}

/// One paragraph's lines as a block — a heading when the type is larger.
fn block(lines: &[&Line], body_size: f64) -> Block {
    let mut text = String::new();
    for line in lines {
        if text.is_empty() {
            text.push_str(&line.text);
            continue;
        }
        // A word broken across lines is rejoined; anything else gets a space.
        if let Some(stem) = text.strip_suffix('-')
            && stem.ends_with(char::is_alphabetic)
            && line.text.starts_with(char::is_lowercase)
        {
            text = stem.to_string();
            text.push_str(&line.text);
        } else {
            text.push(' ');
            text.push_str(&line.text);
        }
    }

    let size = lines.iter().map(|line| line.size).fold(0.0f64, f64::max);
    match heading_level(size, body_size, lines.len(), &text) {
        Some(level) => Block::Heading {
            level,
            text: Inline::text(text),
        },
        None => Block::Paragraph {
            text: Inline::text(text),
        },
    }
}

/// Headings are guessed from type size, conservatively: a false heading is
/// worse than a missed one, so a long or same-sized block stays a paragraph.
fn heading_level(size: f64, body_size: f64, line_count: usize, text: &str) -> Option<u8> {
    if body_size <= 0.0 || line_count > 2 || text.chars().count() > 120 {
        return None;
    }
    let ratio = size / body_size;
    match ratio {
        r if r >= 1.7 => Some(1),
        r if r >= 1.35 => Some(2),
        r if r >= 1.15 => Some(3),
        _ => None,
    }
}

/// The page's body type size: the size that carries the most text.
///
/// Weighing by characters rather than by lines is what keeps a two-line page —
/// a title and one sentence — from deciding that the title is the body.
fn body_size(lines: &[Line]) -> f64 {
    let mut buckets: Vec<(f64, usize)> = Vec::new();

    for line in lines {
        // Half a point is finer than any real distinction between type sizes.
        let key = (line.size * 2.0).round() / 2.0;
        let characters = line.text.chars().count();
        match buckets.iter_mut().find(|(size, _)| *size == key) {
            Some(bucket) => bucket.1 += characters,
            None => buckets.push((key, characters)),
        }
    }

    buckets
        .into_iter()
        .max_by_key(|(_, characters)| *characters)
        .map(|(size, _)| size)
        .unwrap_or(0.0)
}

// --------------------------------------------------------------- metadata --

/// Title, author and creation date from the document information dictionary.
fn metadata(document: &Document) -> Metadata {
    let mut meta = Metadata {
        page_count: Some(document.get_pages().len() as u32),
        ..Metadata::default()
    };

    let Some(info) = document
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|object| document.dereference(object).ok())
        .and_then(|(_, object)| object.as_dict().ok())
    else {
        return meta;
    };

    let text_of = |key: &[u8]| {
        info.get(key)
            .ok()
            .and_then(|object| object.as_str().ok())
            .map(decode_text_string)
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    };

    meta.title = text_of(b"Title");
    meta.author = text_of(b"Author");
    meta.created = text_of(b"CreationDate").as_deref().and_then(pdf_date);
    meta
}

/// A PDF text string is either PDFDocEncoded (close enough to Latin-1) or
/// UTF-16BE with a byte order mark.
fn decode_text_string(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xfe, 0xff]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

/// `D:20260724103000+02'00'` → `2026-07-24`.
fn pdf_date(value: &str) -> Option<String> {
    let digits: String = value
        .trim_start_matches("D:")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.len() < 8 {
        return None;
    }
    Some(format!(
        "{}-{}-{}",
        &digits[0..4],
        &digits[4..6],
        &digits[6..8]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f64, right: f64, y: f64, size: f64) -> Line {
        Line {
            text: text.to_string(),
            x,
            right,
            y,
            size,
        }
    }

    #[test]
    fn lines_join_into_a_paragraph_until_the_gap_widens() {
        let lines = [
            line(
                "Revenue grew twelve per cent quarter",
                72.0,
                500.0,
                100.0,
                10.0,
            ),
            line("over quarter, driven by cloud.", 72.0, 300.0, 112.0, 10.0),
            line("A separate thought entirely.", 72.0, 320.0, 160.0, 10.0),
        ];

        assert_eq!(
            paragraphs(&lines, 10.0),
            vec![
                Block::paragraph(
                    "Revenue grew twelve per cent quarter over quarter, driven by cloud."
                ),
                Block::paragraph("A separate thought entirely."),
            ]
        );
    }

    #[test]
    fn a_word_broken_across_lines_is_rejoined() {
        let lines = [
            line("The extraordinary quarterly per-", 72.0, 500.0, 100.0, 10.0),
            line("formance continued.", 72.0, 300.0, 112.0, 10.0),
        ];
        assert_eq!(
            paragraphs(&lines, 10.0),
            vec![Block::paragraph(
                "The extraordinary quarterly performance continued."
            )]
        );
    }

    #[test]
    fn larger_type_becomes_a_heading() {
        let lines = [
            line("Quarterly Report", 72.0, 300.0, 60.0, 24.0),
            line("Summary", 72.0, 200.0, 90.0, 14.0),
            line("Revenue grew.", 72.0, 400.0, 120.0, 10.0),
            line("Cash flow held.", 72.0, 400.0, 132.0, 10.0),
        ];

        assert_eq!(
            paragraphs(&lines, 10.0),
            vec![
                Block::Heading {
                    level: 1,
                    text: Inline::text("Quarterly Report")
                },
                Block::Heading {
                    level: 2,
                    text: Inline::text("Summary")
                },
                Block::paragraph("Revenue grew. Cash flow held."),
            ]
        );
    }

    #[test]
    fn headings_stay_conservative() {
        // Same size as the body: not a heading, however short.
        assert_eq!(heading_level(10.0, 10.0, 1, "Short"), None);
        // Large but long: prose in a big font is still prose.
        let long = "x".repeat(200);
        assert_eq!(heading_level(24.0, 10.0, 1, &long), None);
        // Large and short: a heading.
        assert_eq!(heading_level(24.0, 10.0, 1, "Short"), Some(1));
    }

    #[test]
    fn a_gutter_splits_the_page_into_columns() {
        // Two blocks of spans with a wide gap between them.
        let mut spans: Vec<(f64, f64)> = (0..6).map(|_| (72.0, 280.0)).collect();
        spans.extend((0..6).map(|_| (330.0, 530.0)));

        let bounds = column_bounds(&spans, 10.0);
        assert_eq!(bounds.len(), 2, "expected two columns: {bounds:?}");
        assert!(bounds[0].1 < bounds[1].0);
    }

    #[test]
    fn a_single_column_page_stays_one_flow() {
        let spans: Vec<(f64, f64)> = (0..8).map(|_| (72.0, 530.0)).collect();
        assert_eq!(column_bounds(&spans, 10.0).len(), 1);
    }

    #[test]
    fn an_ordinary_word_space_is_not_a_gutter() {
        // Spans broken by spaces a few points wide must stay one column.
        let spans = [(72.0, 200.0), (205.0, 330.0), (336.0, 530.0)];
        assert_eq!(column_bounds(&spans, 10.0).len(), 1);
    }

    fn glyph(x: f64, y: f64, size: f64) -> Glyph {
        Glyph {
            x,
            y,
            size,
            end: x + size * 0.5,
            text: "x".to_string(),
        }
    }

    #[test]
    fn a_wide_vertical_gap_starts_a_new_band() {
        // Two blocks of lines with an empty stretch between them.
        let mut characters: Vec<Glyph> = Vec::new();
        for row in 0..3 {
            for column in 0..10 {
                characters.push(glyph(
                    72.0 + column as f64 * 6.0,
                    100.0 + row as f64 * 12.0,
                    10.0,
                ));
            }
        }
        for row in 0..3 {
            for column in 0..10 {
                characters.push(glyph(
                    72.0 + column as f64 * 6.0,
                    300.0 + row as f64 * 12.0,
                    10.0,
                ));
            }
        }

        let (top, bottom) = split_horizontally(&characters, 10.0).expect("a band boundary");
        assert_eq!(top.len(), 30);
        assert_eq!(bottom.len(), 30);
    }

    #[test]
    fn ordinary_line_spacing_is_not_a_band_boundary() {
        let characters: Vec<Glyph> = (0..6)
            .flat_map(|row| {
                (0..10).map(move |column| {
                    glyph(72.0 + column as f64 * 6.0, 100.0 + row as f64 * 12.0, 10.0)
                })
            })
            .collect();
        assert!(split_horizontally(&characters, 10.0).is_none());
    }

    #[test]
    fn decodes_pdf_text_strings() {
        assert_eq!(decode_text_string(b"Plain"), "Plain");
        let utf16: Vec<u8> = [0xfe, 0xff, 0x00, 0x48, 0x00, 0x69].to_vec();
        assert_eq!(decode_text_string(&utf16), "Hi");
    }

    #[test]
    fn parses_pdf_dates() {
        assert_eq!(
            pdf_date("D:20260724103000+02'00'").as_deref(),
            Some("2026-07-24")
        );
        assert_eq!(pdf_date("20260724").as_deref(), Some("2026-07-24"));
        assert_eq!(pdf_date("D:2026").as_deref(), None);
    }
}
