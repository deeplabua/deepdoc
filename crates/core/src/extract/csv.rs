//! CSV extractor — one table, the first record as its header.

use std::path::Path;

use crate::detect::{Format, Sniff};
use crate::error::{Error, Result};
use crate::extract::{ExtractOpts, Extractor};
use crate::model::{Block, Document, Metadata, Row};

pub struct CsvExtractor;

impl Extractor for CsvExtractor {
    fn name(&self) -> &'static str {
        "csv"
    }

    fn supports(&self, _path: &Path, sniff: &Sniff) -> bool {
        crate::detect::detect(sniff) == Some(Format::Csv)
    }

    fn extract(&self, path: &Path, _opts: &ExtractOpts) -> Result<Document> {
        let raw = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        let blocks = parse(&raw).map_err(|message| Error::parse(path, message))?;

        Ok(Document {
            meta: Metadata {
                source_format: Some(Format::Csv),
                source_path: Some(path.display().to_string()),
                ..Metadata::default()
            },
            blocks,
        })
    }
}

/// Parse CSV bytes into a single table block. Pure.
pub fn parse(raw: &[u8]) -> std::result::Result<Vec<Block>, String> {
    let mut reader = csv::ReaderBuilder::new()
        // Ragged rows are common in exported data; keep them rather than fail.
        .flexible(true)
        // Headers are handled here, not by the reader, so the first record is
        // available like any other.
        .has_headers(false)
        .from_reader(raw);

    let mut records = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| e.to_string())?;
        records.push(Row::from_texts(record.iter()));
    }

    if records.is_empty() {
        return Ok(Vec::new());
    }

    let header = records.remove(0);
    Ok(vec![Block::Table {
        header: Some(header),
        rows: records,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_record_becomes_the_header() {
        let blocks = parse(b"name,qty\nbolt,4\nnut,8\n").unwrap();
        assert_eq!(
            blocks,
            vec![Block::Table {
                header: Some(Row::from_texts(["name", "qty"])),
                rows: vec![
                    Row::from_texts(["bolt", "4"]),
                    Row::from_texts(["nut", "8"])
                ],
            }]
        );
    }

    #[test]
    fn quoted_fields_keep_their_commas_and_newlines() {
        let blocks = parse(b"a,b\n\"x,y\",\"line1\nline2\"\n").unwrap();
        let Block::Table { rows, .. } = &blocks[0] else {
            panic!("expected a table");
        };
        assert_eq!(rows[0], Row::from_texts(["x,y", "line1\nline2"]));
    }

    #[test]
    fn ragged_rows_are_kept() {
        let blocks = parse(b"a,b,c\n1,2\n").unwrap();
        let Block::Table { rows, .. } = &blocks[0] else {
            panic!("expected a table");
        };
        assert_eq!(rows[0], Row::from_texts(["1", "2"]));
    }

    #[test]
    fn empty_input_yields_no_blocks() {
        assert!(parse(b"").unwrap().is_empty());
    }
}
