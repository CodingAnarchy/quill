//! Word (`.docx`) export in standard manuscript format (#191). A `.docx` is a zip
//! of XML parts; the XML here is hand-written so we control the manuscript
//! formatting precisely — Times New Roman 12pt, double-spaced body with
//! first-line indents, each chapter on a fresh page with its title, epigraphs set
//! apart, and Markdown `*`/`**` carried through as italic/bold runs. Consumes the
//! shared [`Document`] from #190.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use super::{Document, Item};
use crate::frontmatter::Epigraph;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#;

const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

const DOC_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;

/// Document defaults: Times New Roman 12pt, double-spaced — the manuscript-format
/// baseline that body paragraphs inherit and headings/epigraphs override.
const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman"/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:line="480" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults></w:styles>"#;

/// Compile a [`Document`] to the bytes of a `.docx` file.
pub fn to_docx(doc: &Document) -> std::io::Result<Vec<u8>> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default();
    let parts = [
        ("[Content_Types].xml", CONTENT_TYPES.to_string()),
        ("_rels/.rels", RELS.to_string()),
        ("word/_rels/document.xml.rels", DOC_RELS.to_string()),
        ("word/styles.xml", STYLES.to_string()),
        ("word/document.xml", document_xml(doc)),
    ];
    for (name, content) in parts {
        zip.start_file(name, opts).map_err(std::io::Error::other)?;
        zip.write_all(content.as_bytes())?;
    }
    Ok(zip.finish().map_err(std::io::Error::other)?.into_inner())
}

/// The `word/document.xml` body: the title block, then each part/chapter.
fn document_xml(doc: &Document) -> String {
    let mut body = String::new();

    // Title page: pushed down the page, centered.
    if let Some(series) = &doc.series {
        body.push_str(&para(r#"<w:jc w:val="center"/>"#, run(series, false, false, None)));
    }
    body.push_str(&para(
        r#"<w:spacing w:before="2400"/><w:jc w:val="center"/>"#,
        run(&doc.title, true, false, Some(36)),
    ));
    if let Some(subtitle) = &doc.subtitle {
        body.push_str(&para(r#"<w:jc w:val="center"/>"#, run(subtitle, false, true, None)));
    }
    if let Some(author) = &doc.author {
        body.push_str(&para(r#"<w:jc w:val="center"/>"#, run(&format!("by {author}"), false, false, None)));
    }
    if let Some(epigraph) = &doc.epigraph {
        body.push_str(&epigraph_paras(epigraph));
    }

    for item in &doc.items {
        match item {
            Item::Part(name) => body.push_str(&para(
                r#"<w:pageBreakBefore/><w:spacing w:before="240" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
                run(name, true, false, Some(32)),
            )),
            Item::Chapter { title, epigraph, body: prose } => {
                body.push_str(&para(
                    r#"<w:pageBreakBefore/><w:spacing w:before="240" w:after="240" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
                    run(title, true, false, Some(28)),
                ));
                if let Some(epigraph) = epigraph {
                    body.push_str(&epigraph_paras(epigraph));
                }
                for block in prose.split("\n\n") {
                    if let Some(p) = prose_paragraph(block) {
                        body.push_str(&p);
                    }
                }
            }
        }
    }

    // Letter page, 1" margins.
    let sect = r#"<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr>"#;
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
            "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
            "<w:body>{body}{sect}</w:body></w:document>"
        ),
        body = body,
        sect = sect,
    )
}

/// One body block → a paragraph, or `None` for an empty block. A heading line
/// (`#`…) becomes a centered bold line; otherwise a double-spaced, first-line-
/// indented paragraph with `*`/`**` emphasis as italic/bold runs.
fn prose_paragraph(block: &str) -> Option<String> {
    let text = block.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(heading) = text.strip_prefix('#') {
        let heading = heading.trim_start_matches('#').trim();
        return Some(para(
            r#"<w:spacing w:before="120" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
            run(heading, true, false, None),
        ));
    }
    // Soft-wrapped lines join into one paragraph.
    let joined = text.split('\n').map(str::trim).collect::<Vec<_>>().join(" ");
    let runs: String = inline_runs(&joined)
        .into_iter()
        .map(|(t, b, i)| run(&t, b, i, None))
        .collect();
    Some(para(r#"<w:ind w:firstLine="720"/>"#, runs))
}

/// An epigraph as set-apart italic paragraphs (single-spaced, indented), the
/// attribution on its own line.
fn epigraph_paras(epigraph: &Epigraph) -> String {
    let ppr = r#"<w:ind w:left="1440"/><w:spacing w:line="240" w:lineRule="auto"/>"#;
    let mut out = String::new();
    for line in epigraph.text.lines() {
        out.push_str(&para(ppr, run(line, false, true, None)));
    }
    if let Some(attribution) = &epigraph.attribution {
        out.push_str(&para(ppr, run(&format!("— {attribution}"), false, true, None)));
    }
    out
}

/// A `<w:p>` with the given paragraph properties (omitted when empty) and runs.
fn para(ppr: &str, runs: String) -> String {
    let ppr = if ppr.is_empty() { String::new() } else { format!("<w:pPr>{ppr}</w:pPr>") };
    format!("<w:p>{ppr}{runs}</w:p>")
}

/// A `<w:r>` run with bold/italic and an optional font size (half-points).
fn run(text: &str, bold: bool, italic: bool, size: Option<u32>) -> String {
    let mut rpr = String::new();
    if bold {
        rpr.push_str("<w:b/>");
    }
    if italic {
        rpr.push_str("<w:i/>");
    }
    if let Some(sz) = size {
        rpr.push_str(&format!("<w:sz w:val=\"{sz}\"/>"));
    }
    let rpr = if rpr.is_empty() { String::new() } else { format!("<w:rPr>{rpr}</w:rPr>") };
    format!("<w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r>", xml_escape(text))
}

/// Split a line into runs by `*` (italic) and `**` (bold) markers — Quill's
/// emphasis convention. Other text (including `_`) is taken literally.
fn inline_runs(text: &str) -> Vec<(String, bool, bool)> {
    let mut runs = Vec::new();
    let mut cur = String::new();
    let (mut bold, mut italic) = (false, false);
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' {
            if chars.get(i + 1) == Some(&'*') {
                push_run(&mut runs, &mut cur, bold, italic);
                bold = !bold;
                i += 2;
                continue;
            }
            push_run(&mut runs, &mut cur, bold, italic);
            italic = !italic;
            i += 1;
            continue;
        }
        cur.push(chars[i]);
        i += 1;
    }
    push_run(&mut runs, &mut cur, bold, italic);
    // Unbalanced markers mean the `*`s weren't emphasis (a stray `*` in prose,
    // e.g. "5 * 3"). Don't drop them and over-italicize — emit the line verbatim.
    if bold || italic {
        return vec![(text.to_string(), false, false)];
    }
    runs
}

fn push_run(runs: &mut Vec<(String, bool, bool)>, cur: &mut String, bold: bool, italic: bool) {
    if !cur.is_empty() {
        runs.push((std::mem::take(cur), bold, italic));
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn epigraph(text: &str, by: Option<&str>) -> Epigraph {
        Epigraph { text: text.to_string(), attribution: by.map(str::to_string) }
    }

    fn doc() -> Document {
        Document {
            title: "Red <State> Earth".into(),
            subtitle: None,
            author: Some("A. Writer".into()),
            series: None,
            epigraph: Some(epigraph("In the beginning.", Some("Anon"))),
            items: vec![
                Item::Part("Part One".into()),
                Item::Chapter {
                    title: "The Find".into(),
                    epigraph: None,
                    body: "The locket *gleamed* and felt **heavy**.\n\n## A scene\n\nMore.".into(),
                },
            ],
        }
    }

    /// Read one part's text out of the produced .docx zip.
    fn part(bytes: &[u8], name: &str) -> String {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
        let mut file = zip.by_name(name).expect("part present");
        let mut s = String::new();
        file.read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn produces_a_zip_with_the_required_parts() {
        let bytes = to_docx(&doc()).unwrap();
        assert_eq!(&bytes[..2], b"PK", "a zip container");
        for name in ["[Content_Types].xml", "_rels/.rels", "word/styles.xml", "word/document.xml"] {
            assert!(!part(&bytes, name).is_empty(), "{name} present");
        }
    }

    #[test]
    fn document_carries_structure_and_emphasis() {
        let xml = part(&to_docx(&doc()).unwrap(), "word/document.xml");
        assert!(xml.contains("by A. Writer"));
        assert!(xml.contains("Part One") && xml.contains("The Find"), "headings present");
        // Each chapter starts on a new page.
        assert!(xml.contains("<w:pageBreakBefore/>"), "chapter page break");
        // Markdown emphasis became real runs, not literal markup.
        assert!(xml.contains("<w:b/>") && xml.contains("<w:i/>"), "bold + italic runs");
        assert!(!xml.contains('*'), "no literal asterisks: {xml}");
        // XML special characters in the title are escaped.
        assert!(xml.contains("Red &lt;State&gt; Earth"), "title escaped");
    }

    #[test]
    fn parses_nested_and_plain_emphasis() {
        assert_eq!(inline_runs("plain"), vec![("plain".into(), false, false)]);
        assert_eq!(
            inline_runs("a *b* c"),
            vec![("a ".into(), false, false), ("b".into(), false, true), (" c".into(), false, false)]
        );
        assert_eq!(inline_runs("**bold**"), vec![("bold".into(), true, false)]);
        // Underscores are literal (not emphasis), so snake_case survives.
        assert_eq!(inline_runs("snake_case"), vec![("snake_case".into(), false, false)]);
        // A stray/unbalanced `*` is kept literal, not turned into runaway italics.
        assert_eq!(inline_runs("5 * 3 = 15"), vec![("5 * 3 = 15".into(), false, false)]);
    }
}
