//! End-to-end checks over the tiny fixture files: file → Markdown.
//!
//! These are the tests that catch a regression anywhere along the pipeline —
//! detection, extraction and rendering all have to agree.

use std::path::PathBuf;

use deepdoc_core::extract::{ExtractOpts, extract_path};
use deepdoc_core::render::{RenderOpts, to_markdown, to_text};
use deepdoc_core::{Document, Format};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn extract(name: &str) -> Document {
    extract_path(&fixture(name), &ExtractOpts::default())
        .unwrap_or_else(|e| panic!("cannot extract {name}: {e}"))
}

fn markdown(name: &str) -> String {
    to_markdown(&extract(name), &RenderOpts::default())
}

#[test]
fn text_fixture() {
    let doc = extract("sample.txt");
    assert_eq!(doc.meta.source_format, Some(Format::Txt));
    assert_eq!(
        to_markdown(&doc, &RenderOpts::default()),
        "DeepDoc plain text fixture.\n\n\
         A second paragraph, wrapped\nacross two source lines.\n"
    );
}

#[test]
fn markdown_fixture_round_trips() {
    let source = std::fs::read_to_string(fixture("sample.md")).unwrap();
    assert_eq!(markdown("sample.md"), source);
}

#[test]
fn markdown_fixture_takes_its_title_from_the_first_heading() {
    assert_eq!(
        extract("sample.md").meta.title.as_deref(),
        Some("Quarterly Report")
    );
}

#[test]
fn csv_fixture_becomes_a_table() {
    assert_eq!(
        markdown("sample.csv"),
        "| part | qty | note     |\n\
         | ---- | --- | -------- |\n\
         | bolt | 4   | M6, zinc |\n\
         | nut  | 8   | plain    |\n"
    );
}

#[test]
fn html_fixture_becomes_clean_markdown() {
    assert_eq!(
        markdown("sample.html"),
        "# Quarterly Report\n\n\
         Revenue grew **12%**, driven by *cloud* and a [partner deal](https://example.com).\n\n\
         - Cloud\n\
         - Devices\n\
         \x20 1. Phones\n\
         \x20 2. Tablets\n\n\
         | Segment | Q1  | Q2  |\n\
         | ------- | --- | --- |\n\
         | Cloud   | 4.1 | 4.7 |\n\
         | Devices | 1.2 | 1.1 |\n\n\
         ```rust\n\
         fn main() {}\n\
         ```\n\n\
         ![Revenue by segment]()\n"
    );
}

#[test]
fn html_fixture_carries_title_and_author() {
    let doc = extract("sample.html");
    assert_eq!(doc.meta.title.as_deref(), Some("Quarterly Report"));
    assert_eq!(doc.meta.author.as_deref(), Some("Ada Lovelace"));
}

#[test]
fn html_fixture_drops_navigation_and_scripts() {
    let text = to_text(&extract("sample.html"));
    for junk in ["Home", "Docs", "console.log", "DeepLab", "font-family"] {
        assert!(!text.contains(junk), "{junk:?} should not survive:\n{text}");
    }
}

#[test]
fn rtf_fixture_keeps_paragraphs_and_styling() {
    assert_eq!(
        markdown("sample.rtf"),
        "Quarterly Report\n\n\
         Revenue grew **12%**, driven by *cloud* in café markets.\n\n\
         Cash flow stayed positive — barely.\n"
    );
}

#[test]
fn every_fixture_round_trips_through_json() {
    for name in [
        "sample.txt",
        "sample.md",
        "sample.csv",
        "sample.html",
        "sample.rtf",
    ] {
        let doc = extract(name);
        let json = serde_json::to_string(&doc).expect("serializes");
        let back: Document = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(doc, back, "{name} did not survive a JSON round trip");
    }
}
