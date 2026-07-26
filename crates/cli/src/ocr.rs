//! Optional OCR (`--ocr`), compiled in only with the `ocr` feature.
//!
//! DeepDoc's promise is a deterministic parser, so recognition is deliberately
//! *not* on the default path — the released binaries have no models in them and
//! no code from this module. What `--ocr` buys is one command instead of two for
//! people who build with it; the two-tool loop (`deepocr` then `deepdoc`) stays
//! the answer for everyone else, and is what the `no_text_layer` reason in the
//! manifest exists to drive.
//!
//! The integration is one decision point and no new concepts. When extraction
//! comes back "recognised document, no extractable text", the file goes through
//! [`deepocr_core`] and comes back as a **searchable PDF held in memory** — the
//! page raster with an invisible text layer over it — which is then parsed by
//! DeepDoc's ordinary PDF reader. Round-tripping through a PDF rather than
//! taking the recognised words directly is what earns the reading-order,
//! column and heading heuristics for free: the OCR'd page arrives as
//! born-digital, because that is exactly what it now is. Nothing is written to
//! disk on the way.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use deepocr_core::{
    DEFAULT_DPI, OcrBackend as _, PreprocessOpts, QuarterTurn, SearchablePdf, TextPolicy,
    preprocess,
};

/// A loaded OCR engine, shared across the batch.
///
/// Loading allocates the model weights and takes a moment, so a run builds one
/// and every worker thread borrows it.
pub struct Engine {
    backend: deepocr_core::OcrsBackend,
}

impl Engine {
    /// Load the models and the recognizer.
    ///
    /// `models` is `--ocr-model`; unset, the resolver falls back to
    /// `DEEPOCR_MODEL_DIR` and then to the user cache, and — in an
    /// `ocr-fetch-models` build only — downloads them there on first run.
    pub fn load(models: Option<&Path>) -> Result<Engine> {
        let resolved = resolve_models(models)?;
        let backend = deepocr_core::OcrsBackend::new(&resolved)
            .context("cannot load the OCR models")
            .map_err(help_with_models)?;
        Ok(Engine { backend })
    }

    /// Recognize every page of `path` and return a searchable PDF, in memory.
    ///
    /// Pages that already carry text keep it and are passed through untouched,
    /// so a document that is only partly scanned does not get a second,
    /// recognised copy of the text it already had.
    pub fn to_searchable_pdf(&self, path: &Path) -> Result<Recognized> {
        let document = deepocr_core::Document::open(path)
            .with_context(|| format!("cannot read {} for OCR", path.display()))?;

        let mut pdf = SearchablePdf::new();
        let mut pages = 0usize;

        for index in 0..document.page_count() {
            let page = document
                .page(index, DEFAULT_DPI, TextPolicy::Skip)
                .with_context(|| format!("cannot read page {} of {}", index + 1, path.display()))?;

            let words = if page.has_text {
                Vec::new()
            } else {
                let prepared = preprocess::prepare(
                    &page,
                    &PreprocessOpts {
                        deskew: false,
                        turn: QuarterTurn::None,
                    },
                );
                let words = self
                    .backend
                    .recognize(&prepared.image)
                    .with_context(|| format!("cannot recognize page {}", index + 1))?;
                pages += 1;
                prepared.map_words_onto_page(words, &page)
            };

            pdf.add_page(&page, &words, QuarterTurn::None)
                .with_context(|| format!("cannot assemble page {}", index + 1))?;
        }

        let bytes = pdf
            .into_bytes()
            .context("cannot assemble the searchable PDF")?;
        Ok(Recognized { bytes, pages })
    }
}

/// A recognised document, as the bytes of a searchable PDF.
pub struct Recognized {
    pub bytes: Vec<u8>,
    /// How many pages actually went through recognition.
    pub pages: usize,
}

#[cfg(feature = "ocr-fetch-models")]
fn resolve_models(explicit: Option<&Path>) -> Result<deepocr_core::models::ResolvedModels> {
    // The only build of DeepDoc that ever touches the network, and only to fill
    // the model cache the first time.
    deepocr_core::models::resolve_or_fetch(explicit, &mut deepocr_core::models::SilentProgress)
        .context("cannot resolve the OCR models")
        .map_err(help_with_models)
}

#[cfg(not(feature = "ocr-fetch-models"))]
fn resolve_models(explicit: Option<&Path>) -> Result<deepocr_core::models::ResolvedModels> {
    deepocr_core::models::resolve(explicit)
        .context("cannot resolve the OCR models")
        .map_err(help_with_models)
}

/// Say where models come from, because "not found" alone leaves the user with
/// no next step — the weights live outside the binary by design.
fn help_with_models(error: anyhow::Error) -> anyhow::Error {
    if cfg!(feature = "ocr-fetch-models") {
        return error;
    }
    anyhow!(
        "{error:#}\n\nThis build carries no model weights. Point it at a copy:\n    \
         deepdoc scan.pdf --ocr --ocr-model /path/to/models\n    \
         DEEPOCR_MODEL_DIR=/path/to/models deepdoc scan.pdf --ocr\n\
         Or build one that fetches them once: \
         cargo install deepdoc --features ocr-fetch-models\n\
         Models ship with DeepOCR: https://github.com/deeplabua/deepocr"
    )
}

/// Where `--ocr-model` came from, for the log line.
pub fn model_source(explicit: Option<&PathBuf>) -> String {
    match explicit {
        Some(path) => path.display().to_string(),
        None => deepocr_core::models::model_dir_from_env()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "the model cache".to_string()),
    }
}
