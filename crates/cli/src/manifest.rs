//! Machine-readable per-file status for one run (`--manifest <PATH>`).
//!
//! Exit codes are a *process*-level signal, so a batch that skipped three scans
//! and extracted forty documents still exits 0 and explains itself in prose on
//! stderr. An ingestion pipeline that wants to route those three scans through
//! OCR would have to either grep the log or drive its own loop over the files —
//! and a hand-rolled loop re-derives the output names, which is exactly where
//! `report.pdf` and `report.docx` collapse into one `report.md`.
//!
//! The manifest is the structured answer: one JSON array for the whole run, in
//! the order the run reports it, naming for every input what happened, why, and
//! the file that was actually written.
//!
//! ```json
//! [
//!   { "source": "docs/handbook.docx", "output": "out/handbook.md",
//!     "status": "extracted", "format": "docx" },
//!   { "source": "docs/scan.pdf", "output": null, "status": "skipped",
//!     "reason": "no_text_layer", "format": "pdf" }
//! ]
//! ```
//!
//! It is a report, not a policy: writing one changes no exit code and turns no
//! skip into a failure.

use std::path::Path;

use anyhow::{Context, Result};
use deepdoc_core::Format;
use serde::Serialize;

/// What became of one input.
#[derive(Debug, Serialize)]
pub struct FileStatus {
    /// The input path, as the run saw it.
    pub source: String,
    /// The file that was actually written — the name the batch chose, with
    /// colliding stems already resolved (`out/report.pdf.md`). `null` when
    /// nothing was written: a skip, an error, or output going to stdout.
    pub output: Option<String>,
    pub status: Status,
    /// Why there is no output. Absent on `extracted`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    /// The detected format, or `null` when detection did not recognise the file.
    pub format: Option<Format>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Extracted,
    Skipped,
    Error,
}

/// Why a file produced no output.
///
/// The distinction that earns the manifest its keep is `no_text_layer` (a
/// recognised document that is a scan — send it to OCR) against
/// `unsupported_format` (not a document at all — ignore it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    NoTextLayer,
    UnsupportedFormat,
    ParseError,
    IoError,
}

impl Reason {
    /// Map an error onto the reason the manifest reports.
    ///
    /// Anything the core does not name is an I/O problem — that is what a
    /// failed write or an unreadable folder is. OCR knows nothing about this
    /// mapping and neither does the core: the engine reports, the pipeline
    /// decides.
    pub fn of(error: &anyhow::Error) -> Reason {
        match error.downcast_ref::<deepdoc_core::Error>() {
            Some(deepdoc_core::Error::NoText { .. }) => Reason::NoTextLayer,
            Some(
                deepdoc_core::Error::Unsupported { .. }
                | deepdoc_core::Error::NotImplemented { .. },
            ) => Reason::UnsupportedFormat,
            Some(deepdoc_core::Error::Parse { .. }) => Reason::ParseError,
            _ => Reason::IoError,
        }
    }
}

/// Write the run's statuses as one JSON array.
pub fn write(path: &Path, statuses: &[FileStatus]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }

    let mut json =
        serde_json::to_string_pretty(statuses).context("cannot serialize the manifest")?;
    json.push('\n');
    std::fs::write(path, json).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepdoc_core::Error;
    use std::path::PathBuf;

    fn status(reason: Option<Reason>) -> FileStatus {
        FileStatus {
            source: "docs/scan.pdf".into(),
            output: None,
            status: Status::Skipped,
            reason,
            format: Some(Format::Pdf),
        }
    }

    /// The manifest is a public contract — this is what breaks when a field or
    /// a value is renamed.
    #[test]
    fn json_schema_snapshot() {
        let extracted = FileStatus {
            source: "docs/handbook.docx".into(),
            output: Some("out/handbook.md".into()),
            status: Status::Extracted,
            reason: None,
            format: Some(Format::Docx),
        };
        assert_eq!(
            serde_json::to_value([extracted, status(Some(Reason::NoTextLayer))]).unwrap(),
            serde_json::json!([
                {
                    "source": "docs/handbook.docx",
                    "output": "out/handbook.md",
                    "status": "extracted",
                    "format": "docx",
                },
                {
                    "source": "docs/scan.pdf",
                    "output": null,
                    "status": "skipped",
                    "reason": "no_text_layer",
                    "format": "pdf",
                },
            ])
        );
    }

    #[test]
    fn core_errors_map_onto_reasons() {
        let path = PathBuf::from("a.pdf");
        let cases = [
            (Error::NoText { path: path.clone() }, Reason::NoTextLayer),
            (
                Error::Unsupported { path: path.clone() },
                Reason::UnsupportedFormat,
            ),
            (
                Error::NotImplemented {
                    format: Format::Pdf,
                },
                Reason::UnsupportedFormat,
            ),
            (Error::parse(&path, "broken"), Reason::ParseError),
            (
                Error::io(&path, std::io::Error::other("nope")),
                Reason::IoError,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(Reason::of(&anyhow::Error::from(error)), expected);
        }
    }

    /// A problem the core never saw — a failed write, an unreadable folder.
    #[test]
    fn anything_else_is_an_io_problem() {
        assert_eq!(
            Reason::of(&anyhow::anyhow!("cannot read /nope")),
            Reason::IoError
        );
    }
}
