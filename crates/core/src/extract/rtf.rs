//! RTF extractor — a small reader for the parts of RTF that carry text.
//!
//! Why not the `rtf-parser` crate: as of 0.4.3 it never handles the `\par`
//! control word, so every paragraph boundary is lost and a document collapses
//! into one blob — the one thing a Markdown extractor cannot afford. Its default
//! features also pull `wasm-bindgen` into the binary. RTF's grammar is small
//! enough that reading it directly is both cheaper and more accurate.
//!
//! Scope: groups, destinations, character escapes, `\uN` unicode, bold/italic,
//! and the breaks (`\par`, `\line`, `\cell`, `\row`). Formatting we cannot
//! represent — fonts, colours, sizes — is skipped, and headings are not guessed
//! from point sizes.

use std::path::Path;

use crate::detect::{Format, Sniff};
use crate::error::{Error, Result};
use crate::extract::{ExtractOpts, Extractor};
use crate::model::{Block, Document, Inline, Metadata, Span};

pub struct RtfExtractor;

impl Extractor for RtfExtractor {
    fn name(&self) -> &'static str {
        "rtf"
    }

    fn supports(&self, _path: &Path, sniff: &Sniff) -> bool {
        crate::detect::detect(sniff) == Some(Format::Rtf)
    }

    fn extract(&self, path: &Path, _opts: &ExtractOpts) -> Result<Document> {
        let raw = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        // RTF is 7-bit ASCII with escapes, but files in the wild carry raw high
        // bytes; lossy decoding keeps those documents usable.
        let text = String::from_utf8_lossy(&raw);
        let blocks = parse(&text).map_err(|message| Error::parse(path, message))?;

        Ok(Document {
            meta: Metadata {
                source_format: Some(Format::Rtf),
                source_path: Some(path.display().to_string()),
                ..Metadata::default()
            },
            blocks,
        })
    }
}

/// Group state — inherited by nested groups, restored when a group closes.
#[derive(Debug, Clone)]
struct State {
    bold: bool,
    italic: bool,
    /// Inside a destination whose content is not document text.
    skip: bool,
    /// How many characters follow a `\uN` as its non-unicode fallback.
    unicode_fallback: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            skip: false,
            unicode_fallback: 1,
        }
    }
}

/// Destinations that hold metadata or binary payloads rather than text.
fn is_skipped_destination(word: &str) -> bool {
    matches!(
        word,
        "fonttbl"
            | "colortbl"
            | "stylesheet"
            | "listtable"
            | "listoverridetable"
            | "filetbl"
            | "rsidtbl"
            | "generator"
            | "info"
            | "pict"
            | "object"
            | "themedata"
            | "datastore"
            | "latentstyles"
            | "xmlnstbl"
            | "fldinst"
            | "revtbl"
            | "upr"
    )
}

/// Parse RTF source into blocks. Pure.
pub fn parse(source: &str) -> std::result::Result<Vec<Block>, String> {
    if !source.trim_start().starts_with("{\\rtf") {
        return Err("not an RTF document (no {\\rtf header)".to_string());
    }

    let mut reader = Reader::new(source);
    reader.run();
    Ok(reader.finish())
}

struct Reader<'a> {
    chars: Vec<char>,
    position: usize,
    stack: Vec<State>,
    /// Text of the run being accumulated, under the current styling.
    run: String,
    /// Spans of the paragraph being accumulated.
    spans: Vec<Span>,
    blocks: Vec<Block>,
    /// Characters still to swallow as a `\uN` fallback.
    pending_fallback: usize,
    source: std::marker::PhantomData<&'a str>,
}

impl Reader<'_> {
    fn new(source: &str) -> Reader<'_> {
        Reader {
            chars: source.chars().collect(),
            position: 0,
            stack: vec![State::default()],
            run: String::new(),
            spans: Vec::new(),
            blocks: Vec::new(),
            pending_fallback: 0,
            source: std::marker::PhantomData,
        }
    }

    fn state(&self) -> &State {
        self.stack.last().expect("the stack is never empty")
    }

    fn state_mut(&mut self) -> &mut State {
        self.stack.last_mut().expect("the stack is never empty")
    }

    fn run(&mut self) {
        while let Some(ch) = self.chars.get(self.position).copied() {
            self.position += 1;
            match ch {
                '{' => {
                    self.flush_run();
                    let state = self.state().clone();
                    self.stack.push(state);
                }
                '}' => {
                    self.flush_run();
                    if self.stack.len() > 1 {
                        self.stack.pop();
                    }
                }
                '\\' => self.control(),
                // Raw line endings are formatting in the source file, not text.
                '\r' | '\n' => {}
                _ => self.push_char(ch),
            }
        }
    }

    /// Read one control word or control symbol, starting after the backslash.
    fn control(&mut self) {
        let Some(&first) = self.chars.get(self.position) else {
            return;
        };

        if !first.is_ascii_alphabetic() {
            self.position += 1;
            self.control_symbol(first);
            return;
        }

        let start = self.position;
        while self
            .chars
            .get(self.position)
            .is_some_and(|c| c.is_ascii_alphabetic())
        {
            self.position += 1;
        }
        let word: String = self.chars[start..self.position].iter().collect();

        // Optional numeric parameter, possibly negative.
        let mut parameter: Option<i32> = None;
        let number_start = self.position;
        if self.chars.get(self.position) == Some(&'-') {
            self.position += 1;
        }
        while self
            .chars
            .get(self.position)
            .is_some_and(char::is_ascii_digit)
        {
            self.position += 1;
        }
        if self.position > number_start {
            let digits: String = self.chars[number_start..self.position].iter().collect();
            parameter = digits.parse().ok();
        }

        // A single space after a control word is a delimiter, not text.
        if self.chars.get(self.position) == Some(&' ') {
            self.position += 1;
        }

        self.control_word(&word, parameter);
    }

    fn control_word(&mut self, word: &str, parameter: Option<i32>) {
        let on = parameter != Some(0);

        match word {
            "par" | "sect" => self.end_paragraph(),
            "line" => {
                self.flush_run();
                self.spans.push(Span::LineBreak);
            }
            // Table cells become tab-separated text; RTF table structure is a
            // later phase, and mangled pipes would be worse than plain columns.
            "cell" | "tab" => self.push_char('\t'),
            "row" => self.end_paragraph(),
            // Styling changes end the current run — otherwise the text before
            // and after `\b0` would share one span.
            "b" => {
                self.flush_run();
                self.state_mut().bold = on;
            }
            "i" => {
                self.flush_run();
                self.state_mut().italic = on;
            }
            "plain" => {
                self.flush_run();
                let state = self.state_mut();
                state.bold = false;
                state.italic = false;
            }
            "uc" => {
                if let Some(count) = parameter {
                    self.state_mut().unicode_fallback = count.max(0) as usize;
                }
            }
            "u" => {
                if let Some(code) = parameter {
                    // Word writes codepoints above 0x7FFF as negative numbers.
                    let code = if code < 0 {
                        (code + 65536) as u32
                    } else {
                        code as u32
                    };
                    if let Some(ch) = char::from_u32(code) {
                        self.push_char(ch);
                    }
                }
                self.pending_fallback = self.state().unicode_fallback;
            }
            "emdash" => self.push_char('—'),
            "endash" => self.push_char('–'),
            "bullet" => self.push_char('•'),
            "lquote" => self.push_char('‘'),
            "rquote" => self.push_char('’'),
            "ldblquote" => self.push_char('“'),
            "rdblquote" => self.push_char('”'),
            word if is_skipped_destination(word) => self.state_mut().skip = true,
            // Everything else is formatting we do not model.
            _ => {}
        }
    }

    fn control_symbol(&mut self, symbol: char) {
        match symbol {
            '\\' | '{' | '}' => self.push_char(symbol),
            // `{\*\foo …}` marks a destination the reader may ignore wholesale.
            '*' => self.state_mut().skip = true,
            '~' => self.push_char('\u{00a0}'),
            '_' => self.push_char('-'),
            '\r' | '\n' => self.end_paragraph(),
            '\'' => {
                let hex: String = self
                    .chars
                    .get(self.position..self.position + 2)
                    .map(|slice| slice.iter().collect())
                    .unwrap_or_default();
                self.position += hex.len();
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    self.push_char(cp1252(byte));
                }
            }
            // `\-` (optional hyphen) and friends leave no text behind.
            _ => {}
        }
    }

    fn push_char(&mut self, ch: char) {
        if self.pending_fallback > 0 {
            self.pending_fallback -= 1;
            return;
        }
        if self.state().skip {
            return;
        }
        self.run.push(ch);
    }

    /// Close the current styled run.
    fn flush_run(&mut self) {
        if self.run.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.run);
        let state = self.state();
        self.spans.push(style(&text, state.bold, state.italic));
    }

    fn end_paragraph(&mut self) {
        self.flush_run();
        let inline = Inline::new(std::mem::take(&mut self.spans));
        if !inline.plain_text().trim().is_empty() {
            self.blocks.push(Block::Paragraph { text: inline });
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.end_paragraph();
        self.blocks
    }
}

/// Wrap a run of text in the character styling RTF reported for it.
fn style(text: &str, bold: bool, italic: bool) -> Span {
    let span = Span::Text(text.to_string());
    match (bold, italic) {
        (false, false) => span,
        (true, false) => Span::Bold(vec![span]),
        (false, true) => Span::Italic(vec![span]),
        (true, true) => Span::Bold(vec![Span::Italic(vec![span])]),
    }
}

/// Decode a `\'hh` byte as Windows-1252, the codepage almost every RTF uses.
fn cp1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9f => HIGH[(byte - 0x80) as usize],
        // Everything else matches Latin-1, which is Unicode's first block.
        _ => char::from(byte),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = r"{\rtf1\ansi\deff0{\fonttbl{\f0 Helvetica;}}\pard ";

    fn parse_body(body: &str) -> Vec<Block> {
        parse(&format!("{HEADER}{body}}}")).expect("valid rtf")
    }

    #[test]
    fn paragraphs_split_on_par() {
        assert_eq!(
            parse_body(r"first\par second\par "),
            vec![Block::paragraph("first"), Block::paragraph("second")]
        );
    }

    #[test]
    fn bold_and_italic_runs_survive() {
        let blocks = parse_body(r"plain {\b bold} {\i italic}\par ");
        let Block::Paragraph { text } = &blocks[0] else {
            panic!("expected a paragraph, got {blocks:?}");
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
    fn styling_switches_off_with_a_zero_parameter() {
        let blocks = parse_body(r"\b bold\b0  plain\par ");
        let Block::Paragraph { text } = &blocks[0] else {
            panic!("expected a paragraph");
        };
        assert_eq!(
            text.spans,
            vec![
                Span::Bold(vec![Span::Text("bold".into())]),
                Span::Text(" plain".into()),
            ]
        );
    }

    #[test]
    fn font_and_colour_tables_are_not_text() {
        let blocks = parse_body(r"{\colortbl;\red255\green0\blue0;}visible\par ");
        assert_eq!(blocks, vec![Block::paragraph("visible")]);
    }

    #[test]
    fn ignorable_destinations_are_skipped() {
        assert_eq!(
            parse_body(r"{\*\generator Riched20 10.0}kept\par "),
            vec![Block::paragraph("kept")]
        );
    }

    #[test]
    fn escapes_and_unicode_are_decoded() {
        // `\'e9` is a Windows-1252 byte; `႐?` is a codepoint followed by
        // the ASCII fallback character, which must be swallowed.
        assert_eq!(
            parse_body(r"caf\'e9 \u1090? \\ \{ \} \emdash\par "),
            vec![Block::paragraph("café т \\ { } —")]
        );
    }

    #[test]
    fn line_breaks_stay_inside_a_paragraph() {
        let blocks = parse_body(r"first\line second\par ");
        let Block::Paragraph { text } = &blocks[0] else {
            panic!("expected a paragraph");
        };
        assert_eq!(
            text.spans,
            vec![
                Span::Text("first".into()),
                Span::LineBreak,
                Span::Text("second".into()),
            ]
        );
    }

    #[test]
    fn table_cells_become_tab_separated_lines() {
        assert_eq!(
            parse_body(r"a\cell b\cell \row "),
            vec![Block::paragraph("a\tb\t")]
        );
    }

    #[test]
    fn trailing_text_without_a_final_par_is_kept() {
        assert_eq!(
            parse_body("no trailing par"),
            vec![Block::paragraph("no trailing par")]
        );
    }

    #[test]
    fn blank_paragraphs_are_dropped() {
        assert_eq!(
            parse_body(r"text\par \par \par "),
            vec![Block::paragraph("text")]
        );
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(parse("not rtf at all").is_err());
    }
}
