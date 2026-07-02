//! Word (`.docx`) export in standard manuscript format (#191). A `.docx` is a zip
//! of XML parts; the XML here is hand-written so we control the manuscript
//! formatting precisely — Times New Roman 12pt, double-spaced body with
//! first-line indents, each chapter on a fresh page with its title, epigraphs set
//! apart, and Markdown `*`/`**` carried through as italic/bold runs. Consumes the
//! shared [`Document`] from #190.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use super::{
    copyright_lines, inline_runs, prose_blocks, render_epigraph, xml_escape, Block, Document,
    Prose, PubMeta, SCENE_BREAK,
};
use crate::frontmatter::Epigraph;

/// The content-types manifest, minus its closing tag. The manuscript profile
/// appends the header override (see [`content_types`]); this head declares the
/// always-present parts.
const CONTENT_TYPES_HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>"#;

/// The header part's content-type override, added only for the manuscript profile.
const CONTENT_TYPES_HEADER: &str = r#"<Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/>"#;

const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

/// The document's relationships, minus its closing tag (always the styles part).
/// The manuscript profile appends the header relationship (see [`doc_rels`]).
const DOC_RELS_HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#;

/// The header relationship, added only for the manuscript profile. `rId2` is
/// referenced by the `<w:headerReference>` in the section properties.
const DOC_RELS_HEADER: &str = r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/>"#;

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
    let header = running_header(doc.author.as_deref(), Some(&doc.title), profile);
    build_docx(document_xml(doc, profile), profile, header)
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
        if let Some(first) = books.first() {
            body.push_str(&copyright_page(&first.publishing));
        }
        body.push_str(&toc_page());
    }
    for book in books {
        body.push_str(&book_heading(&book.title, profile));
        body.push_str(&book_body(book, profile));
    }
    let header = running_header(author, series, profile);
    build_docx(wrap_document(&body, &header), profile, header)
}

/// The running-header content for the manuscript profile, or [`None`] when it
/// carries no header (the Vellum profile, which applies its own).
///
/// Standard manuscript format runs `Author / Title / page #` across the top of
/// every page; the page number is a live `PAGE` field. Either field may be
/// absent (missing author or title), in which case only what's present appears.
type Header = Option<String>;

/// Build the [`Header`] for a profile: the manuscript running header (author,
/// title, and a live page-number field), or [`None`] for Vellum.
fn running_header(author: Option<&str>, title: Option<&str>, profile: Profile) -> Header {
    if profile != Profile::Manuscript {
        return None;
    }
    // Right-aligned `Surname / Title / N` — the page number a live PAGE field so
    // it renumbers when the manuscript reflows.
    let mut runs = String::new();
    if let Some(author) = author {
        runs.push_str(&run(&format!("{author} / "), false, false, None));
    }
    if let Some(title) = title {
        runs.push_str(&run(&format!("{title} / "), false, false, None));
    }
    runs.push_str(&page_field());
    Some(header_xml(&para(r#"<w:jc w:val="right"/>"#, runs)))
}

/// Zip the `.docx` package around a built `word/document.xml`. The manuscript
/// profile adds a `word/header1.xml` part (with its content-type + relationship);
/// Vellum omits it.
fn build_docx(document_xml: String, profile: Profile, header: Header) -> std::io::Result<Vec<u8>> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default();
    // Vellum needs a Heading 1 style to recognize chapter breaks; the manuscript
    // profile adds nothing to the base styles.
    let extra = match profile {
        Profile::Manuscript => "",
        Profile::Vellum => VELLUM_STYLES,
    };
    let styles = format!("{STYLES}{extra}</w:styles>");
    let mut parts = vec![
        ("[Content_Types].xml".to_string(), content_types(&header)),
        ("_rels/.rels".to_string(), RELS.to_string()),
        (
            "word/_rels/document.xml.rels".to_string(),
            doc_rels(&header),
        ),
        ("word/styles.xml".to_string(), styles),
        ("word/document.xml".to_string(), document_xml),
    ];
    if let Some(header) = header {
        parts.push(("word/header1.xml".to_string(), header));
    }
    for (name, content) in parts {
        zip.start_file(name, opts).map_err(std::io::Error::other)?;
        zip.write_all(content.as_bytes())?;
    }
    Ok(zip.finish().map_err(std::io::Error::other)?.into_inner())
}

/// The `[Content_Types].xml`, with the header override when a header is present.
fn content_types(header: &Header) -> String {
    let extra = if header.is_some() {
        CONTENT_TYPES_HEADER
    } else {
        ""
    };
    format!("{CONTENT_TYPES_HEAD}{extra}</Types>")
}

/// The document relationships, with the header relationship when present.
fn doc_rels(header: &Header) -> String {
    let extra = if header.is_some() {
        DOC_RELS_HEADER
    } else {
        ""
    };
    format!("{DOC_RELS_HEAD}{extra}</Relationships>")
}

/// The `word/document.xml` for one book: the title block (manuscript only), then
/// its parts/chapters, wrapped in the document shell.
fn document_xml(doc: &Document, profile: Profile) -> String {
    let header = running_header(doc.author.as_deref(), Some(&doc.title), profile);
    let mut body = String::new();
    if profile == Profile::Manuscript {
        body.push_str(&title_page(doc));
        body.push_str(&copyright_page(&doc.publishing));
        body.push_str(&toc_page());
    }
    body.push_str(&book_body(doc, profile));
    wrap_document(&body, &header)
}

/// The copyright page (#583), when publishing metadata warrants one: the rights
/// line, publisher, publication date, and ISBN, each centered on its own page
/// after the title page (cover → title → copyright → body). Manuscript profile
/// only — Vellum applies its own front matter and splits on Heading 1. Empty when
/// no metadata is set, leaving the output unchanged.
fn copyright_page(publishing: &PubMeta) -> String {
    let lines = copyright_lines(publishing);
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        // The first line opens a fresh page; the rest stack centered beneath it.
        let ppr = if i == 0 {
            r#"<w:pageBreakBefore/><w:spacing w:before="2400"/><w:jc w:val="center"/>"#
        } else {
            r#"<w:jc w:val="center"/>"#
        };
        out.push_str(&para(ppr, run(line, false, false, None)));
    }
    out
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
                for prose in prose_blocks(&chapter.body) {
                    body.push_str(&prose_paragraph(&prose));
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
        // Outline level 1 so a series book title also lands in the TOC.
        Profile::Manuscript => para(
            r#"<w:pageBreakBefore/><w:spacing w:before="360" w:after="360" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/><w:outlineLvl w:val="0"/>"#,
            run(title, true, false, Some(32)),
        ),
        Profile::Vellum => para(
            r#"<w:pStyle w:val="Heading1"/>"#,
            run(title, false, false, None),
        ),
    }
}

/// Wrap a built body in the `word/document.xml` shell (letter page, 1" margins).
/// When a [`Header`] is present (manuscript profile), the section references the
/// `word/header1.xml` part via `rId2` so the running header shows on every page.
fn wrap_document(body: &str, header: &Header) -> String {
    let header_ref = if header.is_some() {
        r#"<w:headerReference w:type="default" r:id="rId2"/>"#
    } else {
        ""
    };
    // The `r` namespace carries the header relationship id; declare it only when
    // it is used so the document stays minimal for the Vellum profile.
    let rns = if header.is_some() {
        r#" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#
    } else {
        ""
    };
    let sect = format!(
        r#"<w:sectPr>{header_ref}<w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720"/></w:sectPr>"#,
    );
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
            "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"{rns}>",
            "<w:body>{body}{sect}</w:body></w:document>"
        ),
        rns = rns,
        body = body,
        sect = sect,
    )
}

/// A live `PAGE` field: the current page number, computed by the word processor
/// and updated as the document reflows. Rendered as three runs — a field-begin,
/// the `PAGE` instruction, and a field-end — with a `1` placeholder so a viewer
/// that doesn't recalculate still shows a number.
fn page_field() -> String {
    concat!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
        r#"<w:r><w:t>1</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
    )
    .to_string()
}

/// A `Contents`-titled page carrying a live `TOC` field (manuscript profile). The
/// `\o "1-1"` switch collects heading-1 outline levels — the chapter/part titles
/// — and `\h` makes the entries clickable. A placeholder line stands in until the
/// reader updates the field (Word prompts on open; LibreOffice recalculates).
fn toc_page() -> String {
    let mut out = para(
        r#"<w:pageBreakBefore/><w:spacing w:before="240" w:after="240" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
        run("Contents", true, false, Some(28)),
    );
    let field = concat!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> TOC \o "1-1" \h </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
        r#"<w:r><w:t>Right-click to update the table of contents.</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
    );
    out.push_str(&format!(
        "<w:p><w:pPr><w:spacing w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>{field}</w:p>",
    ));
    out
}

/// The `word/header1.xml` part wrapping one header paragraph. Its own `<w:hdr>`
/// root with the wordprocessing namespace, as a header part requires.
fn header_xml(paragraph: &str) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
            "<w:hdr xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
            "{paragraph}</w:hdr>"
        ),
        paragraph = paragraph,
    )
}

/// A part or chapter title. The manuscript profile opens a fresh page under a
/// centered bold line; the Vellum profile tags it Heading 1 so Vellum splits
/// chapters there and applies its own styling.
fn heading(title: &str, profile: Profile) -> String {
    match profile {
        // `outlineLvl 0` marks the title as an outline-level-1 heading so the TOC
        // field collects it, without changing the centered bold look.
        Profile::Manuscript => para(
            r#"<w:pageBreakBefore/><w:spacing w:before="240" w:after="240" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/><w:outlineLvl w:val="0"/>"#,
            run(title, true, false, Some(28)),
        ),
        Profile::Vellum => para(
            r#"<w:pStyle w:val="Heading1"/>"#,
            run(title, false, false, None),
        ),
    }
}

/// One classified body block → a paragraph. A scene break becomes a centered
/// divider; a heading line (`#`…) a centered bold line; otherwise a
/// double-spaced, first-line-indented paragraph with `*`/`**` emphasis as
/// italic/bold runs.
fn prose_paragraph(prose: &Prose) -> String {
    match prose {
        Prose::SceneBreak => para(
            r#"<w:spacing w:before="120" w:after="120" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
            run(SCENE_BREAK, false, false, None),
        ),
        Prose::Heading(heading) => para(
            r#"<w:spacing w:before="120" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
            run(heading, true, false, None),
        ),
        Prose::Paragraph(text) => {
            let runs: String = inline_runs(text)
                .into_iter()
                .map(|r| run(&r.text, r.bold, r.italic, None))
                .collect();
            // Widow/orphan control keeps a lone paragraph line off a page top/bottom
            // (#650). Body stays ragged-right — standard manuscript format and Vellum
            // import both expect unjustified text, applying their own typography.
            para(r#"<w:widowControl/><w:ind w:firstLine="720"/>"#, runs)
        }
        // Verbatim monospace: line breaks preserved, no indent or emphasis (#522).
        Prose::Preformatted(code) => para(
            r#"<w:spacing w:line="240" w:lineRule="auto"/>"#,
            mono_run(code),
        ),
    }
}

/// A single monospaced run carrying a preformatted block — its lines separated by
/// `<w:br/>` and whitespace preserved, no emphasis (#522).
fn mono_run(code: &str) -> String {
    // The breaks only ever sit between two `<w:t>` runs, so this is a join.
    let inner = code
        .lines()
        .map(|line| format!("<w:t xml:space=\"preserve\">{}</w:t>", xml_escape(line)))
        .collect::<Vec<_>>()
        .join("<w:br/>");
    format!("<w:r><w:rPr><w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\"/></w:rPr>{inner}</w:r>")
}

/// An epigraph as set-apart italic paragraphs (single-spaced, indented), the
/// attribution on its own line.
fn epigraph_paras(epigraph: &Epigraph) -> String {
    let ppr = r#"<w:ind w:left="1440"/><w:spacing w:line="240" w:lineRule="auto"/>"#;
    render_epigraph(
        epigraph,
        |line| para(ppr, run(line, false, true, None)),
        |attribution| para(ppr, run(&format!("— {attribution}"), false, true, None)),
    )
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
            publishing: Default::default(),
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
            publishing: Default::default(),
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
    fn a_fenced_block_renders_as_a_monospace_paragraph_with_breaks() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: Default::default(),
            epigraph: None,
            front: vec![],
            parts: vec![crate::export::Part {
                name: None,
                chapters: vec![crate::export::Chapter {
                    title: "C".into(),
                    epigraph: None,
                    body: "Before.\n\n```\nLOG one\nLOG two\n```\n\nAfter.".into(),
                }],
            }],
            back: vec![],
        };
        let xml = part(
            &to_docx(&d, Profile::Manuscript).unwrap(),
            "word/document.xml",
        );
        // Monospace font, both lines present, joined by a run-level break.
        assert!(xml.contains("Courier New"), "monospace font:\n{xml}");
        assert!(
            xml.contains("LOG one</w:t><w:br/><w:t xml:space=\"preserve\">LOG two"),
            "lines kept with a break:\n{xml}"
        );
    }

    #[test]
    fn series_docx_carries_every_book_in_order() {
        let book = |title: &str| crate::export::Document {
            title: title.into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: Default::default(),
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
    fn body_paragraphs_carry_widow_orphan_control() {
        // Widow/orphan control (#650): each body paragraph sets `widowControl` so no
        // single line strands at a page top/bottom. The body stays ragged-right (no
        // justified `jc`) — manuscript format and Vellum import both expect that.
        for profile in [Profile::Manuscript, Profile::Vellum] {
            let xml = part(&to_docx(&doc(), profile).unwrap(), "word/document.xml");
            assert!(
                xml.contains("<w:widowControl/>"),
                "widowControl on body paragraphs"
            );
            // The body paragraph pairs widowControl with the first-line indent.
            assert!(
                xml.contains(r#"<w:widowControl/><w:ind w:firstLine="720"/>"#),
                "widowControl on the indented body paragraph: {xml}"
            );
            // Body is not justified (manuscript/Vellum convention).
            assert!(
                !xml.contains(r#"<w:jc w:val="both"/>"#),
                "body stays ragged-right, not justified"
            );
        }
    }

    #[test]
    fn a_scene_break_becomes_a_centered_divider_not_literal_dashes() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: Default::default(),
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
    fn a_copyright_page_renders_after_the_title_page() {
        // Publishing metadata (#583) renders a copyright page after the title page,
        // in reading order, carrying the rights line and publisher.
        let mut d = doc();
        d.publishing = PubMeta {
            publisher: Some("Quill & Co.".into()),
            date: Some("2026".into()),
            rights: Some("© 2026 A. Writer".into()),
            ..Default::default()
        };
        let xml = part(
            &to_docx(&d, Profile::Manuscript).unwrap(),
            "word/document.xml",
        );
        let author_at = xml.find("by A. Writer").expect("title page");
        let rights_at = xml.find("© 2026 A. Writer").expect("rights line");
        let body_at = xml.find("The Find").expect("first chapter");
        assert!(
            author_at < rights_at && rights_at < body_at,
            "copyright between title page and body: {xml}"
        );
        assert!(
            xml.contains("Published by Quill &amp; Co."),
            "publisher escaped: {xml}"
        );
        assert!(xml.contains("First published 2026"), "date line");

        // No publishing metadata → no copyright page (default output unchanged).
        let plain = part(
            &to_docx(&doc(), Profile::Manuscript).unwrap(),
            "word/document.xml",
        );
        assert!(!plain.contains("Published by"), "no copyright by default");

        // Vellum applies its own front matter → no generated copyright page.
        let vellum = part(&to_docx(&d, Profile::Vellum).unwrap(), "word/document.xml");
        assert!(
            !vellum.contains("Published by"),
            "no copyright page in the Vellum profile"
        );
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

    /// Whether the produced .docx zip contains a part by name.
    fn has_part(bytes: &[u8], name: &str) -> bool {
        let zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
        let names: Vec<String> = zip.file_names().map(str::to_string).collect();
        names.iter().any(|n| n == name)
    }

    #[test]
    fn manuscript_profile_emits_a_running_header_with_a_page_field() {
        // #705: the manuscript profile carries a running header (author / title /
        // page) wired through the package — a header part, its content-type
        // override, a relationship, and a section reference.
        let bytes = to_docx(&doc(), Profile::Manuscript).unwrap();
        assert!(
            has_part(&bytes, "word/header1.xml"),
            "header part is written"
        );
        let header = part(&bytes, "word/header1.xml");
        // Running header carries author and title, right-aligned.
        assert!(header.contains("<w:hdr"), "header root element");
        assert!(
            header.contains("A. Writer /") && header.contains("Red &lt;State&gt; Earth /"),
            "author and (escaped) title in the header: {header}"
        );
        assert!(
            header.contains(r#"<w:jc w:val="right"/>"#),
            "header right-aligned"
        );
        // A live PAGE field, not a literal number.
        assert!(
            header.contains("<w:instrText xml:space=\"preserve\"> PAGE </w:instrText>"),
            "PAGE field instruction: {header}"
        );
        assert!(
            header.contains(r#"<w:fldChar w:fldCharType="begin"/>"#)
                && header.contains(r#"<w:fldChar w:fldCharType="end"/>"#),
            "field delimiters"
        );
        // The content-type override and relationship are declared, and the section
        // references the header.
        let ctypes = part(&bytes, "[Content_Types].xml");
        assert!(
            ctypes.contains(r#"PartName="/word/header1.xml""#),
            "header content-type override: {ctypes}"
        );
        let rels = part(&bytes, "word/_rels/document.xml.rels");
        assert!(
            rels.contains(r#"Target="header1.xml""#) && rels.contains(r#"Id="rId2""#),
            "header relationship: {rels}"
        );
        let xml = part(&bytes, "word/document.xml");
        assert!(
            xml.contains(r#"<w:headerReference w:type="default" r:id="rId2"/>"#),
            "section references the header: {xml}"
        );
        assert!(
            xml.contains(
                r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#
            ),
            "the r namespace is declared for the header id"
        );
    }

    #[test]
    fn manuscript_profile_emits_a_toc_field_for_the_chapters() {
        // #705: a TOC field collects the chapter/part titles into a clickable,
        // updatable contents. The chapter headings carry outline level 1 so the
        // field picks them up.
        let bytes = to_docx(&doc(), Profile::Manuscript).unwrap();
        let xml = part(&bytes, "word/document.xml");
        assert!(
            xml.contains(r#"<w:instrText xml:space="preserve"> TOC \o "1-1" \h </w:instrText>"#),
            "TOC field instruction: {xml}"
        );
        // The Contents heading precedes the chapter body.
        let toc_at = xml.find("Contents").expect("contents heading");
        let chapter_at = xml.find("The Find").expect("first chapter");
        assert!(toc_at < chapter_at, "TOC before the body: {xml}");
        // Chapter/part headings are outline level 1 so the TOC can collect them.
        assert!(
            xml.contains(r#"<w:outlineLvl w:val="0"/>"#),
            "headings carry an outline level: {xml}"
        );
    }

    #[test]
    fn vellum_profile_has_no_header_or_toc() {
        // Vellum applies its own front matter and running header — the generated
        // package must not add one, or it would conflict on import.
        let bytes = to_docx(&doc(), Profile::Vellum).unwrap();
        assert!(
            !has_part(&bytes, "word/header1.xml"),
            "no header part in the Vellum profile"
        );
        let ctypes = part(&bytes, "[Content_Types].xml");
        assert!(
            !ctypes.contains("header1.xml"),
            "no header content-type override"
        );
        let rels = part(&bytes, "word/_rels/document.xml.rels");
        assert!(!rels.contains("header1.xml"), "no header relationship");
        let xml = part(&bytes, "word/document.xml");
        assert!(
            !xml.contains("<w:headerReference"),
            "no header reference in the section"
        );
        assert!(!xml.contains(" TOC "), "no TOC field: {xml}");
        assert!(!xml.contains("Contents"), "no contents page");
    }

    #[test]
    fn series_docx_carries_a_header_and_toc() {
        // #705: the series document (manuscript) also gets a running header
        // (series / author) and a TOC over the book/chapter titles.
        let book = |title: &str| crate::export::Document {
            title: title.into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: Default::default(),
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
        let header = part(&bytes, "word/header1.xml");
        assert!(
            header.contains("The Saga /") && header.contains("A. Writer /"),
            "series and author in the header: {header}"
        );
        let xml = part(&bytes, "word/document.xml");
        assert!(xml.contains(" TOC "), "series TOC field: {xml}");
        assert!(
            xml.contains(r#"<w:headerReference w:type="default" r:id="rId2"/>"#),
            "series header reference"
        );
    }
}
