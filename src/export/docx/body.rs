//! Body and paragraph rendering for the `.docx` writer: the `word/document.xml`
//! shell, the manuscript title/copyright/contents pages, part/chapter headings,
//! prose paragraphs (with `*`/`**` emphasis, scene breaks, and preformatted
//! blocks), and the low-level `<w:p>`/`<w:r>` builders the rest of the writer
//! shares.

use std::fmt::Write;

use super::{
    copyright_lines, inline_runs, prose_blocks, render_epigraph, running_header, xml_escape, Block,
    Document, Header, Profile, Prose, PubMeta, SCENE_BREAK,
};
use crate::frontmatter::Epigraph;

/// The `word/document.xml` for one book: the title block (manuscript only), then
/// its parts/chapters, wrapped in the document shell.
pub(super) fn document_xml(doc: &Document, profile: Profile) -> String {
    let header = running_header(doc.author.as_deref(), Some(&doc.title), profile);
    let mut body = String::new();
    if profile == Profile::Manuscript {
        // The half-title (#820) is the leaf ahead of the full title page, when
        // enabled — the manuscript profile's front matter (Vellum splits on
        // Heading 1 and applies its own front matter, so both new pages sit out).
        if doc.half_title {
            body.push_str(&half_title_page(&doc.title));
        }
        body.push_str(&title_page(doc));
        body.push_str(&copyright_page(&doc.publishing));
        // The "also in this series" page (#820), when this is a series title's book.
        body.push_str(&series_list_page(&doc.series_list_lines()));
        body.push_str(&toc_page());
    }
    body.push_str(&book_body(doc, profile));
    wrap_document(&body, &header)
}

/// The half-title page (#820), when enabled: the book title alone on a fresh page
/// ahead of the full title page — the conventional first printed leaf of a trade
/// book. Manuscript profile only. A bare page, so just the centered title.
fn half_title_page(title: &str) -> String {
    para(
        r#"<w:pageBreakBefore/><w:spacing w:before="3600"/><w:jc w:val="center"/>"#,
        run(title, true, false, Some(28)),
    )
}

/// The "also in this series" page (#820), when a series title's book supplies the
/// list: a fresh page with the heading, then each *other* book of the series by
/// title, centered. `lines` is [`Document::series_list_lines`] — the heading
/// followed by the book titles — so an empty slice (a standalone book, or the page
/// disabled) yields an empty string that leaves the output unchanged. Manuscript
/// profile only.
fn series_list_page(lines: &[String]) -> String {
    let Some((heading, books)) = lines.split_first() else {
        return String::new();
    };
    let mut out = para(
        r#"<w:pageBreakBefore/><w:spacing w:before="2400" w:after="240"/><w:jc w:val="center"/>"#,
        run(heading, true, false, Some(28)),
    );
    for book in books {
        out.push_str(&para(
            r#"<w:jc w:val="center"/>"#,
            run(book, false, true, None),
        ));
    }
    out
}

/// The copyright page (#583), when publishing metadata warrants one: the rights
/// line, publisher, publication date, and ISBN, each centered on its own page
/// after the title page (cover → title → copyright → body). Manuscript profile
/// only — Vellum applies its own front matter and splits on Heading 1. Empty when
/// no metadata is set, leaving the output unchanged.
pub(super) fn copyright_page(publishing: &PubMeta) -> String {
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
pub(super) fn book_body(doc: &Document, profile: Profile) -> String {
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

/// Wrap a built body in the `word/document.xml` shell (letter page, 1" margins).
/// When a [`Header`] is present (manuscript profile), the section references the
/// `word/header1.xml` part via `rId2` so the running header shows on every page.
pub(super) fn wrap_document(body: &str, header: &Header) -> String {
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
pub(super) fn page_field() -> String {
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
pub(super) fn toc_page() -> String {
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
    let _ = write!(
        out,
        "<w:p><w:pPr><w:spacing w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>{field}</w:p>",
    );
    out
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
// `runs` by value: taking `&str` would ripple `&` into call sites across other
// docx modules (series.rs, mod.rs) outside this change's scope.
#[allow(clippy::needless_pass_by_value)]
pub(super) fn para(ppr: &str, runs: String) -> String {
    let ppr = if ppr.is_empty() {
        String::new()
    } else {
        format!("<w:pPr>{ppr}</w:pPr>")
    };
    format!("<w:p>{ppr}{runs}</w:p>")
}

/// A `<w:r>` run with bold/italic and an optional font size (half-points).
pub(super) fn run(text: &str, bold: bool, italic: bool, size: Option<u32>) -> String {
    let mut rpr = String::new();
    if bold {
        rpr.push_str("<w:b/>");
    }
    if italic {
        rpr.push_str("<w:i/>");
    }
    if let Some(sz) = size {
        let _ = write!(rpr, "<w:sz w:val=\"{sz}\"/>");
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
