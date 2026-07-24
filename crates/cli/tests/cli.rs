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
fn known_but_unimplemented_type_exits_with_five() {
    let dir = TempDir::new("notimpl");
    let file = dir.write("book.pdf", "%PDF-1.7\ntrailer");

    let output = deepdoc([file.as_os_str()]);
    assert_eq!(output.status.code(), Some(5));
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
