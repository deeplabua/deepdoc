# Changelog

All notable changes to DeepDoc are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 — 2026-07-26

Auditable ingestion: a parser upgrade now says which chunks it actually changed, and a batch now
answers "what happened to every file?" in JSON instead of prose on stderr. Both came from
engineers reading the launch write-up ([#1], [#2]).

### Added

- **A content hash on every chunk** ([#1]). `--chunk --format json` gives each chunk a
  `hash: "sha256:<hex>"`, so re-extracting a corpus after a parser upgrade tells you which chunks
  moved and which can keep their embeddings. The hash covers the chunk's **heading path and text
  together** (`heading_path.join("\u{1F}") + "\u{1E}" + text`, the ASCII unit and record
  separators as the boundary): a chunk only contains its own heading when it starts with one, so
  for everything else the context lives outside `text`, and re-filing the same sentences under a
  new heading — the exact case worth catching — would otherwise hash identically.
- **`--manifest <path>`** ([#2]): one JSON array for the whole run naming, per input, its
  `status` (`extracted` / `skipped` / `error`), the `reason` there is no output (`no_text_layer`,
  `unsupported_format`, `parse_error`, `io_error`), the detected `format`, and the `output` file
  that was actually written. Exit code 4 is a *process*-level signal, so routing scans to OCR
  meant either grepping stderr or hand-rolling a loop over the files — and a hand-rolled loop
  re-derives the output names, where `report.pdf` and `report.docx` collapse into one
  `report.md`. `output` reports the name the batch really chose (`out/report.pdf.md`), so the
  pipeline never computes it. Inputs that could not even be listed get an entry too. It works
  without `--recursive` (an array of one), is deterministically ordered, and is a report, not a
  policy: it changes no exit code and turns no skip into a failure.

  ```sh
  deepdoc ./corpus --recursive -o out/ --manifest run.json
  jq -r '.[] | select(.reason=="no_text_layer") | .source' run.json | xargs -n1 deepocr
  ```

### Changed

- **Breaking for `deepdoc-core` users**: `Chunk` has a new public field, `hash`, so code that
  builds one with a struct literal (`Chunk { text, heading_path, source, byte_range }`) no longer
  compiles. Add `hash: deepdoc_core::chunk::chunk_hash(&heading_path, &text)`. Reading chunks —
  the common case — is unaffected.
- The chunk header in `--format md` / `--format text` now carries a short form of the hash, so
  "which chunk is this?" is answerable in every format, not only in JSON:
  `<!-- chunk 2/5 | Handbook > Payroll | bytes 296-390 | sha256:2b7c1d0f8a3e… -->`.

### Notes

- `sha2` (RustCrypto) does the hashing: pure Rust, MIT OR Apache-2.0, and already in the graph
  behind the PDF reader — the dependency tree gained no new crates, and still contains no C or
  `-sys` crates.

[#1]: https://github.com/deeplabua/deepdoc/issues/1
[#2]: https://github.com/deeplabua/deepdoc/issues/2

## 0.1.1 — 2026-07-26

### Changed

- A document with no extractable text now points at the tool that fixes it. The message used to
  say a scan "needs `--ocr`" — a flag DeepDoc does not have, and rejects with exit 2. It now names
  [DeepOCR](https://github.com/deeplabua/deepocr), which writes an invisible text layer over the
  page image; the result is born-digital and extracts normally. Exit code 4 is unchanged, so an
  ingestion pipeline keeps its routing signal.
- The README documents that loop under **Scanned documents**. OCR stays deliberately outside this
  parser: recognition is probabilistic, and mixing it in would cost the determinism DeepDoc is
  built on. A `--ocr` feature that links DeepOCR's engine in directly remains planned and off by
  default, so the released binary stays small and model-free.

## 0.1.0 — 2026-07-24

First release. `deepdoc report.pdf` prints clean Markdown — one static binary, no JVM, no Python,
no model downloads, and nothing leaves the machine.

### Added

- **Thirteen formats in one binary**, all parsed in-process: `.docx`, `.pptx`, `.xlsx`, `.odt`,
  `.odp`, `.ods`, `.epub`, `.rtf`, `.html`, `.md`, `.csv`, `.txt`, and born-digital `.pdf`.
  The format is detected from the file's signature, then its extension — the name may lie.
- **One neutral `Document` model** behind every format (headings, paragraphs, lists, tables, code,
  images' alt text, page markers, metadata), and **pure serializers** on top of it:
  `--format md` (default), `--format json`, `--format text`. A new format plugs in without
  touching the CLI or the renderers.
- **PDF structure, not just text.** Glyphs are segmented by a recursive XY-cut (horizontal bands
  before columns, so a full-width heading cannot bridge a column gutter), assembled into lines and
  paragraphs, with hyphenated words rejoined. Headings come from relative type size,
  conservatively. `--pages` selects a range; page markers keep the document's own numbering.
- **RAG chunking** (`--chunk [<size>]`, `--chunk-overlap <n>`). Chunks are cut on block boundaries
  — a paragraph or a table is never split down the middle unless it alone exceeds the target — and
  each one carries the chain of enclosing headings plus a `byte_range` into the Markdown rendering,
  so slicing that output with the range returns the chunk verbatim. Overlap is taken in whole
  blocks. Sizes count characters: a tokenizer would mean a model file in a binary whose promise is
  that there is nothing to install.
- **Batch extraction**: `--recursive` walks a folder and mirrors its tree into `--output`, files
  extracted across cores. A folder holds what it holds, so an unsupported type or a scan is a
  *skip* in the `✓ N extracted, M skipped` summary rather than a failure — but a file named on the
  command line is a failure with its own exit code. Colliding output names keep their original
  extension (`report.txt.md`), so a batch never silently drops a document.
- **Deterministic output.** The same input gives the same bytes: extraction runs in parallel, but
  stdout and the log stay in input order, and nothing iterates a hash map into the output.
- **Exit codes** for pipelines: `0` success, `1` failure, `2` bad arguments, `4` recognized but no
  extractable text (looks like a scan), `5` unsupported type.
- **`--metadata`** prepends title / author / created / language / publisher / page count as YAML
  front-matter; `--format json` always carries them.
- **`deepdoc-core` as a library** — embed extraction in a Rust service with no subprocess and no
  cloud.
- Distribution: prebuilt binaries for macOS (arm64, x64), Linux x64 and Windows x64, shell and
  PowerShell installers, `brew install deeplabua/tap/deepdoc`, and both crates on crates.io.

### Notes

- v0.1 targets **born-digital** documents. Scanned pages have no text to extract; DeepDoc says so
  (exit 4) instead of inventing content.
- Tables in PDFs are deliberately left as paragraphs: a wrong table reads worse than the text it
  was reconstructed from.
- Every dependency is permissive (MIT / Apache-2.0 / BSD / MPL / Zlib), and the graph contains no
  C or `-sys` crates — that is what keeps the binary self-contained.
