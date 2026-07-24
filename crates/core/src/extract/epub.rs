//! EPUB extractor — the spine's XHTML documents, in reading order.
//!
//! An EPUB is a ZIP whose `META-INF/container.xml` points at a package
//! document (`.opf`); the package lists the manifest and the spine, which is
//! the order a reader turns the pages in. Each spine item is XHTML, so it goes
//! through the HTML extractor rather than a second HTML implementation.

use std::path::Path;

use crate::detect::{Format, Sniff};
use crate::error::{Error, Result};
use crate::extract::container::Container;
use crate::extract::xml::{self, Element};
use crate::extract::{ExtractOpts, Extractor, html};
use crate::model::{Block, Document, Inline, Metadata};

pub struct EpubExtractor;

impl Extractor for EpubExtractor {
    fn name(&self) -> &'static str {
        "epub"
    }

    fn supports(&self, _path: &Path, sniff: &Sniff) -> bool {
        crate::detect::detect(sniff) == Some(Format::Epub)
    }

    fn extract(&self, path: &Path, _opts: &ExtractOpts) -> Result<Document> {
        let mut container = Container::open(path)?;
        let parse_error = |message: String| Error::parse(path, message);

        let container_xml = container
            .read("META-INF/container.xml")
            .map_err(parse_error)?;
        let package_path = root_file(&container_xml).map_err(parse_error)?;

        let package_xml = container.read(&package_path).map_err(parse_error)?;
        let package = Package::parse(&package_xml).map_err(parse_error)?;

        let base = directory_of(&package_path);
        let mut blocks = Vec::new();
        for href in &package.spine {
            let entry = resolve(&base, href);
            let Some(source) = container.read_optional(&entry) else {
                // A spine item that is not in the archive is a broken book, not
                // a reason to lose the chapters that are there.
                continue;
            };
            blocks.extend(chapter(&source));
        }

        let mut meta = package.meta;
        meta.source_format = Some(Format::Epub);
        meta.source_path = Some(path.display().to_string());

        Ok(Document { meta, blocks })
    }
}

/// One chapter: its XHTML through the HTML extractor, with a heading ensured.
fn chapter(source: &str) -> Vec<Block> {
    let parsed = html::parse(source);
    let mut blocks = parsed.blocks;

    if blocks.is_empty() {
        return blocks;
    }
    // Chapters usually open with a heading; when one does not, its `<title>`
    // keeps the book navigable in the Markdown.
    if !matches!(blocks.first(), Some(Block::Heading { .. }))
        && let Some(title) = parsed.title.filter(|title| !title.trim().is_empty())
    {
        blocks.insert(
            0,
            Block::Heading {
                level: 1,
                text: Inline::text(title.trim()),
            },
        );
    }

    blocks
}

/// What the package document says: metadata plus the spine, in reading order.
#[derive(Debug, Default, PartialEq)]
pub struct Package {
    pub meta: Metadata,
    /// Chapter hrefs, relative to the package document.
    pub spine: Vec<String>,
}

impl Package {
    /// Parse an `.opf` package document. Pure.
    pub fn parse(source: &str) -> std::result::Result<Package, String> {
        let root = xml::parse(source)?;

        let meta = root
            .child("metadata")
            .map(package_metadata)
            .unwrap_or_default();

        // manifest: id → href
        let manifest = root.child("manifest");
        let items: Vec<(&str, &str)> = manifest
            .map(|manifest| {
                manifest
                    .find_all("item")
                    .into_iter()
                    .filter_map(|item| Some((item.attr("id")?, item.attr("href")?)))
                    .collect()
            })
            .unwrap_or_default();

        let spine = root
            .child("spine")
            .map(|spine| {
                spine
                    .find_all("itemref")
                    .into_iter()
                    .filter(|item| item.attr("linear") != Some("no"))
                    .filter_map(|item| {
                        let idref = item.attr("idref")?;
                        items
                            .iter()
                            .find(|(id, _)| *id == idref)
                            .map(|(_, href)| (*href).to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Package { meta, spine })
    }
}

fn package_metadata(metadata: &Element) -> Metadata {
    let text_of = |name: &str| {
        metadata
            .find(name)
            .map(|element| element.text().trim().to_string())
            .filter(|text| !text.is_empty())
    };

    Metadata {
        title: text_of("title"),
        author: text_of("creator"),
        created: text_of("date"),
        language: text_of("language"),
        publisher: text_of("publisher"),
        ..Metadata::default()
    }
}

/// The package document's path, from `META-INF/container.xml`.
fn root_file(container_xml: &str) -> std::result::Result<String, String> {
    let root = xml::parse(container_xml)?;
    root.find_all("rootfile")
        .into_iter()
        .find_map(|rootfile| rootfile.attr("full-path"))
        .map(str::to_string)
        .ok_or_else(|| "container.xml names no rootfile".to_string())
}

/// `OEBPS/content.opf` → `OEBPS/`.
fn directory_of(path: &str) -> String {
    match path.rfind('/') {
        Some(index) => path[..=index].to_string(),
        None => String::new(),
    }
}

/// Resolve a manifest href against the package's directory.
///
/// Hrefs are URLs: they carry percent-escapes and may end in a fragment, and
/// they may climb out of the package's directory with `../`.
fn resolve(base: &str, href: &str) -> String {
    let href = href.split(['#', '?']).next().unwrap_or(href);
    let joined = format!("{base}{}", percent_decode(href));

    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());

    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE: &str = r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
        <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
          <dc:title>A Short Book</dc:title>
          <dc:creator>Ada Lovelace</dc:creator>
          <dc:language>en-GB</dc:language>
          <dc:publisher>DeepLab Press</dc:publisher>
          <dc:date>2026-07-24</dc:date>
        </metadata>
        <manifest>
          <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml"/>
          <item id="c1" href="text/chapter1.xhtml" media-type="application/xhtml+xml"/>
          <item id="c2" href="text/chapter2.xhtml" media-type="application/xhtml+xml"/>
          <item id="css" href="style.css" media-type="text/css"/>
        </manifest>
        <spine>
          <itemref idref="nav" linear="no"/>
          <itemref idref="c2"/>
          <itemref idref="c1"/>
        </spine>
      </package>"#;

    #[test]
    fn reads_metadata_from_the_package() {
        let package = Package::parse(PACKAGE).unwrap();
        assert_eq!(package.meta.title.as_deref(), Some("A Short Book"));
        assert_eq!(package.meta.author.as_deref(), Some("Ada Lovelace"));
        assert_eq!(package.meta.language.as_deref(), Some("en-GB"));
        assert_eq!(package.meta.publisher.as_deref(), Some("DeepLab Press"));
        assert_eq!(package.meta.created.as_deref(), Some("2026-07-24"));
    }

    #[test]
    fn the_spine_sets_the_order_not_the_manifest() {
        let package = Package::parse(PACKAGE).unwrap();
        assert_eq!(
            package.spine,
            vec!["text/chapter2.xhtml", "text/chapter1.xhtml"],
            "non-linear items are skipped and the spine order wins"
        );
    }

    #[test]
    fn finds_the_package_document() {
        let container = r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
            <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
          </container>"#;
        assert_eq!(root_file(container).unwrap(), "OEBPS/content.opf");
        assert!(root_file("<container/>").is_err());
    }

    #[test]
    fn hrefs_resolve_against_the_package_directory() {
        assert_eq!(resolve("OEBPS/", "text/ch1.xhtml"), "OEBPS/text/ch1.xhtml");
        assert_eq!(resolve("OEBPS/", "ch1.xhtml#part2"), "OEBPS/ch1.xhtml");
        assert_eq!(resolve("OEBPS/text/", "../ch1.xhtml"), "OEBPS/ch1.xhtml");
        assert_eq!(
            resolve("OEBPS/", "my%20chapter.xhtml"),
            "OEBPS/my chapter.xhtml"
        );
        assert_eq!(resolve("", "ch1.xhtml"), "ch1.xhtml");
    }

    #[test]
    fn directory_of_a_package_path() {
        assert_eq!(directory_of("OEBPS/content.opf"), "OEBPS/");
        assert_eq!(directory_of("content.opf"), "");
    }

    #[test]
    fn a_chapter_without_a_heading_gets_one_from_its_title() {
        let blocks =
            chapter("<html><head><title>Chapter One</title></head><body><p>text</p></body></html>");
        assert_eq!(
            blocks,
            vec![
                Block::Heading {
                    level: 1,
                    text: Inline::text("Chapter One")
                },
                Block::paragraph("text"),
            ]
        );
    }

    #[test]
    fn a_chapter_that_has_a_heading_keeps_it() {
        let blocks = chapter(
            "<html><head><title>Chapter One</title></head><body><h1>The Beginning</h1><p>text</p></body></html>",
        );
        assert_eq!(
            blocks,
            vec![
                Block::Heading {
                    level: 1,
                    text: Inline::text("The Beginning")
                },
                Block::paragraph("text"),
            ]
        );
    }

    #[test]
    fn an_empty_chapter_adds_nothing() {
        assert!(chapter("<html><head><title>Blank</title></head><body></body></html>").is_empty());
    }

    #[test]
    fn a_package_without_a_spine_is_not_an_error() {
        let package = Package::parse(r#"<package xmlns="o"/>"#).unwrap();
        assert!(package.spine.is_empty());
    }
}
