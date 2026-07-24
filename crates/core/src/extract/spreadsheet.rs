//! Spreadsheets (`.xlsx`, `.ods`) via calamine — one heading and one table per
//! sheet.
//!
//! Dates are rendered from the Excel serial number here rather than through
//! calamine's `dates` feature: that feature pulls chrono, and chrono's default
//! build reaches for the platform time-zone database — a C dependency in a
//! binary that promises none. The conversion itself is a few lines of integer
//! maths, and a spreadsheet's dates are worth showing as dates.

use std::io::Cursor;
use std::path::Path;

use calamine::{Data, Reader};

use crate::detect::{Format, Sniff};
use crate::error::{Error, Result};
use crate::extract::container::Container;
use crate::extract::{ExtractOpts, Extractor};
use crate::model::{Block, Document, Inline, Row};

pub struct SpreadsheetExtractor;

impl Extractor for SpreadsheetExtractor {
    fn name(&self) -> &'static str {
        "spreadsheet"
    }

    fn supports(&self, _path: &Path, sniff: &Sniff) -> bool {
        matches!(
            crate::detect::detect(sniff),
            Some(Format::Xlsx | Format::Ods)
        )
    }

    fn extract(&self, path: &Path, _opts: &ExtractOpts) -> Result<Document> {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        let blocks = parse(&bytes).map_err(|message| Error::parse(path, message))?;

        let format = crate::detect::detect_path(path).ok();
        let mut meta = match format {
            // Both containers carry metadata, in their own format's part.
            Some(Format::Ods) => Container::from_bytes(&bytes)
                .ok()
                .and_then(|mut container| container.read_optional("meta.xml"))
                .map(|source| crate::extract::odf::metadata(Some(&source)))
                .unwrap_or_default(),
            _ => Container::from_bytes(&bytes)
                .map(|mut container| crate::extract::ooxml::core_properties(&mut container))
                .unwrap_or_default(),
        };
        meta.source_format = format;
        meta.source_path = Some(path.display().to_string());

        Ok(Document { meta, blocks })
    }
}

/// Parse a workbook into one heading plus one table per sheet.
pub fn parse(bytes: &[u8]) -> std::result::Result<Vec<Block>, String> {
    let mut workbook = calamine::open_workbook_auto_from_rs(Cursor::new(bytes.to_vec()))
        .map_err(|e| e.to_string())?;

    let mut blocks = Vec::new();
    for name in workbook.sheet_names() {
        let Ok(range) = workbook.worksheet_range(&name) else {
            continue;
        };

        let rows = used_rows(&range);
        if rows.is_empty() {
            continue;
        }

        blocks.push(Block::Heading {
            level: 2,
            text: Inline::text(name.clone()),
        });

        let mut rows = rows.into_iter().map(Row::from_texts).collect::<Vec<_>>();
        // A sheet's first row is its header row, as in CSV.
        let header = rows.remove(0);
        blocks.push(Block::Table {
            header: Some(header),
            rows,
        });
    }

    Ok(blocks)
}

/// The sheet's cells as text, trimmed to the rectangle that actually holds data.
fn used_rows(range: &calamine::Range<Data>) -> Vec<Vec<String>> {
    let mut last_row = None;
    let mut last_column = 0usize;

    for (index, row) in range.rows().enumerate() {
        if let Some(column) = row.iter().rposition(|cell| !matches!(cell, Data::Empty)) {
            last_row = Some(index);
            last_column = last_column.max(column);
        }
    }

    let Some(last_row) = last_row else {
        return Vec::new();
    };

    range
        .rows()
        .take(last_row + 1)
        .map(|row| {
            (0..=last_column)
                .map(|column| row.get(column).map(cell_text).unwrap_or_default())
                .collect()
        })
        .collect()
}

/// One cell as text.
fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(text) => text.trim().to_string(),
        Data::Int(value) => value.to_string(),
        Data::Float(value) => number(*value),
        Data::Bool(value) => if *value { "TRUE" } else { "FALSE" }.to_string(),
        Data::DateTime(value) => {
            if value.is_duration() {
                duration(value.as_f64())
            } else {
                excel_date(value.as_f64())
            }
        }
        Data::DateTimeIso(text) | Data::DurationIso(text) => text.clone(),
        Data::Error(error) => format!("#{error:?}"),
    }
}

/// Format a float the way a spreadsheet shows it: no trailing `.0`.
fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    let mut text = format!("{value}");
    if text.contains('e') {
        text = format!("{value:.6}");
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    text
}

/// An Excel serial number as an ISO date, date-time or time.
///
/// Serial 1 is 1900-01-01. Excel also believes in a 1900-02-29 that never
/// existed, so serials from 61 onwards are one day ahead of the real count:
/// they anchor on 1899-12-30, while the first sixty anchor on 1899-12-31.
fn excel_date(serial: f64) -> String {
    let days = serial.trunc() as i64;
    let fraction = serial - serial.trunc();

    // A value below 1 is a time of day with no date part.
    if days == 0 {
        return time_of_day(fraction);
    }

    let epoch_offset = if days < 60 { 25_568 } else { 25_569 };
    let (year, month, day) = civil_from_days(days - epoch_offset);
    let date = format!("{year:04}-{month:02}-{day:02}");
    if fraction.abs() < 1e-9 {
        date
    } else {
        format!("{date} {}", time_of_day(fraction))
    }
}

fn time_of_day(fraction: f64) -> String {
    let total = (fraction * 86_400.0).round() as i64;
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// Excel durations are a count of days.
fn duration(days: f64) -> String {
    let total = (days * 86_400.0).round() as i64;
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    format!("{hours}:{minutes:02}:{seconds:02}")
}

/// Days since the Unix epoch → civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;

    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but valid xlsx: inline strings, one sheet per entry.
    fn xlsx(sheets: &[(&str, &[&[&str]])]) -> Vec<u8> {
        let mut entries: Vec<(String, String)> = Vec::new();

        let mut sheet_tags = String::new();
        let mut overrides = String::new();
        let mut rels = String::new();

        for (index, (name, rows)) in sheets.iter().enumerate() {
            let number = index + 1;
            sheet_tags.push_str(&format!(
                r#"<sheet name="{name}" sheetId="{number}" r:id="rId{number}"/>"#
            ));
            rels.push_str(&format!(
                r#"<Relationship Id="rId{number}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{number}.xml"/>"#
            ));
            overrides.push_str(&format!(
                r#"<Override PartName="/xl/worksheets/sheet{number}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#
            ));

            let mut body = String::new();
            for (row_index, row) in rows.iter().enumerate() {
                let row_number = row_index + 1;
                let mut cells = String::new();
                for (column, value) in row.iter().enumerate() {
                    let reference = format!("{}{row_number}", column_name(column));
                    cells.push_str(&format!(
                        r#"<c r="{reference}" t="inlineStr"><is><t>{value}</t></is></c>"#
                    ));
                }
                body.push_str(&format!(r#"<row r="{row_number}">{cells}</row>"#));
            }
            entries.push((
                format!("xl/worksheets/sheet{number}.xml"),
                format!(
                    r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{body}</sheetData></worksheet>"#
                ),
            ));
        }

        entries.push((
            "[Content_Types].xml".into(),
            format!(
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
                     <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
                     <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
                     {overrides}
                   </Types>"#
            ),
        ));
        entries.push((
            "_rels/.rels".into(),
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdWb" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.into(),
        ));
        entries.push((
            "xl/workbook.xml".into(),
            format!(
                r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{sheet_tags}</sheets></workbook>"#
            ),
        ));
        entries.push((
            "xl/_rels/workbook.xml.rels".into(),
            format!(
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#
            ),
        ));

        let borrowed: Vec<(&str, &str)> = entries
            .iter()
            .map(|(name, content)| (name.as_str(), content.as_str()))
            .collect();
        crate::extract::container::tests::zip_bytes(&borrowed)
    }

    fn column_name(index: usize) -> String {
        let mut index = index;
        let mut name = String::new();
        loop {
            name.insert(0, (b'A' + (index % 26) as u8) as char);
            if index < 26 {
                break;
            }
            index = index / 26 - 1;
        }
        name
    }

    #[test]
    fn every_sheet_becomes_a_heading_and_a_table() {
        let bytes = xlsx(&[
            ("Summary", &[&["part", "qty"], &["bolt", "4"]]),
            ("Notes", &[&["note"], &["ok"]]),
        ]);

        assert_eq!(
            parse(&bytes).unwrap(),
            vec![
                Block::Heading {
                    level: 2,
                    text: Inline::text("Summary")
                },
                Block::Table {
                    header: Some(Row::from_texts(["part", "qty"])),
                    rows: vec![Row::from_texts(["bolt", "4"])],
                },
                Block::Heading {
                    level: 2,
                    text: Inline::text("Notes")
                },
                Block::Table {
                    header: Some(Row::from_texts(["note"])),
                    rows: vec![Row::from_texts(["ok"])],
                },
            ]
        );
    }

    #[test]
    fn an_empty_sheet_is_skipped() {
        let bytes = xlsx(&[("Empty", &[]), ("Data", &[&["a"], &["1"]])]);
        let blocks = parse(&bytes).unwrap();
        assert_eq!(
            blocks.first(),
            Some(&Block::Heading {
                level: 2,
                text: Inline::text("Data")
            })
        );
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn a_non_workbook_is_an_error() {
        assert!(parse(b"not a spreadsheet").is_err());
    }

    #[test]
    fn cells_render_like_a_spreadsheet_shows_them() {
        assert_eq!(cell_text(&Data::Empty), "");
        assert_eq!(cell_text(&Data::Int(42)), "42");
        assert_eq!(cell_text(&Data::Float(4.5)), "4.5");
        assert_eq!(cell_text(&Data::Float(4.0)), "4");
        assert_eq!(cell_text(&Data::Bool(true)), "TRUE");
        assert_eq!(cell_text(&Data::String("  padded  ".into())), "padded");
    }

    #[test]
    fn excel_serials_become_iso_dates() {
        // Anchors cross-checked against a real calendar, including either side
        // of the phantom 1900-02-29.
        assert_eq!(excel_date(1.0), "1900-01-01");
        assert_eq!(excel_date(59.0), "1900-02-28");
        assert_eq!(excel_date(61.0), "1900-03-01");
        assert_eq!(excel_date(25_569.0), "1970-01-01");
        assert_eq!(excel_date(45_123.0), "2023-07-16");
        assert_eq!(excel_date(45_123.5), "2023-07-16 12:00:00");
        assert_eq!(excel_date(0.25), "06:00:00");
    }

    #[test]
    fn durations_are_hours_minutes_seconds() {
        assert_eq!(duration(1.5), "36:00:00");
        assert_eq!(duration(0.5 / 24.0), "0:30:00");
    }
}
