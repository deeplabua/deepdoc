# CLAUDE.md — DeepDoc

Context for contributors / Claude. Read at the start of a session.

## What this is

DeepDoc is a command-line tool (and a library) that extracts **clean Markdown** — or
structured JSON — from any document in one command ("turn this docx / pdf / pptx into
Markdown for my RAG pipeline"). Free and open-source; part of the DeepLab line of tools.

Engine: **pure Rust**. No JVM, no Python, no cloud, and — for the v0.1 scope — **no native
C/C++ dependency and nothing to install besides the binary**. Extraction parses document
structure; it does not render pages.

## Status

Pre-alpha / early development. v0.1 targets **born-digital documents** (real text, not
scans) across a wide set of formats.

## Product boundaries — IMPORTANT

"Extract" spans different formats, but the pitch is breadth in one static binary. To stay
focused:

- **v0.1 = born-digital only.** No OCR, no ML. Scanned PDFs / images are out of scope for
  v0.1 (detect + a clear "looks scanned, needs --ocr" message, exit code 4).
- **OCR (`ocrs`/`rten`, pure Rust, ships weights) and ML table recovery** are a planned,
  **feature-gated** path — not v0.1. Keep the default binary tiny and model-free.
- **Rendering / thumbnails are a different tool (DeepThumb)** — don't add page rasterizing
  here.

The core is designed around an `Extractor` trait (`supports` / `extract`) that yields a
format-neutral `Document` model; **pure** serializers render `Document` → Markdown / JSON /
text / chunks. New formats plug in without touching the CLI or the serializers.

## Structure

```
crates/
  cli/    # clap, arg parsing, routing (bin: deepdoc)  [may live as crates/deepdoc]
  core/   # pure, testable library (deepdoc-core)
    detect     # file type -> which Extractor
    model      # the Document IR (blocks, tables, metadata)
    extract    # trait Extractor + per-format impls (docx, pptx, xlsx, odf, epub, html, pdf, …)
    render     # Document -> Markdown | JSON | text   (pure)
    chunk      # Document -> RAG chunks with heading context  (pure)
```

Testing principle: the value-carrying logic (`render`, `chunk`, `detect` dispatch, table →
Markdown) lives as **pure functions** unit-tested on small in-memory `Document`s and tiny
fixture files. `extract` (file I/O + parsing) is the impure edge.

## Commands

```sh
cargo build
cargo run -- <args>          # e.g. cargo run -- report.pdf --format md
cargo test
cargo clippy -- -D warnings
cargo fmt
```

## Requirements

**None at runtime for v0.1** — a single static binary, no external tools. (This is a
headline differentiator vs Tika/JVM and Docling/Python.) The optional `ocr` feature (later)
either ships model weights (`ocrs`) or shells out to `tesseract` — both off by default.

## Code style — IMPORTANT

- **All code comments and doc-comments in English** (`//`, `///`, `//!`), as are commit
  messages. Chat / PRD / status docs are Russian; code is English.
- `cargo fmt` before commit; `cargo clippy -- -D warnings` must be clean.
- **Licence hygiene:** every dependency must be permissive (MIT / Apache-2.0 / BSD / MPL /
  Zlib). **No GPL/AGPL/commercial-only crates** (learned the hard way on DeepShrink's
  `imagequant`). Check a new dep's licence before adding it.

## Version control

This project uses **jj (Jujutsu)**. Pre-alpha: commit directly to `main`; commit messages
in English.

```sh
jj git fetch
jj new main@origin -m "feat: ..."   # only when the working copy is empty
# ...edits...
jj describe -m "feat: message"      # English only
jj bookmark set main -r @
jj git push --bookmark main
```

## Part of DeepLab

DeepDoc is part of [DeepLab](https://deeplab.tools). Multi-repo: this is the standalone
public CLI + library repository. Product roadmap and planning live in the private DeepLab
workspace, not in this repo.
