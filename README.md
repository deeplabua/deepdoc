# DeepDoc

> Any document → clean Markdown. One command, local, no cloud.

[![CI](https://github.com/deeplabua/deepdoc/actions/workflows/ci.yml/badge.svg)](https://github.com/deeplabua/deepdoc/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/deepdoc.svg)](https://crates.io/crates/deepdoc)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Stars](https://img.shields.io/github/stars/deeplabua/deepdoc?style=flat&logo=github&label=Star)](https://github.com/deeplabua/deepdoc/stargazers)

`deepdoc` turns a document into clean Markdown (or structured JSON) in a single command.
Point it at a `.docx`, `.pdf`, `.pptx`, `.xlsx`, `.odt`, `.epub`, `.html` — one static
binary reads them all and gives you Markdown ready for an LLM, a RAG pipeline, or a diff.
**Locally, no upload, no JVM, no Python, no model downloads.**

A pure-Rust document extractor.

> **Status:** early development (v0.1). Handles **born-digital** documents (real text, not
> scans) across a wide set of formats, with Markdown / JSON output and RAG chunking. Install
> via Homebrew or crates.io.
>
> If DeepDoc is useful to you, please **[⭐ star the repo](https://github.com/deeplabua/deepdoc)** — it genuinely helps. See [Support](#support) to chip in.

## Why

Everyone building on LLMs hits the same wall: *"I have a folder of documents — get me the
text as clean Markdown."* The usual options are **Apache Tika** (a JVM you have to run and
babysit), **Python stacks** (`unstructured`, Docling, MarkItDown — pip, native wheels, model
downloads), or **cloud parsers** (LlamaParse — upload your private documents to someone
else's server, pay per page). DeepDoc answers the one question directly: **any document →
clean Markdown, one binary, on your machine.**

DeepDoc doesn't try to out-parse ML tools on messy scans — it's the **fast, deterministic
path for the 80% of documents that are born-digital**, with an OCR fallback planned for the
rest.

## Install

Homebrew (macOS / Linux):

```sh
brew install deeplabua/tap/deepdoc
```

From crates.io:

```sh
cargo install deepdoc
```

Or download a prebuilt binary from the [releases page](https://github.com/deeplabua/deepdoc/releases).

No runtime dependencies — the binary is self-contained.

## Usage

Convert a document to Markdown (prints to stdout):

```sh
deepdoc report.pdf
```

Pick the output format, or write to a file:

```sh
deepdoc slides.pptx --format md        # Markdown (default)
deepdoc report.docx --format json      # structured: metadata + blocks + offsets
deepdoc data.xlsx  --format text       # plain text, no formatting
deepdoc report.pdf -o report.md
```

Batch a whole folder (mirrors the tree into an output dir):

```sh
deepdoc ./docs --recursive -o out/
```

Chunk for a RAG pipeline (each chunk keeps its heading context):

```sh
deepdoc handbook.pdf --chunk 800 --format json
```

Include document metadata as YAML front-matter, or extract a PDF page range:

```sh
deepdoc paper.pdf --metadata
deepdoc big.pdf --pages 1-10
```

The original file is never modified — DeepDoc only reads.

### Supported inputs (v0.1)

| Family | Formats |
| --- | --- |
| Word processing | `.docx`, `.odt`, `.rtf` |
| Presentations | `.pptx`, `.odp` |
| Spreadsheets | `.xlsx`, `.ods`, `.csv` |
| Books | `.epub` |
| Web / text | `.html`, `.md`, `.txt` |
| PDF | `.pdf` (born-digital text; scanned PDFs need the planned `--ocr`) |

## How it works

DeepDoc parses each format into one neutral `Document` model (headings, paragraphs, tables,
lists, metadata, page markers), then serializes that to Markdown, JSON, or text. Office and
e-book formats are ZIP + XML (parsed directly); spreadsheets go through a fast pure-Rust
reader; PDF text is reconstructed from the page's text objects with column/reading-order
detection. Everything runs in-process — nothing is uploaded and nothing is shelled out to.

## Scope

DeepDoc extracts **born-digital** documents (real embedded text). Scanned pages and images
need OCR, which is a planned **feature-gated** path (pure-Rust `ocrs`, or `tesseract`), off
by default to keep the binary small. High-fidelity table recovery from complex/scanned PDFs
is where ML parsers (Docling, Marker) win — DeepDoc's lane is speed, determinism, and zero
dependencies on the clean majority.

## Library

The extractor is also a crate — depend on `deepdoc-core` to embed extraction in your own
Rust service (no subprocess, no cloud):

```toml
[dependencies]
deepdoc-core = "0"
```

## Support

DeepDoc is free and open-source, built and maintained by one developer.

- **[⭐ Star the repo](https://github.com/deeplabua/deepdoc)** — the cheapest way to help;
  it boosts visibility so more people find the tool.
- **Chip in a tip** via the **Sponsor** button at the top of the repo. It supports a
  Ukrainian developer and keeps the project moving. Thank you 💙💛

## Part of DeepLab

DeepDoc is part of [DeepLab](https://deeplab.tools) — a line of tools for developers and
product teams.

## License

Licensed under either of

- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
