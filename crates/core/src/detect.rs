//! File type detection: signature first, extension as a hint.
//!
//! Order (see `docs/PRD/03-tech-stack.md`):
//! 1. magic bytes — `%PDF`, `{\rtf`, `PK\x03\x04` (then peek inside the ZIP for
//!    `mimetype`, `word/`, `ppt/`, `xl/`);
//! 2. extension, which is what decides the text formats (txt/md/csv/…);
//! 3. a last-resort sniff for HTML in the head bytes.
//!
//! Detection is a pure function over [`Sniff`], so it is unit-testable without
//! touching the filesystem.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// How many bytes of the file head are read for sniffing.
pub const SNIFF_LEN: usize = 8 * 1024;

/// A document format DeepDoc knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Txt,
    Markdown,
    Csv,
    Html,
    Rtf,
    Docx,
    Pptx,
    Xlsx,
    Odt,
    Odp,
    Ods,
    Epub,
    Pdf,
}

impl Format {
    /// Every format DeepDoc knows about.
    pub const ALL: [Format; 13] = [
        Format::Txt,
        Format::Markdown,
        Format::Csv,
        Format::Html,
        Format::Rtf,
        Format::Docx,
        Format::Pptx,
        Format::Xlsx,
        Format::Odt,
        Format::Odp,
        Format::Ods,
        Format::Epub,
        Format::Pdf,
    ];

    /// Canonical lowercase name, as used in metadata and error messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Txt => "txt",
            Format::Markdown => "markdown",
            Format::Csv => "csv",
            Format::Html => "html",
            Format::Rtf => "rtf",
            Format::Docx => "docx",
            Format::Pptx => "pptx",
            Format::Xlsx => "xlsx",
            Format::Odt => "odt",
            Format::Odp => "odp",
            Format::Ods => "ods",
            Format::Epub => "epub",
            Format::Pdf => "pdf",
        }
    }

    /// Map a file extension (without the dot, any case) onto a format.
    pub fn from_extension(ext: &str) -> Option<Format> {
        let ext = ext.to_ascii_lowercase();
        Some(match ext.as_str() {
            "txt" | "text" | "log" => Format::Txt,
            "md" | "markdown" => Format::Markdown,
            "csv" => Format::Csv,
            "html" | "htm" | "xhtml" => Format::Html,
            "rtf" => Format::Rtf,
            "docx" => Format::Docx,
            "pptx" => Format::Pptx,
            "xlsx" => Format::Xlsx,
            "odt" => Format::Odt,
            "odp" => Format::Odp,
            "ods" => Format::Ods,
            "epub" => Format::Epub,
            "pdf" => Format::Pdf,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Everything detection needs about a file: its extension and its head bytes.
#[derive(Debug, Clone)]
pub struct Sniff {
    pub extension: Option<String>,
    pub head: Vec<u8>,
}

impl Sniff {
    /// Build a sniff from parts — used by tests and by callers that already hold the bytes.
    pub fn new(extension: Option<String>, head: Vec<u8>) -> Self {
        Self { extension, head }
    }

    /// Read the head of `path` for sniffing.
    pub fn read(path: &Path) -> Result<Sniff> {
        use std::io::Read;

        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());

        let mut file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
        let mut head = vec![0u8; SNIFF_LEN];
        let mut filled = 0;
        while filled < head.len() {
            match file.read(&mut head[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::io(path, e)),
            }
        }
        head.truncate(filled);

        Ok(Sniff { extension, head })
    }
}

/// Detect the format of a file on disk.
pub fn detect_path(path: &Path) -> Result<Format> {
    let sniff = Sniff::read(path)?;
    detect(&sniff).ok_or_else(|| Error::Unsupported {
        path: PathBuf::from(path),
    })
}

/// Detect a format from already-sniffed bytes. Pure.
pub fn detect(sniff: &Sniff) -> Option<Format> {
    if let Some(format) = detect_signature(&sniff.head) {
        return Some(format);
    }
    if let Some(format) = sniff.extension.as_deref().and_then(Format::from_extension) {
        return Some(format);
    }
    if looks_like_html(&sniff.head) {
        return Some(Format::Html);
    }
    None
}

/// Detect a format from magic bytes alone.
pub fn detect_signature(head: &[u8]) -> Option<Format> {
    if head.starts_with(b"%PDF-") {
        return Some(Format::Pdf);
    }
    if head.starts_with(b"{\\rtf") {
        return Some(Format::Rtf);
    }
    if head.starts_with(b"PK\x03\x04") {
        return detect_zip(head);
    }
    None
}

/// Classify a ZIP container by looking at what is stored inside it.
///
/// This reads the first local file header (ODF/EPUB put an uncompressed
/// `mimetype` entry there) and otherwise scans the head bytes for the marker
/// directories OOXML uses. It is a heuristic on the first [`SNIFF_LEN`] bytes;
/// full central-directory reading arrives with the real ZIP extractors (Phase 3).
fn detect_zip(head: &[u8]) -> Option<Format> {
    if let Some(mimetype) = first_entry_mimetype(head) {
        let format = match mimetype.as_str() {
            "application/epub+zip" => Format::Epub,
            "application/vnd.oasis.opendocument.text" => Format::Odt,
            "application/vnd.oasis.opendocument.presentation" => Format::Odp,
            "application/vnd.oasis.opendocument.spreadsheet" => Format::Ods,
            _ => return None,
        };
        return Some(format);
    }

    // OOXML: the part names themselves identify the flavour.
    if contains(head, b"word/") {
        return Some(Format::Docx);
    }
    if contains(head, b"ppt/") {
        return Some(Format::Pptx);
    }
    if contains(head, b"xl/") {
        return Some(Format::Xlsx);
    }
    None
}

/// If the ZIP's first entry is named `mimetype`, return its (stored) content.
fn first_entry_mimetype(head: &[u8]) -> Option<String> {
    // Local file header: signature(4) .. name_len(2) @26, extra_len(2) @28, name @30.
    const NAME_LEN_OFFSET: usize = 26;
    const NAME_OFFSET: usize = 30;

    let name_len =
        u16::from_le_bytes([*head.get(NAME_LEN_OFFSET)?, *head.get(NAME_LEN_OFFSET + 1)?]) as usize;
    let extra_len = u16::from_le_bytes([
        *head.get(NAME_LEN_OFFSET + 2)?,
        *head.get(NAME_LEN_OFFSET + 3)?,
    ]) as usize;
    let name = head.get(NAME_OFFSET..NAME_OFFSET + name_len)?;
    if name != b"mimetype" {
        return None;
    }

    // ODF/EPUB store `mimetype` uncompressed and first, so its bytes follow the header.
    let start = NAME_OFFSET + name_len + extra_len;
    let value = head.get(start..)?;
    let end = value
        .iter()
        .position(
            |b| !matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'+' | b'-' | b'/'),
        )
        .unwrap_or(value.len());
    let mimetype = std::str::from_utf8(&value[..end]).ok()?;
    (!mimetype.is_empty()).then(|| mimetype.to_string())
}

/// Last-resort HTML sniff for extensionless files.
fn looks_like_html(head: &[u8]) -> bool {
    let probe: Vec<u8> = head
        .iter()
        .take(512)
        .map(|b| b.to_ascii_lowercase())
        .collect();
    contains(&probe, b"<!doctype html") || contains(&probe, b"<html")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sniff(ext: &str, head: &[u8]) -> Sniff {
        Sniff::new(Some(ext.to_string()), head.to_vec())
    }

    /// Build a minimal ZIP local file header for `name`, followed by `payload`.
    fn zip_entry(name: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PK\x03\x04");
        bytes.extend_from_slice(&[0u8; 22]); // version..uncompressed size
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn extension_maps_to_format() {
        assert_eq!(Format::from_extension("MD"), Some(Format::Markdown));
        assert_eq!(Format::from_extension("htm"), Some(Format::Html));
        assert_eq!(Format::from_extension("xlsx"), Some(Format::Xlsx));
        assert_eq!(Format::from_extension("exe"), None);
    }

    #[test]
    fn signature_beats_extension() {
        // A PDF that someone named `.txt` is still a PDF.
        let detected = detect(&sniff("txt", b"%PDF-1.7\n%\xe2\xe3\xcf\xd3"));
        assert_eq!(detected, Some(Format::Pdf));
    }

    #[test]
    fn text_formats_come_from_the_extension() {
        assert_eq!(detect(&sniff("txt", b"hello")), Some(Format::Txt));
        assert_eq!(detect(&sniff("csv", b"a,b\n1,2\n")), Some(Format::Csv));
        assert_eq!(detect(&sniff("md", b"# Title\n")), Some(Format::Markdown));
    }

    #[test]
    fn rtf_is_detected_by_signature() {
        assert_eq!(detect(&sniff("bin", b"{\\rtf1\\ansi")), Some(Format::Rtf));
    }

    #[test]
    fn ooxml_zip_is_classified_by_part_names() {
        let docx = zip_entry(b"[Content_Types].xml", b"<Types/>word/document.xml");
        assert_eq!(detect(&sniff("bin", &docx)), Some(Format::Docx));

        let pptx = zip_entry(b"[Content_Types].xml", b"<Types/>ppt/presentation.xml");
        assert_eq!(detect(&sniff("bin", &pptx)), Some(Format::Pptx));

        let xlsx = zip_entry(b"[Content_Types].xml", b"<Types/>xl/workbook.xml");
        assert_eq!(detect(&sniff("bin", &xlsx)), Some(Format::Xlsx));
    }

    #[test]
    fn odf_and_epub_are_classified_by_mimetype_entry() {
        let epub = zip_entry(b"mimetype", b"application/epub+zip");
        assert_eq!(detect(&sniff("bin", &epub)), Some(Format::Epub));

        let odt = zip_entry(b"mimetype", b"application/vnd.oasis.opendocument.text");
        assert_eq!(detect(&sniff("bin", &odt)), Some(Format::Odt));

        let ods = zip_entry(
            b"mimetype",
            b"application/vnd.oasis.opendocument.spreadsheet",
        );
        assert_eq!(detect(&sniff("bin", &ods)), Some(Format::Ods));
    }

    #[test]
    fn html_is_sniffed_without_an_extension() {
        let sniffed = Sniff::new(None, b"<!DOCTYPE HTML><html><body>hi".to_vec());
        assert_eq!(detect(&sniffed), Some(Format::Html));
    }

    #[test]
    fn unknown_type_is_not_detected() {
        assert_eq!(detect(&sniff("exe", b"MZ\x90\x00")), None);
        assert_eq!(detect(&Sniff::new(None, b"\x00\x01\x02".to_vec())), None);
    }
}
