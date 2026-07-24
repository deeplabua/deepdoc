//! ZIP container access — OOXML, ODF and EPUB are all ZIP archives of XML.

use std::io::{Cursor, Read};
use std::path::Path;

use crate::error::{Error, Result};

/// A ZIP archive read fully into memory.
///
/// Office documents are small relative to the machines that read them, and the
/// extractors jump between entries (`document.xml`, then its relationships,
/// then the numbering table), so streaming would buy nothing.
pub struct Container {
    archive: zip::ZipArchive<Cursor<Vec<u8>>>,
}

impl Container {
    pub fn open(path: &Path) -> Result<Container> {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        Container::from_bytes(&bytes).map_err(|message| Error::parse(path, message))
    }

    pub fn from_bytes(bytes: &[u8]) -> std::result::Result<Container, String> {
        let archive =
            zip::ZipArchive::new(Cursor::new(bytes.to_vec())).map_err(|e| e.to_string())?;
        Ok(Container { archive })
    }

    /// Read an entry as text. Entries are UTF-8 in every format we support;
    /// anything else is decoded lossily rather than failing the document.
    pub fn read(&mut self, name: &str) -> std::result::Result<String, String> {
        let mut entry = self
            .archive
            .by_name(name)
            .map_err(|_| format!("{name} is missing"))?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| format!("cannot read {name}: {e}"))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Read an entry, or `None` when it does not exist.
    pub fn read_optional(&mut self, name: &str) -> Option<String> {
        self.read(name).ok()
    }

    /// Every entry name, in the archive's own order.
    pub fn names(&self) -> Vec<String> {
        self.archive.file_names().map(str::to_string).collect()
    }

    /// Entry names under a prefix, sorted so numbered parts line up naturally
    /// (`slide2.xml` before `slide10.xml`).
    pub fn names_under(&self, prefix: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .names()
            .into_iter()
            .filter(|name| name.starts_with(prefix))
            .collect();
        names.sort_by_key(|name| natural_key(name));
        names
    }
}

/// Split a name into text and number parts so numbers compare numerically.
fn natural_key(name: &str) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut chars = name.chars().peekable();

    while let Some(&ch) = chars.peek() {
        if ch.is_ascii_digit() {
            let mut digits = String::new();
            while chars.peek().is_some_and(char::is_ascii_digit) {
                digits.push(chars.next().expect("peeked"));
            }
            keys.push(Key::Number(digits.parse().unwrap_or(u64::MAX)));
        } else {
            let mut text = String::new();
            while chars.peek().is_some_and(|c| !c.is_ascii_digit()) {
                text.push(chars.next().expect("peeked"));
            }
            keys.push(Key::Text(text));
        }
    }

    keys
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Text(String),
    Number(u64),
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a stored (uncompressed) ZIP from name/content pairs.
    pub(crate) fn zip_bytes(entries: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            writer.start_file(*name, options).expect("start entry");
            writer.write_all(content.as_bytes()).expect("write entry");
        }
        writer.finish().expect("finish zip").into_inner()
    }

    #[test]
    fn reads_entries() {
        let bytes = zip_bytes(&[("word/document.xml", "<w:document/>")]);
        let mut container = Container::from_bytes(&bytes).unwrap();
        assert_eq!(
            container.read("word/document.xml").unwrap(),
            "<w:document/>"
        );
        assert!(container.read("nope.xml").is_err());
        assert!(container.read_optional("nope.xml").is_none());
    }

    #[test]
    fn names_under_sorts_numbers_naturally() {
        let bytes = zip_bytes(&[
            ("ppt/slides/slide10.xml", "a"),
            ("ppt/slides/slide2.xml", "b"),
            ("ppt/slides/slide1.xml", "c"),
            ("ppt/presentation.xml", "d"),
        ]);
        let container = Container::from_bytes(&bytes).unwrap();
        assert_eq!(
            container.names_under("ppt/slides/slide"),
            vec![
                "ppt/slides/slide1.xml",
                "ppt/slides/slide2.xml",
                "ppt/slides/slide10.xml",
            ]
        );
    }

    #[test]
    fn a_non_zip_is_an_error() {
        assert!(Container::from_bytes(b"not a zip").is_err());
    }
}
