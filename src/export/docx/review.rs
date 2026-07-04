//! The review-copy `.docx` writer (#834): a markup-ready manuscript for a beta
//! reader or editor. Unlike the "final" `.docx` profiles, this one is built to be
//! *marked up* — every body line is numbered down the margin, the page carries
//! generous 2" margins for handwritten or tracked notes, and the author's inline
//! annotations (#302), which every other export strips, ride along as Word
//! comments anchored to the phrases they were left on.
//!
//! It is a separate entry point rather than a [`Profile`](super::Profile) so the
//! shared `document.xml` path stays annotation-free: the comment plumbing (a
//! `word/comments.xml` part, its content-type + relationship, and the in-body
//! `w:commentRangeStart`/`End` + `w:commentReference` markers) lives here and
//! touches nothing the manuscript/Vellum writers emit.

use std::fmt::Write;
use std::io::{Cursor, Write as _};

use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use super::body::{para, run};
use super::{inline_runs, prose_blocks, xml_escape, Block, Document, Prose, SCENE_BREAK};

/// One inline annotation (#302) carried into the review copy: the exact prose it
/// was anchored to (`text`, extracted from the chapter body) and the author's
/// note. The writer finds `text` inside the rendered paragraph and wraps it with
/// comment-range markers, so the anchoring survives the body's transforms
/// (title-strip, soft-wrap join, emphasis-marker removal) without threading byte
/// offsets through them.
#[derive(Clone, Debug)]
pub struct ReviewNote {
    pub text: String,
    pub note: String,
}

/// The annotations for one chapter of the [`Document`], in the same order the
/// review writer visits chapters (front, then each part's chapters, then back —
/// i.e. [`Document::blocks`] order). One entry per chapter, empty when a chapter
/// carries no notes.
pub type ChapterNotes = Vec<ReviewNote>;

/// Compile a [`Document`] to a review-copy `.docx` (#834): a title page, each
/// chapter under its heading, single-spaced numbered lines with generous margins,
/// and the author's annotations as anchored Word comments. `notes` carries one
/// [`ChapterNotes`] per chapter in [`Document::blocks`] order (front → parts →
/// back); a chapter with no annotations gets an empty entry.
pub fn to_docx_review(doc: &Document, notes: &[ChapterNotes]) -> std::io::Result<Vec<u8>> {
    let mut w = CommentWriter::default();
    let body = review_body(doc, notes, &mut w);
    let document = wrap_review_document(&body);
    build_review_docx(document, w.comments_xml())
}

/// Allocates comment ids and accumulates each comment's body as the writer walks
/// the document, so the in-body `w:commentReference` and the `word/comments.xml`
/// entries share one id space and stay in sync.
#[derive(Default)]
struct CommentWriter {
    /// Rendered `<w:comment …>…</w:comment>` blocks, in id order.
    comments: Vec<String>,
}

impl CommentWriter {
    /// Register a comment carrying `note`, returning its id for the in-body
    /// range/reference markers.
    fn add(&mut self, note: &str) -> usize {
        let id = self.comments.len();
        // A comment's body is itself a paragraph; the author/date are the generic
        // review attribution (Quill leaves the note, on the author's behalf).
        let body = para("", run(note, false, false, None));
        self.comments.push(format!(
            concat!(
                "<w:comment w:id=\"{id}\" w:author=\"Author annotation\" ",
                "w:initials=\"QA\" w:date=\"1970-01-01T00:00:00Z\">{body}</w:comment>",
            ),
            id = id,
            body = body,
        ));
        id
    }

    /// The `word/comments.xml` part, or `None` when the copy carries no
    /// annotations (the part, its content-type, and its relationship are all
    /// omitted then, keeping a clean copy minimal).
    fn comments_xml(&self) -> Option<String> {
        if self.comments.is_empty() {
            return None;
        }
        Some(format!(
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
                "<w:comments xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
                "{body}</w:comments>",
            ),
            body = self.comments.concat(),
        ))
    }
}

/// The review body: a centered title/author block, then each part heading and
/// chapter — the chapter's prose rendered with any of its annotations wrapped as
/// comments. `notes` is consumed chapter by chapter in visit order.
fn review_body(doc: &Document, notes: &[ChapterNotes], w: &mut CommentWriter) -> String {
    let mut body = String::new();
    body.push_str(&para(
        r#"<w:spacing w:before="2400"/><w:jc w:val="center"/>"#,
        run(&doc.title, true, false, Some(36)),
    ));
    if let Some(author) = &doc.author {
        body.push_str(&para(
            r#"<w:jc w:val="center"/>"#,
            run(&format!("by {author}"), false, false, None),
        ));
    }
    body.push_str(&para(
        r#"<w:jc w:val="center"/>"#,
        run("Review copy — for markup", false, true, None),
    ));

    // Chapters are visited in `blocks()` order; `notes` is aligned to that, so a
    // running index pulls each chapter's annotations as we reach it. An empty or
    // short `notes` simply yields no annotations for the remaining chapters.
    let mut chapter = 0;
    let empty: ChapterNotes = Vec::new();
    for block in doc.blocks() {
        match block {
            Block::Part(name) => body.push_str(&heading(name)),
            Block::Chapter(ch) => {
                body.push_str(&heading(&ch.title));
                let chapter_notes = notes.get(chapter).unwrap_or(&empty);
                for prose in prose_blocks(&ch.body) {
                    body.push_str(&review_paragraph(&prose, chapter_notes, w));
                }
                chapter += 1;
            }
        }
    }
    body
}

/// A chapter or part heading: a fresh page under a centered bold line, outline
/// level 1 so a reader's navigation pane still lists them.
fn heading(title: &str) -> String {
    para(
        r#"<w:pageBreakBefore/><w:spacing w:before="240" w:after="240"/><w:jc w:val="center"/><w:outlineLvl w:val="0"/>"#,
        run(title, true, false, Some(28)),
    )
}

/// One prose block → a paragraph, with any annotation whose anchored text falls
/// inside it wrapped in comment-range markers. Scene breaks and headings carry no
/// annotations (they render as their own centered lines).
fn review_paragraph(prose: &Prose, notes: &[ReviewNote], w: &mut CommentWriter) -> String {
    match prose {
        Prose::SceneBreak => para(
            r#"<w:jc w:val="center"/>"#,
            run(SCENE_BREAK, false, false, None),
        ),
        Prose::Heading(heading) => {
            para(r#"<w:jc w:val="center"/>"#, run(heading, true, false, None))
        }
        Prose::Preformatted(code) => para("", mono_runs(code)),
        Prose::Paragraph(text) => para("", annotated_runs(text, notes, w)),
    }
}

/// The runs for a paragraph, splicing comment-range markers around any annotated
/// span. Each annotation whose anchored text appears in the paragraph contributes
/// a `w:commentRangeStart` before its run and a `w:commentRangeEnd` +
/// `w:commentReference` after, so a reader sees the comment anchored to exactly
/// that phrase. Emphasis (`*`/`**`) inside a non-annotated span still carries
/// through as bold/italic runs.
fn annotated_runs(text: &str, notes: &[ReviewNote], w: &mut CommentWriter) -> String {
    // Find each annotation's byte span in the (transform-collapsed) paragraph
    // text, keeping only those that match, sorted and non-overlapping so the
    // splice is a clean left-to-right walk.
    let mut spans: Vec<(usize, usize, usize)> = Vec::new(); // (start, end, comment id)
    for annotation in notes {
        let needle = annotation.text.trim();
        if needle.is_empty() {
            continue;
        }
        if let Some(start) = text.find(needle) {
            let end = start + needle.len();
            // Skip a span overlapping one already taken — Word nests ranges, but a
            // clean, disjoint anchoring is what a marked-up copy wants.
            if spans.iter().any(|&(s, e, _)| start < e && s < end) {
                continue;
            }
            let id = w.add(&annotation.note);
            spans.push((start, end, id));
        }
    }
    spans.sort_by_key(|&(s, ..)| s);

    let mut out = String::new();
    let mut cursor = 0;
    for (start, end, id) in spans {
        // The prose before this annotation, as ordinary emphasis-aware runs.
        out.push_str(&emphasis_runs(&text[cursor..start]));
        let _ = write!(out, r#"<w:commentRangeStart w:id="{id}"/>"#);
        out.push_str(&emphasis_runs(&text[start..end]));
        let _ = write!(out, r#"<w:commentRangeEnd w:id="{id}"/>"#);
        let _ = write!(out, r#"<w:r><w:commentReference w:id="{id}"/></w:r>"#);
        cursor = end;
    }
    // Any prose after the last annotation.
    out.push_str(&emphasis_runs(&text[cursor..]));
    out
}

/// Emphasis-aware runs for a stretch of paragraph text — `*`/`**` carried through
/// as italic/bold, the shared splitter the other writers use.
fn emphasis_runs(text: &str) -> String {
    inline_runs(text)
        .into_iter()
        .map(|r| run(&r.text, r.bold, r.italic, None))
        .collect()
}

/// A monospaced run carrying a preformatted block, its lines separated by
/// `<w:br/>` and whitespace preserved (mirrors the manuscript writer's #522).
fn mono_runs(code: &str) -> String {
    let inner = code
        .lines()
        .map(|line| format!("<w:t xml:space=\"preserve\">{}</w:t>", xml_escape(line)))
        .collect::<Vec<_>>()
        .join("<w:br/>");
    format!("<w:r><w:rPr><w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\"/></w:rPr>{inner}</w:r>")
}

/// Wrap the review body in the `document.xml` shell. The section turns on
/// continuous line numbering (`w:lnNumType`) — every line numbered from 1, the
/// count running straight through — and sets 2" margins all round for markup. The
/// `w:commentRangeStart`/`End` markers in the body reference `word/comments.xml`.
fn wrap_review_document(body: &str) -> String {
    // 2" margins (2880 twips) for markup; letter page. `w:lnNumType` numbers every
    // line (`w:countBy="1"`) continuously (`w:restart="continuous"`) from 1.
    let sect = concat!(
        "<w:sectPr>",
        r#"<w:pgSz w:w="12240" w:h="15840"/>"#,
        r#"<w:pgMar w:top="2880" w:right="2880" w:bottom="2880" w:left="2880"/>"#,
        r#"<w:lnNumType w:countBy="1" w:start="1" w:restart="continuous"/>"#,
        "</w:sectPr>",
    );
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
            "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
            "<w:body>{body}{sect}</w:body></w:document>",
        ),
        body = body,
        sect = sect,
    )
}

// --- Package plumbing ------------------------------------------------------

/// The styles part for the review copy: Times New Roman 12pt, **single**-spaced
/// (double-spacing wastes the page the numbered margins are for). A base `Normal`
/// so word processors have a default style to hang the comments' own on.
const REVIEW_STYLES: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
    "<w:docDefaults><w:rPrDefault><w:rPr>",
    "<w:rFonts w:ascii=\"Times New Roman\" w:hAnsi=\"Times New Roman\"/>",
    "<w:sz w:val=\"24\"/><w:szCs w:val=\"24\"/></w:rPr></w:rPrDefault>",
    "<w:pPrDefault><w:pPr><w:spacing w:line=\"240\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault>",
    "</w:docDefaults>",
    "<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>",
    "</w:styles>",
);

/// `[Content_Types].xml` for the review copy, adding the comments override when a
/// comments part is present.
fn review_content_types(has_comments: bool) -> String {
    let comments = if has_comments {
        r#"<Override PartName="/word/comments.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"/>"#
    } else {
        ""
    };
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
            "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">",
            "<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>",
            "<Default Extension=\"xml\" ContentType=\"application/xml\"/>",
            "<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>",
            "<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>",
            "{comments}</Types>",
        ),
        comments = comments,
    )
}

const REVIEW_RELS: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>",
    "</Relationships>",
);

/// The document relationships: always styles; the comments part when present
/// (`rId2`, which the body's comment references resolve through).
fn review_doc_rels(has_comments: bool) -> String {
    let comments = if has_comments {
        r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="comments.xml"/>"#
    } else {
        ""
    };
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
            "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
            "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
            "{comments}</Relationships>",
        ),
        comments = comments,
    )
}

/// Zip the review-copy `.docx` around its built `document.xml`, adding the
/// `word/comments.xml` part (with its content-type + relationship) only when
/// there are annotations to carry.
fn build_review_docx(document_xml: String, comments: Option<String>) -> std::io::Result<Vec<u8>> {
    let has_comments = comments.is_some();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default();
    let mut parts = vec![
        (
            "[Content_Types].xml".to_string(),
            review_content_types(has_comments),
        ),
        ("_rels/.rels".to_string(), REVIEW_RELS.to_string()),
        (
            "word/_rels/document.xml.rels".to_string(),
            review_doc_rels(has_comments),
        ),
        ("word/styles.xml".to_string(), REVIEW_STYLES.to_string()),
        ("word/document.xml".to_string(), document_xml),
    ];
    if let Some(comments) = comments {
        parts.push(("word/comments.xml".to_string(), comments));
    }
    for (name, content) in parts {
        zip.start_file(name, opts).map_err(std::io::Error::other)?;
        zip.write_all(content.as_bytes())?;
    }
    Ok(zip.finish().map_err(std::io::Error::other)?.into_inner())
}
