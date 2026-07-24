//! Plain-text extractor — blank-line separated paragraphs.
//!
//! Minimal on purpose: it exists so the pipeline (`detect` → `extract` →
//! `render`) is end-to-end runnable from day one. Phase 2 gives Markdown its
//! own extractor and adds csv/html/rtf next to this one.

use std::path::Path;

use crate::detect::{Format, Sniff};
use crate::error::{Error, Result};
use crate::extract::{ExtractOpts, Extractor};
use crate::model::{Block, Document, Metadata};

pub struct TextExtractor;

impl Extractor for TextExtractor {
    fn name(&self) -> &'static str {
        "text"
    }

    fn supports(&self, _path: &Path, sniff: &Sniff) -> bool {
        matches!(
            crate::detect::detect(sniff),
            Some(Format::Txt | Format::Markdown)
        )
    }

    fn extract(&self, path: &Path, _opts: &ExtractOpts) -> Result<Document> {
        let raw = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        let text = String::from_utf8_lossy(&raw);

        let format = crate::detect::detect_path(path).unwrap_or(Format::Txt);
        let meta = Metadata {
            source_format: Some(format),
            source_path: Some(path.display().to_string()),
            ..Metadata::default()
        };

        Ok(Document {
            meta,
            blocks: paragraphs(&text),
        })
    }
}

/// Split plain text into paragraphs on blank lines. Pure.
fn paragraphs(text: &str) -> Vec<Block> {
    text.split("\n\n")
        .map(|chunk| chunk.trim())
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| Block::paragraph(chunk.replace("\r\n", "\n")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_blank_lines() {
        let blocks = paragraphs("first line\nstill first\n\n\nsecond\n");
        assert_eq!(
            blocks,
            vec![
                Block::paragraph("first line\nstill first"),
                Block::paragraph("second"),
            ]
        );
    }

    #[test]
    fn empty_input_yields_no_blocks() {
        assert!(paragraphs("   \n\n  ").is_empty());
    }
}
