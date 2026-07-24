//! End-to-end checks for the office formats: archive → detection → Markdown.
//!
//! The fixtures are built here as real ZIP archives rather than committed as
//! binaries: a `.docx` in the repository is opaque in review and in diffs, and
//! the interesting part — which XML the extractor is fed — stays readable.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use deepdoc_core::extract::{ExtractOpts, extract_path};
use deepdoc_core::render::{RenderOpts, to_markdown};
use deepdoc_core::{Document, Format};
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

/// A scratch directory that removes itself.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let path =
            std::env::temp_dir().join(format!("deepdoc-office-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("cannot create temp dir");
        TempDir(path)
    }

    /// Write an archive to disk under the given file name.
    fn archive(&self, name: &str, entries: &[(&str, &str)]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, zip_bytes(entries)).expect("cannot write archive");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Build a ZIP. A leading `mimetype` entry is stored uncompressed, as ODF and
/// EPUB require — and as the format sniffer relies on.
fn zip_bytes(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));

    for (index, (name, content)) in entries.iter().enumerate() {
        let method = if index == 0 && *name == "mimetype" {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(method),
            )
            .expect("start entry");
        writer.write_all(content.as_bytes()).expect("write entry");
    }

    writer.finish().expect("finish zip").into_inner()
}

fn extract(path: &Path) -> Document {
    extract_path(path, &ExtractOpts::default())
        .unwrap_or_else(|e| panic!("cannot extract {}: {e}", path.display()))
}

fn markdown(path: &Path) -> String {
    to_markdown(&extract(path), &RenderOpts::default())
}

/// Every ODF package carries a manifest; readers check it before anything else.
fn odf_manifest(media_type: &str) -> String {
    format!(
        r#"<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">
             <manifest:file-entry manifest:full-path="/" manifest:media-type="{media_type}"/>
             <manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
           </manifest:manifest>"#
    )
}

const CORE_PROPERTIES: &str = r#"<cp:coreProperties xmlns:cp="cp" xmlns:dc="dc">
    <dc:title>Quarterly Report</dc:title><dc:creator>Ada Lovelace</dc:creator>
  </cp:coreProperties>"#;

// ------------------------------------------------------------------- docx --

const DOCX_DOCUMENT: &str = r#"<w:document xmlns:w="w"><w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Quarterly Report</w:t></w:r></w:p>
    <w:p>
      <w:r><w:t xml:space="preserve">Revenue grew </w:t></w:r>
      <w:r><w:rPr><w:b/></w:rPr><w:t>12%</w:t></w:r>
      <w:r><w:t xml:space="preserve">, driven by </w:t></w:r>
      <w:r><w:rPr><w:i/></w:rPr><w:t>cloud</w:t></w:r>
      <w:r><w:t xml:space="preserve"> and a </w:t></w:r>
      <w:hyperlink r:id="rId1"><w:r><w:t>partner deal</w:t></w:r></w:hyperlink>
      <w:r><w:t>.</w:t></w:r>
    </w:p>
    <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Cloud</w:t></w:r></w:p>
    <w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Storage</w:t></w:r></w:p>
    <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Devices</w:t></w:r></w:p>
    <w:tbl>
      <w:tr><w:tc><w:p><w:r><w:t>Segment</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Q1</w:t></w:r></w:p></w:tc></w:tr>
      <w:tr><w:tc><w:p><w:r><w:t>Cloud</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>4.1</w:t></w:r></w:p></w:tc></w:tr>
    </w:tbl>
  </w:body></w:document>"#;

const DOCX_NUMBERING: &str = r#"<w:numbering xmlns:w="w">
    <w:abstractNum w:abstractNumId="0">
      <w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl>
      <w:lvl w:ilvl="1"><w:numFmt w:val="decimal"/></w:lvl>
    </w:abstractNum>
    <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
  </w:numbering>"#;

fn docx(dir: &TempDir) -> PathBuf {
    dir.archive(
        "report.docx",
        &[
            ("[Content_Types].xml", "<Types/>"),
            ("word/document.xml", DOCX_DOCUMENT),
            ("word/numbering.xml", DOCX_NUMBERING),
            (
                "word/_rels/document.xml.rels",
                r#"<Relationships><Relationship Id="rId1" Target="https://example.com"/></Relationships>"#,
            ),
            ("docProps/core.xml", CORE_PROPERTIES),
            (
                "docProps/app.xml",
                r#"<Properties><Pages>3</Pages><Words>120</Words></Properties>"#,
            ),
        ],
    )
}

#[test]
fn docx_becomes_markdown() {
    let dir = TempDir::new("docx");
    let path = docx(&dir);

    assert_eq!(extract(&path).meta.source_format, Some(Format::Docx));
    assert_eq!(
        markdown(&path),
        "# Quarterly Report\n\n\
         Revenue grew **12%**, driven by *cloud* and a [partner deal](https://example.com).\n\n\
         - Cloud\n\
         \x20 1. Storage\n\
         - Devices\n\n\
         | Segment | Q1  |\n\
         | ------- | --- |\n\
         | Cloud   | 4.1 |\n"
    );
}

#[test]
fn docx_carries_core_properties() {
    let dir = TempDir::new("docx-meta");
    let doc = extract(&docx(&dir));
    assert_eq!(doc.meta.title.as_deref(), Some("Quarterly Report"));
    assert_eq!(doc.meta.author.as_deref(), Some("Ada Lovelace"));
    assert_eq!(doc.meta.page_count, Some(3), "from docProps/app.xml");
}

// ------------------------------------------------------------------- pptx --

fn pptx(dir: &TempDir) -> PathBuf {
    let slide_one = r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree>
        <p:sp><p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
          <p:txBody><a:p><a:r><a:t>Agenda</a:t></a:r></a:p></p:txBody></p:sp>
        <p:sp><p:txBody>
          <a:p><a:r><a:t>Results</a:t></a:r></a:p>
          <a:p><a:pPr lvl="1"/><a:r><a:t>Cloud</a:t></a:r></a:p>
          <a:p><a:r><a:t>Outlook</a:t></a:r></a:p>
        </p:txBody></p:sp>
      </p:spTree></p:cSld></p:sld>"#;
    let slide_two = r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree>
        <p:sp><p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
          <p:txBody><a:p><a:r><a:t>Numbers</a:t></a:r></a:p></p:txBody></p:sp>
      </p:spTree></p:cSld></p:sld>"#;

    dir.archive(
        "deck.pptx",
        &[
            ("[Content_Types].xml", "<Types/>"),
            ("ppt/presentation.xml", "<p:presentation/>"),
            ("ppt/slides/slide1.xml", slide_one),
            ("ppt/slides/slide2.xml", slide_two),
            ("docProps/core.xml", CORE_PROPERTIES),
        ],
    )
}

#[test]
fn pptx_becomes_one_section_per_slide() {
    let dir = TempDir::new("pptx");
    let path = pptx(&dir);

    let doc = extract(&path);
    assert_eq!(doc.meta.source_format, Some(Format::Pptx));
    assert_eq!(doc.meta.page_count, Some(2));

    assert_eq!(
        markdown(&path),
        "## Agenda\n\n\
         - Results\n\
         \x20 - Cloud\n\
         - Outlook\n\n\
         <!-- page 2 -->\n\n\
         ## Numbers\n"
    );
}

// -------------------------------------------------------------- odt / odp --

fn odt(dir: &TempDir) -> PathBuf {
    let content = r#"<office:document-content xmlns:office="o" xmlns:text="t" xmlns:table="tb" xmlns:style="s" xmlns:fo="f">
        <office:automatic-styles>
          <style:style style:name="T1"><style:text-properties fo:font-weight="bold"/></style:style>
        </office:automatic-styles>
        <office:body><office:text>
          <text:h text:outline-level="1">Quarterly Report</text:h>
          <text:p>Revenue grew <text:span text:style-name="T1">12%</text:span>.</text:p>
          <text:list>
            <text:list-item><text:p>Cloud</text:p></text:list-item>
            <text:list-item><text:p>Devices</text:p></text:list-item>
          </text:list>
          <table:table>
            <table:table-row><table:table-cell><text:p>Segment</text:p></table:table-cell><table:table-cell><text:p>Q1</text:p></table:table-cell></table:table-row>
            <table:table-row><table:table-cell><text:p>Cloud</text:p></table:table-cell><table:table-cell><text:p>4.1</text:p></table:table-cell></table:table-row>
          </table:table>
        </office:text></office:body>
      </office:document-content>"#;
    let meta = r#"<office:document-meta xmlns:office="o" xmlns:dc="dc" xmlns:meta="m"><office:meta>
        <dc:title>Quarterly Report</dc:title><dc:creator>Ada Lovelace</dc:creator>
        <meta:document-statistic meta:page-count="2"/>
      </office:meta></office:document-meta>"#;

    let manifest = odf_manifest("application/vnd.oasis.opendocument.text");
    dir.archive(
        "report.odt",
        &[
            ("mimetype", "application/vnd.oasis.opendocument.text"),
            ("META-INF/manifest.xml", &manifest),
            ("content.xml", content),
            ("meta.xml", meta),
        ],
    )
}

#[test]
fn odt_becomes_markdown() {
    let dir = TempDir::new("odt");
    let path = odt(&dir);

    let doc = extract(&path);
    assert_eq!(doc.meta.source_format, Some(Format::Odt));
    assert_eq!(doc.meta.author.as_deref(), Some("Ada Lovelace"));
    assert_eq!(doc.meta.page_count, Some(2), "from meta:document-statistic");

    assert_eq!(
        markdown(&path),
        "# Quarterly Report\n\n\
         Revenue grew **12%**.\n\n\
         - Cloud\n\
         - Devices\n\n\
         | Segment | Q1  |\n\
         | ------- | --- |\n\
         | Cloud   | 4.1 |\n"
    );
}

#[test]
fn odp_becomes_one_section_per_page() {
    let dir = TempDir::new("odp");
    let content = r#"<office:document-content xmlns:office="o" xmlns:draw="d" xmlns:text="t">
        <office:body><office:presentation>
          <draw:page draw:name="Intro">
            <draw:frame><draw:text-box><text:p>Agenda</text:p><text:p>first point</text:p></draw:text-box></draw:frame>
          </draw:page>
          <draw:page draw:name="Numbers">
            <draw:frame><draw:text-box><text:p>Results</text:p></draw:text-box></draw:frame>
          </draw:page>
        </office:presentation></office:body>
      </office:document-content>"#;

    let manifest = odf_manifest("application/vnd.oasis.opendocument.presentation");
    let path = dir.archive(
        "deck.odp",
        &[
            (
                "mimetype",
                "application/vnd.oasis.opendocument.presentation",
            ),
            ("META-INF/manifest.xml", &manifest),
            ("content.xml", content),
        ],
    );

    assert_eq!(extract(&path).meta.source_format, Some(Format::Odp));
    assert_eq!(
        markdown(&path),
        "## Agenda\n\nfirst point\n\n<!-- page 2 -->\n\n## Results\n"
    );
}

// -------------------------------------------------------------------- ods --

#[test]
fn ods_becomes_a_table_per_sheet() {
    let dir = TempDir::new("ods");
    // Written without whitespace between the row and cell elements: that is
    // what a real ODS looks like, and calamine rejects a pretty-printed one.
    let content = concat!(
        r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0""#,
        r#" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0""#,
        r#" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">"#,
        r#"<office:body><office:spreadsheet><table:table table:name="Summary">"#,
        r#"<table:table-row>"#,
        r#"<table:table-cell office:value-type="string"><text:p>part</text:p></table:table-cell>"#,
        r#"<table:table-cell office:value-type="string"><text:p>qty</text:p></table:table-cell>"#,
        r#"</table:table-row><table:table-row>"#,
        r#"<table:table-cell office:value-type="string"><text:p>bolt</text:p></table:table-cell>"#,
        r#"<table:table-cell office:value-type="float" office:value="4"><text:p>4</text:p></table:table-cell>"#,
        r#"</table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#,
    );

    let manifest = odf_manifest("application/vnd.oasis.opendocument.spreadsheet");
    let path = dir.archive(
        "sheet.ods",
        &[
            ("mimetype", "application/vnd.oasis.opendocument.spreadsheet"),
            ("META-INF/manifest.xml", &manifest),
            ("content.xml", content),
        ],
    );

    assert_eq!(extract(&path).meta.source_format, Some(Format::Ods));
    assert_eq!(
        markdown(&path),
        "## Summary\n\n\
         | part | qty |\n\
         | ---- | --- |\n\
         | bolt | 4   |\n"
    );
}

// ------------------------------------------------------------------- xlsx --

#[test]
fn xlsx_becomes_a_table_per_sheet() {
    let dir = TempDir::new("xlsx");

    let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>
        <row r="1"><c r="A1" t="inlineStr"><is><t>part</t></is></c><c r="B1" t="inlineStr"><is><t>qty</t></is></c></row>
        <row r="2"><c r="A2" t="inlineStr"><is><t>bolt</t></is></c><c r="B2"><v>4</v></c></row>
      </sheetData></worksheet>"#;

    let path = dir.archive(
        "sheet.xlsx",
        &[
            (
                "[Content_Types].xml",
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
                     <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
                     <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
                     <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
                   </Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdWb" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Summary" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            ("xl/worksheets/sheet1.xml", sheet),
            ("docProps/core.xml", CORE_PROPERTIES),
        ],
    );

    let doc = extract(&path);
    assert_eq!(doc.meta.source_format, Some(Format::Xlsx));
    assert_eq!(doc.meta.title.as_deref(), Some("Quarterly Report"));
    assert_eq!(
        markdown(&path),
        "## Summary\n\n\
         | part | qty |\n\
         | ---- | --- |\n\
         | bolt | 4   |\n"
    );
}

// -------------------------------------------------------------- detection --

#[test]
fn office_archives_are_detected_by_their_contents_not_their_names() {
    let dir = TempDir::new("detect");

    // Same bytes, misleading extension: detection must still see a docx.
    let bytes = std::fs::read(docx(&dir)).unwrap();
    let renamed = dir.0.join("mystery.bin");
    std::fs::write(&renamed, bytes).unwrap();

    assert_eq!(
        deepdoc_core::detect_path(&renamed).unwrap(),
        Format::Docx,
        "a docx should be recognised from the parts inside it"
    );
}
