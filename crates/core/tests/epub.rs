//! End-to-end checks for EPUB: archive → detection → chapters → Markdown.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use deepdoc_core::extract::{ExtractOpts, extract_path};
use deepdoc_core::render::{RenderOpts, to_markdown};
use deepdoc_core::{Document, Format};
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let path = std::env::temp_dir().join(format!("deepdoc-epub-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("cannot create temp dir");
        TempDir(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write an EPUB. `mimetype` goes first and uncompressed, as the format
/// requires — and as the sniffer relies on to recognise the file.
fn write_epub(dir: &TempDir, name: &str, entries: &[(&str, &str)]) -> PathBuf {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));

    writer
        .start_file(
            "mimetype",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .expect("start mimetype");
    writer
        .write_all(b"application/epub+zip")
        .expect("write mimetype");

    for (entry, content) in entries {
        writer
            .start_file(
                *entry,
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .expect("start entry");
        writer.write_all(content.as_bytes()).expect("write entry");
    }

    let path = dir.0.join(name);
    std::fs::write(&path, writer.finish().expect("finish zip").into_inner()).expect("write epub");
    path
}

const CONTAINER: &str = r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
    <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
  </container>"#;

const PACKAGE: &str = r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
    <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
      <dc:title>A Short Book</dc:title>
      <dc:creator>Ada Lovelace</dc:creator>
      <dc:language>en-GB</dc:language>
      <dc:publisher>DeepLab Press</dc:publisher>
      <dc:date>2026-07-24</dc:date>
    </metadata>
    <manifest>
      <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
      <item id="c1" href="text/chapter1.xhtml" media-type="application/xhtml+xml"/>
      <item id="c2" href="text/chapter2.xhtml" media-type="application/xhtml+xml"/>
      <item id="css" href="style.css" media-type="text/css"/>
    </manifest>
    <spine>
      <itemref idref="nav" linear="no"/>
      <itemref idref="c1"/>
      <itemref idref="c2"/>
    </spine>
  </package>"#;

const CHAPTER_ONE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
  <html xmlns="http://www.w3.org/1999/xhtml"><head><title>Chapter One</title></head>
    <body>
      <h1>The Beginning</h1>
      <p>It was a <em>dark</em> and <strong>stormy</strong> night.</p>
      <ul><li>rain</li><li>wind</li></ul>
    </body>
  </html>"#;

/// No heading of its own — the chapter title has to come from `<title>`.
const CHAPTER_TWO: &str = r#"<?xml version="1.0" encoding="utf-8"?>
  <html xmlns="http://www.w3.org/1999/xhtml"><head><title>Chapter Two</title></head>
    <body><p>The morning after.</p></body>
  </html>"#;

fn book(dir: &TempDir) -> PathBuf {
    write_epub(
        dir,
        "book.epub",
        &[
            ("META-INF/container.xml", CONTAINER),
            ("OEBPS/content.opf", PACKAGE),
            (
                "OEBPS/nav.xhtml",
                "<html><body><nav><ol></ol></nav></body></html>",
            ),
            ("OEBPS/text/chapter1.xhtml", CHAPTER_ONE),
            ("OEBPS/text/chapter2.xhtml", CHAPTER_TWO),
            ("OEBPS/style.css", "body { margin: 0; }"),
        ],
    )
}

fn extract(path: &Path) -> Document {
    extract_path(path, &ExtractOpts::default())
        .unwrap_or_else(|e| panic!("cannot extract {}: {e}", path.display()))
}

#[test]
fn epub_becomes_markdown_chapter_by_chapter() {
    let dir = TempDir::new("book");
    let path = book(&dir);

    assert_eq!(extract(&path).meta.source_format, Some(Format::Epub));
    assert_eq!(
        to_markdown(&extract(&path), &RenderOpts::default()),
        "# The Beginning\n\n\
         It was a *dark* and **stormy** night.\n\n\
         - rain\n\
         - wind\n\n\
         # Chapter Two\n\n\
         The morning after.\n"
    );
}

#[test]
fn epub_metadata_reaches_the_front_matter() {
    let dir = TempDir::new("meta");
    let doc = extract(&book(&dir));

    assert_eq!(doc.meta.title.as_deref(), Some("A Short Book"));
    assert_eq!(doc.meta.author.as_deref(), Some("Ada Lovelace"));
    assert_eq!(doc.meta.language.as_deref(), Some("en-GB"));
    assert_eq!(doc.meta.publisher.as_deref(), Some("DeepLab Press"));
    assert_eq!(doc.meta.created.as_deref(), Some("2026-07-24"));

    let rendered = to_markdown(&doc, &RenderOpts { metadata: true });
    assert!(
        rendered.starts_with(
            "---\n\
             title: \"A Short Book\"\n\
             author: \"Ada Lovelace\"\n\
             created: \"2026-07-24\"\n\
             language: \"en-GB\"\n\
             publisher: \"DeepLab Press\"\n\
             format: \"epub\"\n\
             ---\n"
        ),
        "unexpected front matter:\n{rendered}"
    );
}

#[test]
fn json_output_carries_the_same_metadata() {
    let dir = TempDir::new("json");
    let value = deepdoc_core::to_json(&extract(&book(&dir)));

    assert_eq!(value["meta"]["title"], "A Short Book");
    assert_eq!(value["meta"]["language"], "en-GB");
    assert_eq!(value["meta"]["source_format"], "epub");
    assert!(value["blocks"].as_array().is_some_and(|b| !b.is_empty()));
}

#[test]
fn a_missing_chapter_does_not_lose_the_rest_of_the_book() {
    let dir = TempDir::new("broken");
    let path = write_epub(
        &dir,
        "broken.epub",
        &[
            ("META-INF/container.xml", CONTAINER),
            ("OEBPS/content.opf", PACKAGE),
            // chapter2 is listed in the spine but absent from the archive.
            ("OEBPS/text/chapter1.xhtml", CHAPTER_ONE),
        ],
    );

    let rendered = to_markdown(&extract(&path), &RenderOpts::default());
    assert!(rendered.contains("The Beginning"), "{rendered}");
    assert!(!rendered.contains("Chapter Two"), "{rendered}");
}

#[test]
fn an_epub_without_a_package_document_is_an_error_not_a_panic() {
    let dir = TempDir::new("nopackage");
    let path = write_epub(&dir, "empty.epub", &[("META-INF/container.xml", CONTAINER)]);

    let error = extract_path(&path, &ExtractOpts::default()).expect_err("should fail");
    assert_eq!(error.exit_code(), 1, "a broken container is a read failure");
}
