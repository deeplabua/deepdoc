//! Core error type. The CLI maps these onto the exit codes from the spec.

use std::path::{Path, PathBuf};

use crate::detect::Format;

pub type Result<T> = std::result::Result<T, Error>;

/// Process exit codes (see `docs/PRD/02-cli-spec.md`).
pub mod exit_code {
    /// Success.
    pub const OK: i32 = 0;
    /// Generic failure.
    pub const FAILURE: i32 = 1;
    /// Invalid arguments (also what clap returns on its own).
    pub const USAGE: i32 = 2;
    /// Recognised document, but no extractable text — looks like a scan, needs `--ocr`.
    pub const NO_TEXT: i32 = 4;
    /// Unsupported file type.
    pub const UNSUPPORTED: i32 = 5;
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("unsupported file type: {path}")]
    Unsupported { path: PathBuf },

    /// The type is known but its extractor has not landed yet (pre-v0.1 only).
    #[error("{format} is not supported yet")]
    NotImplemented { format: Format },

    #[error("{path} has no extractable text — it looks like a scan (needs --ocr)")]
    NoText { path: PathBuf },

    #[error("cannot parse {path}: {message}")]
    Parse { path: PathBuf, message: String },

    #[error("invalid page range {input:?}: {message}")]
    InvalidPageRange { input: String, message: String },
}

impl Error {
    /// Read error carrying the path that failed.
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.as_ref().to_path_buf(),
            source,
        }
    }

    pub fn parse(path: impl AsRef<Path>, message: impl Into<String>) -> Self {
        Error::Parse {
            path: path.as_ref().to_path_buf(),
            message: message.into(),
        }
    }

    /// The exit code this error should produce.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Unsupported { .. } | Error::NotImplemented { .. } => exit_code::UNSUPPORTED,
            Error::NoText { .. } => exit_code::NO_TEXT,
            Error::InvalidPageRange { .. } => exit_code::USAGE,
            Error::Io { .. } | Error::Parse { .. } => exit_code::FAILURE,
        }
    }
}
