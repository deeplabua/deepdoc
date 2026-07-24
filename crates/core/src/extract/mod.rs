//! The `Extractor` trait and the registry that maps a [`Format`] onto one.
//!
//! Extraction is the single impure edge of the core: it reads files and parses
//! them into a [`Document`]. Everything downstream (`render`, `chunk`) is pure.

pub mod container;
pub mod csv;
pub mod epub;
pub mod html;
pub mod markdown;
pub mod odf;
pub mod ooxml;
pub mod pdf;
pub mod rtf;
pub mod spreadsheet;
pub mod text;
pub mod xml;

use std::path::Path;

use crate::detect::{Format, Sniff};
use crate::error::{Error, Result};
use crate::model::Document;

/// Options that influence extraction.
#[derive(Debug, Clone, Default)]
pub struct ExtractOpts {
    /// Page subset to extract, for paginated formats (PDF). `None` = all pages.
    pub pages: Option<PageRange>,
}

/// Anything that can turn a file of some format into a [`Document`].
pub trait Extractor {
    /// Stable name, used in logs and error messages.
    fn name(&self) -> &'static str;

    /// Whether this extractor handles the given file.
    fn supports(&self, path: &Path, sniff: &Sniff) -> bool;

    /// Read and parse the file. The impure edge.
    fn extract(&self, path: &Path, opts: &ExtractOpts) -> Result<Document>;
}

/// Pick the extractor for a format, if one exists yet.
///
/// Formats without an implementation return `None`; the caller turns that into
/// [`Error::NotImplemented`]. Phases 2–5 fill this table in.
pub fn for_format(format: Format) -> Option<Box<dyn Extractor>> {
    match format {
        Format::Txt => Some(Box::new(text::TextExtractor)),
        Format::Markdown => Some(Box::new(markdown::MarkdownExtractor)),
        Format::Csv => Some(Box::new(csv::CsvExtractor)),
        Format::Html => Some(Box::new(html::HtmlExtractor)),
        Format::Rtf => Some(Box::new(rtf::RtfExtractor)),
        Format::Docx => Some(Box::new(ooxml::docx::DocxExtractor)),
        Format::Pptx => Some(Box::new(ooxml::pptx::PptxExtractor)),
        Format::Odt | Format::Odp => Some(Box::new(odf::OdfExtractor)),
        Format::Xlsx | Format::Ods => Some(Box::new(spreadsheet::SpreadsheetExtractor)),
        Format::Epub => Some(Box::new(epub::EpubExtractor)),
        Format::Pdf => Some(Box::new(pdf::PdfExtractor)),
    }
}

/// Detect the format of `path` and extract it.
pub fn extract_path(path: &Path, opts: &ExtractOpts) -> Result<Document> {
    let format = crate::detect::detect_path(path)?;
    let extractor = for_format(format).ok_or(Error::NotImplemented { format })?;

    let doc = extractor.extract(path, opts)?;
    if doc.is_empty() {
        return Err(Error::NoText {
            path: path.to_path_buf(),
        });
    }
    Ok(doc)
}

/// A `--pages` selection: `1-10`, `3`, `2-`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRange {
    /// First page, 1-based and inclusive.
    pub start: u32,
    /// Last page, inclusive. `None` means "to the end".
    pub end: Option<u32>,
}

impl PageRange {
    /// Parse a range as accepted by `--pages`.
    pub fn parse(input: &str) -> Result<PageRange> {
        let invalid = |message: &str| Error::InvalidPageRange {
            input: input.to_string(),
            message: message.to_string(),
        };
        let parse_page = |value: &str| {
            value
                .parse::<u32>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| invalid("page numbers start at 1"))
        };

        let trimmed = input.trim();
        let range = match trimmed.split_once('-') {
            None => {
                let page = parse_page(trimmed)?;
                PageRange {
                    start: page,
                    end: Some(page),
                }
            }
            Some((start, "")) => PageRange {
                start: parse_page(start)?,
                end: None,
            },
            Some(("", _)) => return Err(invalid("missing first page")),
            Some((start, end)) => PageRange {
                start: parse_page(start)?,
                end: Some(parse_page(end)?),
            },
        };

        if let Some(end) = range.end
            && end < range.start
        {
            return Err(invalid("last page is before the first"));
        }
        Ok(range)
    }

    /// Whether a 1-based page number falls inside the range.
    pub fn contains(&self, page: u32) -> bool {
        page >= self.start && self.end.is_none_or(|end| page <= end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page_ranges() {
        assert_eq!(
            PageRange::parse("1-10").unwrap(),
            PageRange {
                start: 1,
                end: Some(10)
            }
        );
        assert_eq!(
            PageRange::parse("3").unwrap(),
            PageRange {
                start: 3,
                end: Some(3)
            }
        );
        assert_eq!(
            PageRange::parse("2-").unwrap(),
            PageRange {
                start: 2,
                end: None
            }
        );
    }

    #[test]
    fn rejects_bad_page_ranges() {
        for input in ["0", "-5", "10-2", "abc", ""] {
            assert!(
                PageRange::parse(input).is_err(),
                "{input:?} should not parse"
            );
        }
    }

    #[test]
    fn range_membership() {
        let range = PageRange::parse("2-4").unwrap();
        assert!(!range.contains(1));
        assert!(range.contains(2) && range.contains(4));
        assert!(!range.contains(5));

        let open = PageRange::parse("2-").unwrap();
        assert!(open.contains(9_999));
    }

    #[test]
    fn every_known_format_has_an_extractor() {
        for format in Format::ALL {
            assert!(for_format(format).is_some(), "{format} has no extractor");
        }
    }
}
