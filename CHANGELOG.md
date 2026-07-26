# Changelog

All notable changes to DeepDoc are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.3.0 — 2026-07-26

### Added

- **`--ocr`, an opt-in build feature** ([#3]). `deepdoc --ocr scan.pdf` recognizes a scan and
  prints Markdown in one command instead of two — for builds that ask for it. It is off by
  default and absent from every released binary: the models alone are 12 MB against a 3.8 MB
  binary, cargo-dist builds one artifact set for all channels (so "brew with models, crates.io
  without" is not expressible), and recognition is probabilistic, which is not something to put
  on the default path of a deterministic parser. Two features, so the cost is the one you choose:

  | Feature | What it adds |
  | --- | --- |
  | `ocr` | Recognition; models come from `--ocr-model <dir>` or `DEEPOCR_MODEL_DIR`. No weights, no network stack. |
  | `ocr-fetch-models` | …plus downloading them to the user cache on first run — the only build of DeepDoc that touches the network. |

  There is deliberately no `ocr-embed-models`. `deepocr-core` can compile its weights in, but it
  reads them with `include_bytes!` from a directory DeepOCR's own CI populates before building;
  the crate published to crates.io does not carry them, so the feature cannot compile for anyone
  installing DeepDoc normally. A build flag that only works inside one repository's CI is a trap.

  Recognition runs entirely in memory: the page becomes a searchable PDF that never reaches the
  filesystem, then goes through DeepDoc's ordinary PDF reader, so an OCR'd scan gets the same
  reading-order, column and heading handling as any born-digital file. A standalone `.png`,
  `.jpg` or `.tiff` also becomes readable — not a document to DeepDoc, but a page to DeepOCR.

  `--ocr` is an **extra attempt, never a new failure mode**: when recognition cannot help, the
  original verdict stands, so a batch skips exactly what it skipped before, for the same reason.
  It only triggers on a file that yields *no* text at all, so a mostly-text PDF with one scanned
  page inside still extracts what it has and leaves that page alone.
- **`"ocr": true` in the manifest** on files whose text was recognized rather than read. Absent on
  every other row, so a run without `--ocr` emits exactly the 0.2.0 schema. Deterministically
  parsed text and OCR output are not the same evidence — and a chunk `hash` that changed because
  a page was recognized differently is a different story from one that changed because the parser
  improved.

### Changed

- **`--ocr` now explains itself in every build.** It used to fail with clap's
  `unexpected argument '--ocr'`, which is true and useless. The flag is defined everywhere; a
  build without the feature rejects it with the feature to install and the two-tool loop that
  needs no special build. Same exit code (2).
- The dependency-graph promise is now explicitly about the **default build**. With the `ocr`
  feature the graph gains `deepocr-core` and its tree, including `dirs-sys` (pure Rust despite the
  name — `libc` and `option-ext`, nothing compiled) and `hayro`, a page rasterizer. Rendering
  pages is otherwise deliberately out of scope here; behind an off-by-default flag it is an
  intentional exception. The default build is unchanged and still contains no C or `-sys` crates.

### Notes

- Determinism is suspended for pages that go through recognition, and a run using `--ocr` says so
  on stderr. Everything else remains byte-for-byte reproducible.
- The `ocr` feature needs a newer toolchain than the default build (`deepocr-core` requires Rust
  1.92; DeepDoc itself still builds on 1.85).

[#3]: https://github.com/deeplabua/deepdoc/issues/3

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
