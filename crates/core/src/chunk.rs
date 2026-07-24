//! RAG chunking: `Document` → chunks that carry their heading context.
//!
//! Phase 0 status: types and signature only. The real splitter — block-aligned
//! cuts, heading paths, overlap, byte ranges — is Phase 6 (session 006).

use serde::{Deserialize, Serialize};

use crate::model::Document;

/// Default chunk size, in characters (see `docs/PRD/02-cli-spec.md`).
pub const DEFAULT_CHUNK_SIZE: usize = 800;

/// Options for [`chunk`].
#[derive(Debug, Clone)]
pub struct ChunkOpts {
    /// Target chunk size, in characters.
    pub size: usize,
    /// Overlap between neighbouring chunks, in characters.
    pub overlap: usize,
}

impl Default for ChunkOpts {
    fn default() -> Self {
        Self {
            size: DEFAULT_CHUNK_SIZE,
            overlap: 0,
        }
    }
}

/// One RAG chunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub text: String,
    /// Enclosing headings, outermost first (`["Introduction", "Method"]`).
    pub heading_path: Vec<String>,
    /// Byte range of the chunk inside the rendered document.
    pub byte_range: (usize, usize),
}

/// Split a document into chunks. Pure.
///
/// TODO(Phase 6): cut on block boundaries, attach the heading path, honour
/// `overlap`, and report real byte ranges. For now the whole document is one
/// chunk, so `--chunk` is wired end-to-end but not yet meaningful.
pub fn chunk(doc: &Document, _opts: &ChunkOpts) -> Vec<Chunk> {
    let text = crate::render::to_text(doc);
    if text.trim().is_empty() {
        return Vec::new();
    }
    let len = text.len();
    vec![Chunk {
        text,
        heading_path: Vec::new(),
        byte_range: (0, len),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Block, Metadata};

    #[test]
    fn empty_document_yields_no_chunks() {
        let doc = Document::new(Metadata::default());
        assert!(chunk(&doc, &ChunkOpts::default()).is_empty());
    }

    #[test]
    fn document_text_lands_in_a_chunk() {
        let doc = Document {
            meta: Metadata::default(),
            blocks: vec![Block::paragraph("hello")],
        };
        let chunks = chunk(&doc, &ChunkOpts::default());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text.trim(), "hello");
    }
}
