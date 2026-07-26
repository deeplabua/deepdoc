//! End-to-end checks for PDF: real files, built here, through the whole
//! pipeline — parsing, glyph placement, layout and rendering.
//!
//! The fixtures are written as PDF source rather than committed as binaries:
//! the content streams are the interesting part, and a `.pdf` blob in the
//! repository would be unreviewable.

use std::path::{Path, PathBuf};

use deepdoc_core::extract::{ExtractOpts, PageRange, extract_path};
use deepdoc_core::render::{RenderOpts, to_markdown};
use deepdoc_core::{Document, Format};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let path = std::env::temp_dir().join(format!("deepdoc-pdf-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("cannot create temp dir");
        TempDir(path)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).expect("cannot write pdf");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One line of text to draw: size, position from the bottom-left, and content.
struct Text {
    size: f64,
    x: f64,
    y: f64,
    content: &'static str,
}

const fn text(size: f64, x: f64, y: f64, content: &'static str) -> Text {
    Text {
        size,
        x,
        y,
        content,
    }
}

/// Build a PDF with one page per group of text lines.
///
/// Written by hand — a PDF is a list of objects and a cross-reference table of
/// their byte offsets, which is exactly what a test can produce without a
/// writing library.
fn pdf(pages: &[&[Text]], info: Option<(&str, &str)>) -> Vec<u8> {
    let mut objects: Vec<Vec<u8>> = Vec::new();

    // 1: catalogue, 2: page tree, then a page and a content stream per page.
    let page_ids: Vec<usize> = (0..pages.len()).map(|index| 3 + index * 2).collect();
    let kids: String = page_ids
        .iter()
        .map(|id| format!("{id} 0 R"))
        .collect::<Vec<_>>()
        .join(" ");

    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).into_bytes());

    let font_id = 3 + pages.len() * 2;
    for (index, lines) in pages.iter().enumerate() {
        let content_id = page_ids[index] + 1;
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 {font_id} 0 R >> >> /Contents {content_id} 0 R >>"
            )
            .into_bytes(),
        );

        let mut stream = String::new();
        for line in lines.iter() {
            stream.push_str(&format!(
                "BT /F1 {} Tf {} {} Td ({}) Tj ET\n",
                line.size,
                line.x,
                line.y,
                escape(line.content)
            ));
        }
        let mut object = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
        object.extend_from_slice(stream.as_bytes());
        object.extend_from_slice(b"endstream");
        objects.push(object);
    }

    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());

    let info_id = objects.len() + 1;
    if let Some((title, author)) = info {
        objects.push(
            format!(
                "<< /Title ({}) /Author ({}) /CreationDate (D:20260724103000+02'00') >>",
                escape(title),
                escape(author)
            )
            .into_bytes(),
        );
    }

    // Serialise, recording where each object starts for the xref table.
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        out.extend_from_slice(object);
        out.extend_from_slice(b"\nendobj\n");
    }

    let xref_offset = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }

    let info_entry = if info.is_some() {
        format!(" /Info {info_id} 0 R")
    } else {
        String::new()
    };
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R{info_entry} >>\nstartxref\n{xref_offset}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );

    out
}

/// Parentheses and backslashes are the string delimiters in a PDF.
fn escape(text: &str) -> String {
    text.replace('\\', r"\\")
        .replace('(', r"\(")
        .replace(')', r"\)")
}

fn extract(path: &Path) -> Document {
    extract_path(path, &ExtractOpts::default())
        .unwrap_or_else(|e| panic!("cannot extract {}: {e}", path.display()))
}

fn markdown(path: &Path) -> String {
    to_markdown(&extract(path), &RenderOpts::default())
}

/// A page of body text under a title, with a second paragraph.
const REPORT: &[Text] = &[
    text(24.0, 72.0, 720.0, "Quarterly Report"),
    text(
        10.0,
        72.0,
        680.0,
        "Revenue grew twelve per cent quarter over",
    ),
    text(
        10.0,
        72.0,
        668.0,
        "quarter, driven by cloud and a partner deal.",
    ),
    text(10.0, 72.0, 620.0, "Cash flow stayed positive throughout."),
];

#[test]
fn a_born_digital_pdf_becomes_markdown() {
    let dir = TempDir::new("report");
    let path = dir.write("report.pdf", &pdf(&[REPORT], None));

    assert_eq!(extract(&path).meta.source_format, Some(Format::Pdf));
    assert_eq!(
        markdown(&path),
        "# Quarterly Report\n\n\
         Revenue grew twelve per cent quarter over quarter, driven by cloud and a partner deal.\n\n\
         Cash flow stayed positive throughout.\n"
    );
}

#[test]
fn the_information_dictionary_becomes_metadata() {
    let dir = TempDir::new("info");
    let path = dir.write(
        "report.pdf",
        &pdf(&[REPORT], Some(("Quarterly Report", "Ada Lovelace"))),
    );

    let doc = extract(&path);
    assert_eq!(doc.meta.title.as_deref(), Some("Quarterly Report"));
    assert_eq!(doc.meta.author.as_deref(), Some("Ada Lovelace"));
    assert_eq!(doc.meta.created.as_deref(), Some("2026-07-24"));
    assert_eq!(doc.meta.page_count, Some(1));
}

#[test]
fn pages_are_marked_and_can_be_selected() {
    let dir = TempDir::new("pages");
    let page_two: &[Text] = &[text(10.0, 72.0, 720.0, "The second page.")];
    let page_three: &[Text] = &[text(10.0, 72.0, 720.0, "The third page.")];
    let path = dir.write("book.pdf", &pdf(&[REPORT, page_two, page_three], None));

    let all = markdown(&path);
    assert!(all.contains("<!-- page 2 -->"), "{all}");
    assert!(all.contains("The third page."), "{all}");

    // `--pages 2-3` keeps the marker numbering of the original document.
    let doc = extract_path(
        &path,
        &ExtractOpts {
            pages: Some(PageRange::parse("2-3").unwrap()),
        },
    )
    .expect("extracts");
    let rendered = to_markdown(&doc, &RenderOpts::default());

    assert!(!rendered.contains("Quarterly Report"), "{rendered}");
    assert!(rendered.contains("The second page."), "{rendered}");
    assert!(rendered.contains("<!-- page 3 -->"), "{rendered}");
}

#[test]
fn a_page_without_text_asks_for_ocr() {
    let dir = TempDir::new("scan");
    // A page with no text objects is what a scan looks like to a text extractor.
    let path = dir.write("scan.pdf", &pdf(&[&[]], None));

    let error = extract_path(&path, &ExtractOpts::default()).expect_err("should fail");
    assert_eq!(error.exit_code(), 4, "no text means the scan exit code");
    assert!(
        error.to_string().contains("deepocr"),
        "the message should point at OCR: {error}"
    );
}

#[test]
fn a_broken_pdf_is_an_error_not_a_panic() {
    let dir = TempDir::new("broken");
    let path = dir.write("broken.pdf", b"%PDF-1.4\nnot really a pdf\n%%EOF\n");

    let error = extract_path(&path, &ExtractOpts::default()).expect_err("should fail");
    assert_eq!(error.exit_code(), 1);
}

#[test]
fn two_columns_are_read_one_after_the_other() {
    let dir = TempDir::new("columns");

    let mut lines: Vec<Text> = Vec::new();
    let left = [
        "The left column opens the page",
        "and carries on for several lines",
        "before the reader reaches its",
        "final line at the bottom.",
    ];
    let right = [
        "The right column continues the",
        "same article at the top of the",
        "page, in a second block of text",
        "that ends here.",
    ];
    for (index, content) in left.iter().enumerate() {
        lines.push(text(10.0, 72.0, 700.0 - index as f64 * 12.0, content));
    }
    for (index, content) in right.iter().enumerate() {
        lines.push(text(10.0, 330.0, 700.0 - index as f64 * 12.0, content));
    }

    let path = dir.write("columns.pdf", &pdf(&[&lines], None));
    let rendered = markdown(&path);

    let left_position = rendered.find("final line").expect("left column present");
    let right_position = rendered
        .find("The right column")
        .expect("right column present");
    assert!(
        left_position < right_position,
        "the left column should be read first:\n{rendered}"
    );
}
