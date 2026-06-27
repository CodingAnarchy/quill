//! Word (`.docx`) export in standard manuscript format (#191). A `.docx` is a zip
//! of XML parts; the XML here is hand-written so we control the manuscript
//! formatting precisely — Times New Roman 12pt, double-spaced body with
//! first-line indents, each chapter on a fresh page with its title, epigraphs set
//! apart, and Markdown `*`/`**` carried through as italic/bold runs. Consumes the
//! shared [`Document`] from #190.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use super::{classify_block, inline_runs, xml_escape, Block, Document, Prose, SCENE_BREAK};
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
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman"/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:line="480" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>"#;

/// A `Normal` base plus the canonical built-in "heading 1" paragraph style.
/// Vellum starts a new chapter at each Heading 1, so the [`Profile::Vellum`]
/// writer tags chapter/part titles with it; the full canonical form (the
/// `Heading1` id, the "heading 1" name, `basedOn`/`qFormat`/`outlineLvl`) is what
/// makes word processors recognize it as *the* Heading 1 rather than a lookalike.
const VELLUM_STYLES: &str = r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:uiPriority w:val="9"/><w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="240" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style>"#;

/// Which export shape a `.docx` takes.
#[derive(Clone, Copy, PartialEq)]
pub enum Profile {
    /// Standard manuscript format (#191): a title page, each chapter on a fresh
    /// page under a centered bold title, double-spaced Times New Roman body.
    Manuscript,
    /// Vellum import (#193): chapter and part titles as Heading 1 (Vellum splits
    /// chapters on it), no manuscript title page — Vellum applies its own design.
    Vellum,
}

/// Compile a [`Document`] to the bytes of a `.docx` file in the given [`Profile`].
pub fn to_docx(doc: &Document, profile: Profile) -> std::io::Result<Vec<u8>> {
    build_docx(document_xml(doc, profile), profile)
}

/// Compile a whole series (one book per [`Document`], in order) to one `.docx`
/// (#332): a series title page (manuscript profile), then each book under its own
/// title, page-broken from the last. Single book → the same body as [`to_docx`].
pub fn to_docx_series(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
    profile: Profile,
) -> std::io::Result<Vec<u8>> {
    let mut body = String::new();
    // Series title page (manuscript only — Vellum splits on Heading 1, so stray
    // leading text would import as junk).
    if profile == Profile::Manuscript {
        if let Some(series) = series {
            body.push_str(&para(
                r#"<w:spacing w:before="2400"/><w:jc w:val="center"/>"#,
                run(series, true, false, Some(40)),
            ));
        }
        if let Some(author) = author {
            body.push_str(&para(
                r#"<w:jc w:val="center"/>"#,
                run(&format!("by {author}"), false, false, None),
            ));
        }
    }
    for book in books {
        body.push_str(&book_heading(&book.title, profile));
        body.push_str(&book_body(book, profile));
    }
    build_docx(wrap_document(&body), profile)
}

/// Zip the four-part `.docx` package around a built `word/document.xml`.
fn build_docx(document_xml: String, profile: Profile) -> std::io::Result<Vec<u8>> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default();
    // Vellum needs a Heading 1 style to recognize chapter breaks; the manuscript
    // profile adds nothing to the base styles.
    let extra = match profile {
        Profile::Manuscript => "",
        Profile::Vellum => VELLUM_STYLES,
    };
    let styles = format!("{STYLES}{extra}</w:styles>");
    let parts = [
        ("[Content_Types].xml", CONTENT_TYPES.to_string()),
        ("_rels/.rels", RELS.to_string()),
        ("word/_rels/document.xml.rels", DOC_RELS.to_string()),
        ("word/styles.xml", styles),
        ("word/document.xml", document_xml),
    ];
    for (name, content) in parts {
        zip.start_file(name, opts).map_err(std::io::Error::other)?;
        zip.write_all(content.as_bytes())?;
    }
    Ok(zip.finish().map_err(std::io::Error::other)?.into_inner())
}

/// The `word/document.xml` for one book: the title block (manuscript only), then
/// its parts/chapters, wrapped in the document shell.
fn document_xml(doc: &Document, profile: Profile) -> String {
    let mut body = String::new();
    if profile == Profile::Manuscript {
        body.push_str(&title_page(doc));
    }
    body.push_str(&book_body(doc, profile));
    wrap_document(&body)
}

/// A book's title page (manuscript profile): series line, big bold title,
/// subtitle, author, and epigraph — each centered.
fn title_page(doc: &Document) -> String {
    let mut out = String::new();
    if let Some(series) = &doc.series {
        out.push_str(&para(
            r#"<w:jc w:val="center"/>"#,
            run(series, false, false, None),
        ));
    }
    out.push_str(&para(
        r#"<w:spacing w:before="2400"/><w:jc w:val="center"/>"#,
        run(&doc.title, true, false, Some(36)),
    ));
    if let Some(subtitle) = &doc.subtitle {
        out.push_str(&para(
            r#"<w:jc w:val="center"/>"#,
            run(subtitle, false, true, None),
        ));
    }
    if let Some(author) = &doc.author {
        out.push_str(&para(
            r#"<w:jc w:val="center"/>"#,
            run(&format!("by {author}"), false, false, None),
        ));
    }
    if let Some(epigraph) = &doc.epigraph {
        out.push_str(&epigraph_paras(epigraph));
    }
    out
}

/// A book's part/chapter blocks (no title page) — shared by the single-book and
/// series documents.
fn book_body(doc: &Document, profile: Profile) -> String {
    let mut body = String::new();
    for block in doc.blocks() {
        match block {
            Block::Part(name) => body.push_str(&heading(name, profile)),
            Block::Chapter(chapter) => {
                body.push_str(&heading(&chapter.title, profile));
                if let Some(epigraph) = &chapter.epigraph {
                    body.push_str(&epigraph_paras(epigraph));
                }
                for prose_block in chapter.body.split("\n\n") {
                    if let Some(p) = prose_paragraph(prose_block) {
                        body.push_str(&p);
                    }
                }
            }
        }
    }
    body
}

/// A series book title: a page-broken centered line, larger than a chapter
/// heading (manuscript), or a Heading 1 (Vellum, where it reads as a break).
fn book_heading(title: &str, profile: Profile) -> String {
    match profile {
        Profile::Manuscript => para(
            r#"<w:pageBreakBefore/><w:spacing w:before="360" w:after="360" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
            run(title, true, false, Some(32)),
        ),
        Profile::Vellum => para(
            r#"<w:pStyle w:val="Heading1"/>"#,
            run(title, false, false, None),
        ),
    }
}

/// Wrap a built body in the `word/document.xml` shell (letter page, 1" margins).
fn wrap_document(body: &str) -> String {
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

/// A part or chapter title. The manuscript profile opens a fresh page under a
/// centered bold line; the Vellum profile tags it Heading 1 so Vellum splits
/// chapters there and applies its own styling.
fn heading(title: &str, profile: Profile) -> String {
    match profile {
        Profile::Manuscript => para(
            r#"<w:pageBreakBefore/><w:spacing w:before="240" w:after="240" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
            run(title, true, false, Some(28)),
        ),
        Profile::Vellum => para(
            r#"<w:pStyle w:val="Heading1"/>"#,
            run(title, false, false, None),
        ),
    }
}

/// One body block → a paragraph, or `None` for an empty block. A scene break
/// becomes a centered divider; a heading line (`#`…) a centered bold line;
/// otherwise a double-spaced, first-line-indented paragraph with `*`/`**`
/// emphasis as italic/bold runs.
fn prose_paragraph(block: &str) -> Option<String> {
    Some(match classify_block(block)? {
        Prose::SceneBreak => para(
            r#"<w:spacing w:before="120" w:after="120" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
            run(SCENE_BREAK, false, false, None),
        ),
        Prose::Heading(heading) => para(
            r#"<w:spacing w:before="120" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
            run(&heading, true, false, None),
        ),
        Prose::Paragraph(text) => {
            let runs: String = inline_runs(&text)
                .into_iter()
                .map(|(t, b, i)| run(&t, b, i, None))
                .collect();
            para(r#"<w:ind w:firstLine="720"/>"#, runs)
        }
    })
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
        out.push_str(&para(
            ppr,
            run(&format!("— {attribution}"), false, true, None),
        ));
    }
    out
}

/// A `<w:p>` with the given paragraph properties (omitted when empty) and runs.
fn para(ppr: &str, runs: String) -> String {
    let ppr = if ppr.is_empty() {
        String::new()
    } else {
        format!("<w:pPr>{ppr}</w:pPr>")
    };
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
    let rpr = if rpr.is_empty() {
        String::new()
    } else {
        format!("<w:rPr>{rpr}</w:rPr>")
    };
    format!(
        "<w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
        xml_escape(text)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn epigraph(text: &str, by: Option<&str>) -> Epigraph {
        Epigraph {
            text: text.to_string(),
            attribution: by.map(str::to_string),
        }
    }

    fn doc() -> Document {
        Document {
            title: "Red <State> Earth".into(),
            subtitle: None,
            author: Some("A. Writer".into()),
            series: None,
            epigraph: Some(epigraph("In the beginning.", Some("Anon"))),
            front: vec![],
            parts: vec![crate::export::Part {
                name: Some("Part One".into()),
                chapters: vec![crate::export::Chapter {
                    title: "The Find".into(),
                    epigraph: None,
                    body: "The locket *gleamed* and felt **heavy**.\n\n## A scene\n\nMore.".into(),
                }],
            }],
            back: vec![],
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
    fn smart_typography_glyphs_pass_through_to_docx() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![crate::export::Part {
                name: None,
                chapters: vec![crate::export::Chapter {
                    title: "C".into(),
                    epigraph: None,
                    body: "He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}".into(),
                }],
            }],
            back: vec![],
        };
        let bytes = to_docx(&d, Profile::Manuscript).unwrap();
        let xml = part(&bytes, "word/document.xml");
        assert!(
            xml.contains("He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}"),
            "curly glyphs reach the docx body"
        );
    }

    #[test]
    fn series_docx_carries_every_book_in_order() {
        let book = |title: &str| crate::export::Document {
            title: title.into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![crate::export::Part {
                name: None,
                chapters: vec![crate::export::Chapter {
                    title: "Ch".into(),
                    epigraph: None,
                    body: "prose".into(),
                }],
            }],
            back: vec![],
        };
        let bytes = to_docx_series(
            &[book("Book One"), book("Book Two")],
            Some("The Saga"),
            Some("A. Writer"),
            Profile::Manuscript,
        )
        .unwrap();
        let xml = part(&bytes, "word/document.xml");
        assert!(
            xml.contains("The Saga") && xml.contains("by A. Writer"),
            "series title page"
        );
        let one = xml.find("Book One").expect("book one");
        let two = xml.find("Book Two").expect("book two");
        assert!(one < two, "books in order");
    }

    #[test]
    fn produces_a_zip_with_the_required_parts() {
        let bytes = to_docx(&doc(), Profile::Manuscript).unwrap();
        assert_eq!(&bytes[..2], b"PK", "a zip container");
        for name in [
            "[Content_Types].xml",
            "_rels/.rels",
            "word/styles.xml",
            "word/document.xml",
        ] {
            assert!(!part(&bytes, name).is_empty(), "{name} present");
        }
        // styles.xml is closed once, whichever profile.
        assert_eq!(
            part(&bytes, "word/styles.xml")
                .matches("</w:styles>")
                .count(),
            1
        );
    }

    #[test]
    fn document_carries_structure_and_emphasis() {
        let xml = part(
            &to_docx(&doc(), Profile::Manuscript).unwrap(),
            "word/document.xml",
        );
        assert!(xml.contains("by A. Writer"));
        assert!(
            xml.contains("Part One") && xml.contains("The Find"),
            "headings present"
        );
        // Each chapter starts on a new page.
        assert!(xml.contains("<w:pageBreakBefore/>"), "chapter page break");
        // Markdown emphasis became real runs, not literal markup.
        assert!(
            xml.contains("<w:b/>") && xml.contains("<w:i/>"),
            "bold + italic runs"
        );
        assert!(!xml.contains('*'), "no literal asterisks: {xml}");
        // XML special characters in the title are escaped.
        assert!(xml.contains("Red &lt;State&gt; Earth"), "title escaped");
    }

    #[test]
    fn a_scene_break_becomes_a_centered_divider_not_literal_dashes() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![crate::export::Part {
                name: None,
                chapters: vec![crate::export::Chapter {
                    title: "One".into(),
                    epigraph: None,
                    body: "Scene one.\n\n---\n\nScene two.".into(),
                }],
            }],
            back: vec![],
        };
        let xml = part(
            &to_docx(&d, Profile::Manuscript).unwrap(),
            "word/document.xml",
        );
        assert!(!xml.contains("---"), "no literal rule: {xml}");
        // A centered `#` divider sits between the two scenes.
        let one = xml.find("Scene one.").unwrap();
        let hash = xml
            .find(r#"<w:t xml:space="preserve">#</w:t>"#)
            .expect("# divider");
        let two = xml.find("Scene two.").unwrap();
        assert!(one < hash && hash < two, "divider between scenes: {xml}");
    }

    #[test]
    fn vellum_profile_uses_heading1_and_no_title_page() {
        let bytes = to_docx(&doc(), Profile::Vellum).unwrap();
        let styles = part(&bytes, "word/styles.xml");
        // The Heading 1 style Vellum splits chapters on is defined.
        assert!(
            styles.contains(r#"w:styleId="Heading1""#),
            "Heading1 style: {styles}"
        );
        assert!(
            styles.contains(r#"w:val="heading 1""#),
            "built-in heading 1 name"
        );
        let xml = part(&bytes, "word/document.xml");
        // Chapter and part titles are tagged Heading 1, in reading order.
        assert!(
            xml.contains(r#"<w:pStyle w:val="Heading1"/>"#),
            "headings styled: {xml}"
        );
        let part_at = xml.find("Part One").unwrap();
        let chapter_at = xml.find("The Find").unwrap();
        assert!(part_at < chapter_at, "reading order");
        // No manuscript title page (the author sets title/author in Vellum) and no
        // page-break-driven layout (Vellum reflows).
        assert!(!xml.contains("by A. Writer"), "no title page: {xml}");
        assert!(!xml.contains("<w:pageBreakBefore/>"), "no page breaks");
        // Emphasis still carries through as real runs.
        assert!(
            xml.contains("<w:b/>") && xml.contains("<w:i/>"),
            "bold + italic runs"
        );
        assert!(!xml.contains('*'), "no literal asterisks");
    }
}
