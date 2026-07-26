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

Include document metadata as YAML front-matter, or extract a PDF page range:

```sh
deepdoc paper.pdf --metadata
deepdoc big.pdf --pages 1-10
```

The original file is never modified — DeepDoc only reads.

Chunk for a RAG pipeline. Chunks are cut on block boundaries — never through a paragraph or
a table — and each one carries the chain of headings it sits under, a byte range into the
Markdown it came from, and a content hash:

```sh
deepdoc handbook.pdf --chunk 800 --format json
deepdoc handbook.pdf --chunk 800 --chunk-overlap 100 --format json
deepdoc handbook.pdf --chunk 800          # same chunks as Markdown, with the cuts marked
```

```json
{
  "source": "handbook.pdf",
  "meta": { "title": "Handbook", "pages": 24 },
  "chunks": [
    {
      "text": "## Onboarding\n\nYour first day is mostly paperwork…",
      "heading_path": ["Handbook", "Onboarding"],
      "source": "handbook.pdf",
      "byte_range": [1043, 1802],
      "hash": "sha256:4e9b39d64994e392fa7a19bc795b4272591d5442c2fc6b90ec432ff2bbf9efe9"
    }
  ]
}
```

#### Chunk hashes

`hash` is `sha256` over the chunk's **heading path and text together** —
`heading_path.join("\u{1F}") + "\u{1E}" + text`, with the ASCII unit and record separators as
the boundary. It answers "did this chunk change?" after a parser upgrade, so you re-embed the
chunks that moved instead of the whole corpus.

The heading path is in the hash on purpose. A chunk only contains its own heading when it
starts with one; for every other chunk the context (`Handbook > Onboarding > Day one`) lives
*outside* `text`. Re-filing the same sentences under a new heading is exactly the case worth
catching — hashing `text` alone would miss it.

The Markdown and text forms print a short form of the same hash in the chunk header, so
"which chunk is this?" is answerable in every format:

```
<!-- chunk 2/5 | Handbook > Payroll | bytes 296-390 | sha256:2b7c1d0f8a3e… -->
```

### Machine-readable batch status (`--manifest`)

Exit codes are a *process*-level signal: a batch that skipped three scans and extracted forty
documents still exits 0 and explains itself in prose on stderr. To route just those three
scans through OCR you would have to grep the log — or drive your own loop over the files, and
a hand-rolled loop re-derives the output names, which is where `report.pdf` and `report.docx`
quietly collapse into one `report.md`.

`--manifest <path>` writes one JSON array for the whole run instead — every input, what
happened to it, why, and the file that was actually written:

```sh
deepdoc ./corpus --recursive -o out/ --manifest run.json
```

```json
[
  { "source": "corpus/handbook.docx", "output": "out/handbook.md",
    "status": "extracted", "format": "docx" },
  { "source": "corpus/report.pdf", "output": "out/report.pdf.md",
    "status": "extracted", "format": "pdf" },
  { "source": "corpus/report.docx", "output": "out/report.docx.md",
    "status": "extracted", "format": "docx" },
  { "source": "corpus/scan.pdf", "output": null, "status": "skipped",
    "reason": "no_text_layer", "format": "pdf" },
  { "source": "corpus/notes.zip", "output": null, "status": "skipped",
    "reason": "unsupported_format", "format": null }
]
```

- `status` is `extracted`, `skipped` or `error`; `reason` (absent on `extracted`) is
  `no_text_layer`, `unsupported_format`, `parse_error` or `io_error`.
- `output` is the path DeepDoc really wrote, with colliding stems already resolved — never
  compute it yourself.
- `null` output means nothing was written: a skip, an error, or output going to stdout.
- Inputs that could not even be listed (a missing path, an unreadable folder) get an entry too,
  so the manifest never reports success for a branch of the tree the run never saw.
- It is a report, not a policy: writing one changes no exit code and turns no skip into a
  failure. Order is deterministic, and a single input gives an array of one.

The routing loop it exists for — exactly the scans, nothing else:

```sh
jq -r '.[] | select(.reason=="no_text_layer") | .source' run.json | xargs -n1 deepocr
```

### Scanned documents

A scan has no text to extract, so DeepDoc says so instead of inventing something: it exits
with code **4** and names the file. That is a clean routing signal for an ingestion pipeline —
make the scan searchable with [DeepOCR](https://github.com/deeplabua/deepocr) (a sibling tool,
same idea: pure Rust, local, models embedded), then extract the result:

```sh
deepocr scan.pdf -o scan.ocr.pdf   # invisible text layer over the page image
deepdoc scan.ocr.pdf               # now it is born-digital — parse as usual
```

OCR stays deliberately **outside** DeepDoc's released binaries: recognition is a probabilistic
step, and mixing it into a deterministic parser would cost the property this tool is built on.
The models alone are 12 MB against a 3.8 MB binary, and there is no way to ship "brew with
models, crates.io without" — so everyone would pay for a feature most runs never use.

#### One command instead of two (`--ocr`, opt-in at build time)

If you would rather have it in one command, build it in. The flag is off by default and absent
from every released binary:

```sh
cargo install deepdoc --features ocr-fetch-models
deepdoc scan.pdf --ocr
```

| Feature | What you get |
| --- | --- |
| `ocr` | Recognition, models supplied by you via `--ocr-model <dir>` or `DEEPOCR_MODEL_DIR`. No weights in the binary, no network stack in the graph — the air-gapped and reproducible-build option. |
| `ocr-fetch-models` | The above, plus downloading the weights into the user cache on first run. This is the **only** build of DeepDoc that ever touches the network. |

The weights live outside the binary either way. DeepOCR ships them, so
`--ocr-model /path/to/deepocr/models` works if you already have that tool.

`--ocr` also makes standalone scans readable — a `.png`, `.jpg` or `.tiff` is not a document to
DeepDoc, but it is a page to DeepOCR:

```sh
deepdoc ./scans --recursive -o out/ --ocr --manifest run.json
```

Three things worth knowing:

- **It is an extra attempt, never a new failure mode.** If recognition cannot help — the file is
  not a page at all — the original verdict stands, so a batch that skipped a file without `--ocr`
  skips it with `--ocr` too, for the same reason.
- **The manifest says so.** A file whose text was recognised rather than read carries
  `"ocr": true`. Deterministically parsed text and OCR output are not the same evidence, and an
  index audit needs to tell them apart. The field is absent on every other row, so a run without
  `--ocr` emits exactly the schema it did before.
- **Determinism is suspended for the pages it touches**, and the run says so on stderr. Everything
  else in DeepDoc is still byte-for-byte reproducible.

Recognition happens entirely in memory: the page is recognised into a searchable PDF that never
reaches the filesystem, then parsed by DeepDoc's ordinary PDF reader — which is how an OCR'd scan
gets the same reading-order, column and heading handling as any born-digital file.

A document that yields *some* text is not a candidate: `--ocr` triggers on a file that gives
nothing at all, so a mostly-text PDF with one scanned page inside it still extracts the text it
has and leaves that page alone. Run such a file through `deepocr` directly.

### Supported inputs (v0.1)

| Family | Formats |
| --- | --- |
| Word processing | `.docx`, `.odt`, `.rtf` |
| Presentations | `.pptx`, `.odp` |
| Spreadsheets | `.xlsx`, `.ods`, `.csv` |
| Books | `.epub` |
| Web / text | `.html`, `.md`, `.txt` |
| PDF | `.pdf` (born-digital text; for scans see [Scanned documents](#scanned-documents)) |

## How it works

DeepDoc parses each format into one neutral `Document` model (headings, paragraphs, tables,
lists, metadata, page markers), then serializes that to Markdown, JSON, or text. Office and
e-book formats are ZIP + XML (parsed directly); spreadsheets go through a fast pure-Rust
reader; PDF text is reconstructed from the page's text objects with column/reading-order
detection. Everything runs in-process — nothing is uploaded and nothing is shelled out to.

## Scope

DeepDoc extracts **born-digital** documents (real embedded text). Scanned pages and images go
through [DeepOCR](https://github.com/deeplabua/deepocr) first (see
[Scanned documents](#scanned-documents)), or through the opt-in `--ocr` build feature that links
its engine straight in — off by default, so the released binary stays small and model-free.
High-fidelity table recovery from complex/scanned PDFs
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
- **Chip in a tip** via the **Sponsor** button at the top of the repo, or directly through the
  [monobank jar](https://send.monobank.ua/jar/9NjMEHrvCW). It supports a Ukrainian developer
  and keeps the project moving. Thank you 💙💛

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
