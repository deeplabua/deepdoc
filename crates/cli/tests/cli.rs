//! End-to-end checks on the built binary. No test-harness dependency: the
//! binary's path comes from `CARGO_BIN_EXE_*`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_deepdoc");

/// A scratch directory that removes itself.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let path = std::env::temp_dir().join(format!("deepdoc-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("cannot create temp dir");
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, contents).expect("cannot write fixture");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn deepdoc<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new(BIN)
        .args(args)
        .output()
        .expect("cannot run deepdoc")
}

#[test]
fn help_lists_the_documented_flags() {
    let output = deepdoc(["--help"]);
    assert!(output.status.success());

    let help = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--format",
        "--output",
        "--recursive",
        "--pages",
        "--metadata",
        "--chunk",
        "--chunk-overlap",
        "--quiet",
        "--verbose",
    ] {
        assert!(
            help.contains(flag),
            "--help does not mention {flag}:\n{help}"
        );
    }
}

#[test]
fn missing_input_is_a_usage_error() {
    let output = deepdoc(Vec::<String>::new());
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn unsupported_type_exits_with_five() {
    let dir = TempDir::new("unsupported");
    let file = dir.write("mystery.bin", "\u{0}\u{1}\u{2}not a document");

    let output = deepdoc([file.as_os_str()]);
    assert_eq!(output.status.code(), Some(5));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unsupported"),
        "expected an 'unsupported' message"
    );
}

#[test]
fn a_recognised_but_broken_document_is_a_read_failure() {
    let dir = TempDir::new("broken");
    let file = dir.write("book.pdf", "%PDF-1.7\ntrailer");

    // Every format has an extractor now, so a PDF header with nothing behind
    // it is a damaged file (exit 1), not an unsupported type (exit 5).
    let output = deepdoc([file.as_os_str()]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn document_without_text_exits_with_four() {
    let dir = TempDir::new("notext");
    let file = dir.write("blank.txt", "   \n\n  \n");

    let output = deepdoc([file.as_os_str()]);
    assert_eq!(output.status.code(), Some(4));
}

#[test]
fn text_file_renders_markdown_to_stdout() {
    let dir = TempDir::new("markdown");
    let file = dir.write("notes.txt", "first paragraph\n\nsecond paragraph\n");

    let output = deepdoc([file.as_os_str()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "first paragraph\n\nsecond paragraph\n"
    );
}

#[test]
fn json_output_carries_blocks() {
    let dir = TempDir::new("json");
    let file = dir.write("notes.txt", "hello");

    let output = deepdoc([file.as_os_str(), "--format".as_ref(), "json".as_ref()]);
    assert_eq!(output.status.code(), Some(0));

    let json = String::from_utf8_lossy(&output.stdout);
    assert!(json.contains("\"blocks\""), "unexpected json: {json}");
    assert!(json.contains("\"paragraph\""), "unexpected json: {json}");
}

#[test]
fn html_file_renders_structure_to_markdown() {
    let dir = TempDir::new("html");
    let file = dir.write(
        "page.html",
        "<html><head><title>T</title></head><body><nav>menu</nav>\
         <h1>Report</h1><p>Revenue grew <strong>12%</strong>.</p>\
         <ul><li>one</li><li>two</li></ul></body></html>",
    );

    let output = deepdoc([file.as_os_str()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "# Report\n\nRevenue grew **12%**.\n\n- one\n- two\n"
    );
}

#[test]
fn csv_file_renders_a_table() {
    let dir = TempDir::new("csv");
    let file = dir.write("data.csv", "part,qty\nbolt,4\n");

    let output = deepdoc([file.as_os_str()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "| part | qty |\n| ---- | --- |\n| bolt | 4   |\n"
    );
}

#[test]
fn rtf_file_renders_paragraphs() {
    let dir = TempDir::new("rtf");
    let file = dir.write(
        "note.rtf",
        r"{\rtf1\ansi\pard first\par {\b bold} second\par }",
    );

    let output = deepdoc([file.as_os_str()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "first\n\n**bold** second\n"
    );
}

#[test]
fn office_archives_route_through_the_cli() {
    let dir = TempDir::new("office");

    // A docx is a ZIP; build the smallest one the extractor will accept.
    let document = r#"<w:document xmlns:w="w"><w:body>
          <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Report</w:t></w:r></w:p>
          <w:p><w:r><w:t>Body text.</w:t></w:r></w:p>
        </w:body></w:document>"#;
    let path = dir.path().join("report.docx");
    std::fs::write(&path, minimal_zip(&[("word/document.xml", document)])).unwrap();

    let output = deepdoc([path.as_os_str()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "# Report\n\nBody text.\n"
    );
}

#[test]
fn epub_routes_through_the_cli_with_front_matter() {
    let dir = TempDir::new("epub");

    let container = r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
          <rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles>
        </container>"#;
    let package = r#"<package xmlns="http://www.idpf.org/2007/opf">
          <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
            <dc:title>A Short Book</dc:title><dc:creator>Ada</dc:creator>
          </metadata>
          <manifest><item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/></manifest>
          <spine><itemref idref="c1"/></spine>
        </package>"#;
    let chapter = "<html><head><title>Chapter One</title></head><body><p>text</p></body></html>";

    let path = dir.path().join("book.epub");
    std::fs::write(
        &path,
        // `mimetype` must come first for the format to be recognised.
        minimal_zip(&[
            ("mimetype", "application/epub+zip"),
            ("META-INF/container.xml", container),
            ("content.opf", package),
            ("ch1.xhtml", chapter),
        ]),
    )
    .unwrap();

    let output = deepdoc([path.as_os_str(), "--metadata".as_ref()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "---\ntitle: \"A Short Book\"\nauthor: \"Ada\"\nformat: \"epub\"\n---\n\n\
         # Chapter One\n\ntext\n"
    );
}

#[test]
fn pdf_routes_through_the_cli() {
    let dir = TempDir::new("pdf");
    let path = dir.path().join("report.pdf");
    // A title alone is just a line: a heading is only a heading relative to
    // the body type around it, so the fixture carries both.
    std::fs::write(
        &path,
        minimal_pdf(
            "BT /F1 24 Tf 72 720 Td (Quarterly Report) Tj ET\n\
             BT /F1 10 Tf 72 680 Td (Revenue grew twelve per cent.) Tj ET\n",
        ),
    )
    .unwrap();

    let output = deepdoc([path.as_os_str()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "# Quarterly Report\n\nRevenue grew twelve per cent.\n"
    );
}

#[test]
fn a_pdf_without_text_exits_with_four() {
    let dir = TempDir::new("scan");
    let path = dir.path().join("scan.pdf");
    std::fs::write(&path, minimal_pdf("")).unwrap();

    let output = deepdoc([path.as_os_str()]);
    assert_eq!(output.status.code(), Some(4));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("deepocr"),
        "the message should point at OCR"
    );
}

/// A one-page PDF drawing the given content stream.
fn minimal_pdf(content: &str) -> Vec<u8> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
           /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        )
        .into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];

    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        out.extend_from_slice(object);
        out.extend_from_slice(b"\nendobj\n");
    }

    // The cross-reference table records where every object starts.
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );

    out
}

/// A stored (uncompressed) ZIP, written by hand so the test needs no ZIP crate.
fn minimal_zip(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut directory = Vec::new();

    for (name, content) in entries {
        let offset = out.len() as u32;
        let crc = crc32(content.as_bytes());
        let size = content.len() as u32;

        // Local file header, stored (method 0).
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(content.as_bytes());

        directory.extend_from_slice(b"PK\x01\x02");
        directory.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        directory.extend_from_slice(&crc.to_le_bytes());
        directory.extend_from_slice(&size.to_le_bytes());
        directory.extend_from_slice(&size.to_le_bytes());
        directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
        directory.extend_from_slice(&[0; 8]);
        directory.extend_from_slice(&0u32.to_le_bytes());
        directory.extend_from_slice(&offset.to_le_bytes());
        directory.extend_from_slice(name.as_bytes());
    }

    let directory_offset = out.len() as u32;
    let directory_size = directory.len() as u32;
    out.extend_from_slice(&directory);

    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&directory_size.to_le_bytes());
    out.extend_from_slice(&directory_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());

    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[test]
fn text_format_strips_markup() {
    let dir = TempDir::new("plain");
    let file = dir.write("page.html", "<h1>Title</h1><p>a <em>word</em></p>");

    let output = deepdoc([file.as_os_str(), "--format".as_ref(), "text".as_ref()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Title\n\na word\n");
}

#[test]
fn metadata_flag_emits_front_matter() {
    let dir = TempDir::new("frontmatter");
    let file = dir.write(
        "page.html",
        "<html><head><title>A page</title></head><body><p>hi</p></body></html>",
    );

    let output = deepdoc([file.as_os_str(), "--metadata".as_ref()]);
    assert_eq!(output.status.code(), Some(0));
    let rendered = String::from_utf8_lossy(&output.stdout);
    assert!(
        rendered.starts_with("---\ntitle: \"A page\"\n"),
        "unexpected front matter:\n{rendered}"
    );
}

#[test]
fn output_flag_writes_a_file() {
    let dir = TempDir::new("output");
    let file = dir.write("notes.txt", "hello");
    let target = dir.path().join("out.md");

    let output = deepdoc([file.as_os_str(), "-o".as_ref(), target.as_os_str()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello\n");
}

#[test]
fn batch_keeps_documents_that_share_a_stem_apart() {
    let dir = TempDir::new("collide");
    dir.write("report.txt", "from txt");
    dir.write("report.csv", "a,b\n1,2\n");
    dir.write("notes.txt", "unique stem");
    let out = dir.path().join("out");

    let output = deepdoc([
        dir.path().as_os_str(),
        "--recursive".as_ref(),
        "-o".as_ref(),
        out.as_os_str(),
    ]);
    assert_eq!(output.status.code(), Some(0));

    // The colliding pair keeps its original extension; the unique stem does not.
    assert_eq!(
        std::fs::read_to_string(out.join("report.txt.md")).unwrap(),
        "from txt\n"
    );
    assert!(out.join("report.csv.md").is_file());
    assert!(out.join("notes.md").is_file());
    assert!(!out.join("report.md").exists());
}

#[test]
fn batch_mirrors_the_tree_and_reports_a_summary() {
    let dir = TempDir::new("mirror");
    std::fs::create_dir_all(dir.path().join("sub/deep")).unwrap();
    dir.write("top.txt", "top level");
    dir.write("sub/middle.txt", "one down");
    dir.write("sub/deep/leaf.md", "# Leaf\n");
    // Neither of these is a document; a walked folder holds what it holds.
    dir.write("mystery.bin", "\u{0}\u{1}\u{2}not a document");
    dir.write("blank.txt", "   \n");
    let out = dir.path().join("out");

    let output = deepdoc([
        dir.path().as_os_str(),
        "--recursive".as_ref(),
        "-o".as_ref(),
        out.as_os_str(),
    ]);

    // Skips do not fail the batch, and they do not stop the files after them.
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(out.join("top.md")).unwrap(),
        "top level\n"
    );
    assert_eq!(
        std::fs::read_to_string(out.join("sub/middle.md")).unwrap(),
        "one down\n"
    );
    assert_eq!(
        std::fs::read_to_string(out.join("sub/deep/leaf.md")).unwrap(),
        "# Leaf\n"
    );

    let log = String::from_utf8_lossy(&output.stderr);
    assert!(
        log.contains("✓ 3 extracted, 2 skipped"),
        "unexpected log:\n{log}"
    );
}

#[test]
fn a_batch_of_nothing_but_skips_reports_the_reason() {
    let dir = TempDir::new("allskipped");
    // Walk order is sorted, so the scan is the first problem the run meets.
    dir.write("a-scan.txt", "  \n");
    dir.write("b-mystery.bin", "\u{0}\u{1}\u{2}junk");
    let out = dir.path().join("out");

    let output = deepdoc([
        dir.path().as_os_str(),
        "--recursive".as_ref(),
        "-o".as_ref(),
        out.as_os_str(),
    ]);

    // Nothing came out, so the run does not claim success: the first skip
    // decides the code (no extractable text).
    assert_eq!(output.status.code(), Some(4));
    let log = String::from_utf8_lossy(&output.stderr);
    assert!(
        log.contains("0 extracted, 2 skipped"),
        "unexpected log:\n{log}"
    );
    assert!(!log.contains('✓'), "an empty run should not be ticked off");
}

/// Same input, same bytes — no thread order, no hash order leaking out.
#[test]
fn a_batch_is_deterministic() {
    let dir = TempDir::new("determinism");
    for index in 0..12 {
        dir.write(
            &format!("doc{index}.md"),
            &format!("# Doc {index}\n\ntext\n"),
        );
        dir.write(&format!("data{index}.csv"), "part,qty\nbolt,4\n");
        dir.write(&format!("page{index}.html"), "<h1>T</h1><p>a <b>b</b></p>");
        // An office document too: those parse through style and relationship
        // tables, where an iteration order could escape into the output.
        let document = r#"<w:document xmlns:w="w"><w:body>
              <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Report</w:t></w:r></w:p>
              <w:p><w:r><w:t>Body text.</w:t></w:r></w:p>
            </w:body></w:document>"#;
        std::fs::write(
            dir.path().join(format!("report{index}.docx")),
            minimal_zip(&[("word/document.xml", document)]),
        )
        .unwrap();
    }

    let first = deepdoc([dir.path().as_os_str(), "--recursive".as_ref()]);
    let second = deepdoc([dir.path().as_os_str(), "--recursive".as_ref()]);
    assert_eq!(first.status.code(), Some(0));
    assert_eq!(first.stdout, second.stdout, "stdout differs between runs");
    assert_eq!(first.stderr, second.stderr, "the log differs between runs");
}

#[test]
fn a_file_the_user_named_is_not_silently_skipped() {
    let dir = TempDir::new("named");
    let good = dir.write("notes.txt", "hello");
    let bad = dir.write("mystery.bin", "\u{0}\u{1}\u{2}junk");

    // Walked folders may hold anything, but a file on the command line was
    // asked for by name: not being able to read it is a failure.
    let output = deepdoc([good.as_os_str(), bad.as_os_str()]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello\n");
}

#[test]
fn a_missing_input_does_not_cost_the_other_files() {
    let dir = TempDir::new("missing");
    let good = dir.write("notes.txt", "hello");
    let missing = dir.path().join("nope.txt");

    let output = deepdoc([missing.as_os_str(), good.as_os_str()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello\n");
}

#[test]
fn chunk_json_carries_the_documented_schema() {
    let dir = TempDir::new("chunkjson");
    let file = dir.write(
        "handbook.md",
        "# Handbook\n\n## Onboarding\n\nYour first day is mostly paperwork.\n\n\
         ## Payroll\n\nSalaries are paid monthly.\n",
    );

    let output = deepdoc([
        file.as_os_str(),
        "--chunk".as_ref(),
        "40".as_ref(),
        "--format".as_ref(),
        "json".as_ref(),
    ]);
    assert_eq!(output.status.code(), Some(0));

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("chunk output must be JSON");
    assert_eq!(json["source"], file.display().to_string());
    // The document's own metadata keeps the name it has everywhere else.
    assert_eq!(json["meta"]["title"], "Handbook");

    let chunks = json["chunks"].as_array().expect("chunks must be an array");
    assert!(
        chunks.len() > 1,
        "40 characters should not fit in one chunk"
    );
    for chunk in chunks {
        assert!(chunk["text"].is_string());
        assert!(chunk["heading_path"].is_array());
        assert_eq!(chunk["source"], file.display().to_string());
        assert_eq!(chunk["byte_range"].as_array().unwrap().len(), 2);
    }
    assert_eq!(
        chunks.last().unwrap()["heading_path"],
        serde_json::json!(["Handbook", "Payroll"])
    );
}

#[test]
fn chunk_byte_ranges_point_into_the_markdown_output() {
    let dir = TempDir::new("chunkranges");
    let file = dir.write(
        "handbook.md",
        "# Handbook\n\nWelcome aboard, and read this whole page before you start.\n\n\
         ## Payroll\n\nSalaries are paid monthly, on the last working day.\n",
    );

    let markdown = deepdoc([file.as_os_str()]);
    let markdown = String::from_utf8(markdown.stdout).unwrap();

    let output = deepdoc([
        file.as_os_str(),
        "--chunk".as_ref(),
        "60".as_ref(),
        "--format".as_ref(),
        "json".as_ref(),
    ]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    for chunk in json["chunks"].as_array().unwrap() {
        let start = chunk["byte_range"][0].as_u64().unwrap() as usize;
        let end = chunk["byte_range"][1].as_u64().unwrap() as usize;
        assert_eq!(&markdown[start..end], chunk["text"].as_str().unwrap());
    }
}

#[test]
fn chunked_markdown_shows_where_the_cuts_are() {
    let dir = TempDir::new("chunkmd");
    let file = dir.write(
        "handbook.md",
        "# Handbook\n\nWelcome aboard.\n\n## Payroll\n\nSalaries are paid monthly.\n",
    );

    let output = deepdoc([file.as_os_str(), "--chunk".as_ref(), "45".as_ref()]);
    assert_eq!(output.status.code(), Some(0));

    let rendered = String::from_utf8_lossy(&output.stdout);
    assert!(
        rendered.contains("<!-- chunk 1/2 | Handbook | bytes"),
        "unexpected output:\n{rendered}"
    );
    assert!(
        rendered.contains("<!-- chunk 2/2 | Handbook > Payroll | bytes"),
        "unexpected output:\n{rendered}"
    );
    assert!(rendered.contains("Salaries are paid monthly."));
}

#[test]
fn directory_without_recursive_is_rejected() {
    let dir = TempDir::new("dir");
    dir.write("notes.txt", "hello");

    let output = deepdoc([dir.path().as_os_str()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--recursive"));
}

#[test]
fn invalid_page_range_is_a_usage_error() {
    let dir = TempDir::new("pages");
    let file = dir.write("notes.txt", "hello");

    let output = deepdoc([file.as_os_str(), "--pages".as_ref(), "10-2".as_ref()]);
    assert_eq!(output.status.code(), Some(2));
}
