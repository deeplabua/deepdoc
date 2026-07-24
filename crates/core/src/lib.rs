//! DeepDoc core — any document to clean Markdown, JSON or text.
//!
//! The engine is built around a format-neutral [`Document`](model::Document)
//! model and the [`Extractor`](extract::Extractor) trait:
//!
//! ```text
//! detect  file type      → which Extractor
//! extract file + parsing → Document        (the only impure edge)
//! render  Document       → Markdown | JSON | text   (pure)
//! chunk   Document       → RAG chunks               (pure)
//! ```
//!
//! Adding a format means adding an `Extractor`; neither the CLI nor the
//! serializers change.
//!
//! ```no_run
//! use deepdoc_core::{extract::{self, ExtractOpts}, render::{self, RenderOpts}};
//!
//! let doc = extract::extract_path("notes.txt".as_ref(), &ExtractOpts::default())?;
//! print!("{}", render::to_markdown(&doc, &RenderOpts::default()));
//! # Ok::<(), deepdoc_core::Error>(())
//! ```

pub mod chunk;
pub mod detect;
pub mod error;
pub mod extract;
pub mod model;
pub mod render;

pub use detect::{Format, detect_path};
pub use error::{Error, Result, exit_code};
pub use extract::{ExtractOpts, Extractor, extract_path};
pub use model::{Block, Document, Inline, Metadata, Row, Span};
pub use render::{OutputFormat, RenderOpts, render, to_json, to_markdown, to_text};
