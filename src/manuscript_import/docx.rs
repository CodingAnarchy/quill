//! Word `.docx` import (#401): a `.docx` is a zip of XML, so read
//! `word/document.xml` (and the OOXML core properties) and split chapters on
//! Heading 1 paragraphs, turning bold/italic runs into `**`/`*`. See the crate
//! module doc for the rules; the shared XML scanner lives in `mod.rs`.

use std::fmt::Write;
use std::io::{Cursor, Read};

use super::{
    assemble, attr_value, count_elements, decode_entities, element_text, fallback_chapter,
    finalize_titles, for_each_child, for_each_element, join_paragraphs, mine_preamble, tag_attrs,
    wrap_emphasis, Dropped, Imported,
};

/// Parse a Word `.docx` manuscript: read `word/document.xml` (and the core
/// properties) out of the zip and split it into chapters. The bytes are read
/// only — the caller never alters the source. Errors if the zip is unreadable or
/// has no `word/document.xml` (i.e. it isn't a Word document).
pub fn parse_docx(bytes: &[u8]) -> std::io::Result<Imported> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(std::io::Error::other)?;
    let document = zip_entry(&mut zip, "word/document.xml")?
        .ok_or_else(|| std::io::Error::other("not a Word document (no word/document.xml)"))?;
    let core = zip_entry(&mut zip, "docProps/core.xml")?;
    Ok(parse_document_xml(&document, core.as_deref()))
}

/// One Word comment recovered from a marked-up `.docx` (#886): the prose it was
/// anchored to (`phrase`, the text the reviewer selected before commenting) and
/// the comment's own text (`note`). The inverse of the review-copy writer's
/// [`ReviewNote`](crate::export::docx::ReviewNote): the writer wraps an
/// annotation's phrase in `w:commentRangeStart`/`End` markers and stashes its note
/// in `word/comments.xml`; this recovers the pair so the phrase can be re-anchored
/// as a live annotation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedComment {
    /// The commented prose — the text between the range start/end markers, with
    /// emphasis flattened (it's the reader-visible run text, not Markdown).
    pub phrase: String,
    /// The reviewer's comment text (the body of `word/comments.xml`).
    pub note: String,
    /// The named reviewer the comment came from (#933) — the `w:author` attribute
    /// Word records on each comment. `None` when the `.docx` carries no author on
    /// it (or the generic self-attribution Quill's own review-copy export writes),
    /// so an imported note is grouped, filtered, and sorted by its editor.
    pub reviewer: Option<String>,
}

/// Extract the Word comments from a marked-up `.docx` (#886): read
/// `word/comments.xml` (id → note text) and `word/document.xml` (each comment's
/// anchored phrase, the run text between its `w:commentRangeStart`/`End` markers),
/// pairing them by id. Returns the comments in document order — empty when the
/// `.docx` carries none. The bytes are read only. Errors only if the zip is
/// unreadable or has no `word/document.xml` (i.e. it isn't a Word document); a
/// `.docx` with no comments part is a clean empty result.
pub fn extract_comments(bytes: &[u8]) -> std::io::Result<Vec<ImportedComment>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(std::io::Error::other)?;
    let document = zip_entry(&mut zip, "word/document.xml")?
        .ok_or_else(|| std::io::Error::other("not a Word document (no word/document.xml)"))?;
    let comments = zip_entry(&mut zip, "word/comments.xml")?;
    Ok(extract_comments_from_xml(&document, comments.as_deref()))
}

/// The pure core of [`extract_comments`]: pair each in-body comment anchor with
/// its note text. `document` is `word/document.xml`; `comments` is
/// `word/comments.xml` (`None`/empty ⇒ no comments to recover). Each comment's
/// note is looked up by its `w:id`; its phrase is the run text between the
/// matching `w:commentRangeStart`/`End`. A start with no matching end, or a note
/// id with no in-body anchor, is skipped — only a fully-formed pair is recovered.
/// Comments are returned in the order their ranges open in the body.
fn extract_comments_from_xml(document: &str, comments: Option<&str>) -> Vec<ImportedComment> {
    let notes = comment_notes(comments.unwrap_or(""));
    let mut out = Vec::new();
    // Walk the body once, collecting each range's (id, start byte) as its start
    // marker opens; on the matching end marker, slice the run text between them.
    let mut open: Vec<(String, usize)> = Vec::new();
    let mut pos = 0;
    while let Some((id, start_after, is_end)) = next_comment_marker(document, pos) {
        pos = start_after;
        if is_end {
            // The end marker begins just before `start_after`; find where it opens
            // so the sliced phrase excludes the marker tag itself.
            if let Some(open_at) = open.iter().rposition(|(oid, _)| oid == &id) {
                let (_, from) = open.remove(open_at);
                if let Some(note) = notes.get(&id) {
                    let end_at = document[..start_after]
                        .rfind("<w:commentRangeEnd")
                        .unwrap_or(start_after);
                    let phrase = run_text(&document[from..end_at]);
                    let phrase = phrase.trim();
                    if !phrase.is_empty() {
                        out.push(ImportedComment {
                            phrase: phrase.to_string(),
                            note: note.note.clone(),
                            reviewer: note.reviewer.clone(),
                        });
                    }
                }
            }
        } else {
            open.push((id, start_after));
        }
    }
    out
}

/// The next `w:commentRangeStart` / `w:commentRangeEnd` marker in `xml` at or
/// after `from`: its `w:id`, the byte index just past its `>`, and whether it's an
/// end marker. `None` when no further marker remains. The one seam the comment
/// walk advances on.
fn next_comment_marker(xml: &str, from: usize) -> Option<(String, usize, bool)> {
    let start = find_marker(xml, from, "w:commentRangeStart");
    let end = find_marker(xml, from, "w:commentRangeEnd");
    match (start, end) {
        (Some((sa, sid)), Some((ea, eid))) => {
            if sa <= ea {
                Some((sid, sa, false))
            } else {
                Some((eid, ea, true))
            }
        }
        (Some((sa, sid)), None) => Some((sid, sa, false)),
        (None, Some((ea, eid))) => Some((eid, ea, true)),
        (None, None) => None,
    }
}

/// The next `<name w:id="…"/>` marker at or after `from`: `(index past its '>',
/// its id)`. Scans by the literal tag, reading the `w:id` from the marker's
/// attributes; a marker missing an id is skipped.
fn find_marker(xml: &str, from: usize, name: &str) -> Option<(usize, String)> {
    let pat = format!("<{name}");
    let mut search = from;
    loop {
        let at = search + xml[search..].find(&pat)?;
        let gt = at + xml[at..].find('>')?;
        let id = attr_value(&xml[at + pat.len()..gt], "w:id");
        match id {
            Some(id) => return Some((gt + 1, id)),
            None => search = gt + 1,
        }
    }
}

/// One recovered comment body: its note text and the reviewer it's attributed to
/// (#933), paired to the in-body anchor by id.
struct CommentBody {
    note: String,
    reviewer: Option<String>,
}

/// The generic self-attribution Quill's own review-copy export (#834) stamps on
/// every comment. It names no real reviewer, so importing Quill's own copy back
/// leaves the note author-authored (`reviewer = None`) rather than tagging it to
/// this placeholder.
const SELF_ATTRIBUTION: &str = "Author annotation";

/// Map each `<w:comment w:id="…">` to its note text and reviewer, from
/// `word/comments.xml`. The note is the concatenated `<w:t>` run text of the
/// comment body (empty comments map to an empty string); the reviewer is the
/// `w:author` attribute (#933), dropped when absent, blank, or Quill's own
/// self-attribution. An id-less comment is skipped.
fn comment_notes(comments: &str) -> std::collections::HashMap<String, CommentBody> {
    let mut out = std::collections::HashMap::new();
    for_each_child(comments, "w:comment", |attrs, inner| {
        if let (Some(id), Some(inner)) = (attr_value(attrs, "w:id"), inner) {
            let reviewer = attr_value(attrs, "w:author")
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty() && a != SELF_ATTRIBUTION);
            out.insert(
                id,
                CommentBody {
                    note: run_text(inner).trim().to_string(),
                    reviewer,
                },
            );
        }
    });
    out
}

/// Read one entry of a `.docx` zip to a string, or `None` if it's absent.
fn zip_entry(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> std::io::Result<Option<String>> {
    match zip.by_name(name) {
        Ok(mut f) => {
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            Ok(Some(s))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(std::io::Error::other(e)),
    }
}

/// One Word paragraph: whether it's a Heading 1 (a chapter break) and its text
/// (with `*`/`**` emphasis for a body paragraph, plain for a heading title).
struct Para {
    heading: bool,
    text: String,
}

/// The pure core of [`parse_docx`]: turn the body XML (and optional core-property
/// XML) into the same [`Imported`] shape the Markdown importer produces. Splits
/// on Heading 1 paragraphs via the shared [`assemble`]; the preamble before the
/// first heading is a title page, mined for any title/author the core properties
/// didn't supply under the one shared [`mine_preamble`] policy.
fn parse_document_xml(document: &str, core: Option<&str>) -> Imported {
    let title = core.and_then(|c| element_text(c, "dc:title"));
    let author = core.and_then(|c| element_text(c, "dc:creator"));

    // Tally structural content the flat prose model can't carry (#646). Footnotes
    // and endnotes are converted to inline `[^N]` markers as paragraphs are read;
    // images/tables/comments are counted here so the caller can warn on the drop.
    // (Tables are `<w:tbl>` siblings of paragraphs, so they're counted at the
    // document level, not per-paragraph.)
    let mut dropped = Dropped {
        tables: count_elements(document, "w:tbl"),
        ..Dropped::default()
    };

    // Split on Heading 1 paragraphs; the run before the first heading is a title
    // page. Each `Para` is pre-classified, so the shared assembler just flushes.
    let (mut chapters, preamble) = assemble(
        paragraphs(document, &mut dropped),
        |para| para.heading,
        |para| para.text.clone(),
        |paras| join_paragraphs(&body_texts(paras)),
    );
    let preamble: Vec<String> = preamble.into_iter().map(|p| p.text).collect();

    // The title page fills in anything the core properties didn't — the first
    // non-empty line as the title, a "by …" line as the author (#501: mined with
    // the same unconditional policy the Markdown path uses).
    let has_chapters = !chapters.is_empty();
    let (title, author) = mine_preamble(&preamble, title, author, has_chapters, |p| {
        p.iter()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .map(str::to_string)
    });

    // A heading-less document keeps its whole text as the one chapter's body.
    fallback_chapter(&mut chapters, &title, join_paragraphs(&preamble));
    finalize_titles(&mut chapters);

    Imported {
        title,
        author,
        chapters,
        // A `.docx` carries every body inside the single `word/document.xml`
        // container; once that's read there's no per-body read to fail, so the
        // analogous unreadable-body count is always 0 (see [`Imported`]).
        unreadable_bodies: 0,
        dropped,
    }
}

/// The texts of a run of body paragraphs, for [`join_paragraphs`].
fn body_texts(paras: &[Para]) -> Vec<String> {
    paras.iter().map(|p| p.text.clone()).collect()
}

/// Walk the `<w:p>` paragraphs of `word/document.xml`, in order, tallying dropped
/// structural content into `dropped` and converting footnote/endnote references
/// to inline `[^N]` markers as it goes.
fn paragraphs(document: &str, dropped: &mut Dropped) -> Vec<Para> {
    let mut out = Vec::new();
    for_each_element(document, "w:p", |inner| {
        out.push(inner.map_or_else(
            || Para {
                heading: false,
                text: String::new(),
            },
            |p| parse_paragraph(p, dropped),
        ));
    });
    out
}

/// One paragraph's inner XML → a [`Para`]. A Heading 1 paragraph is a chapter
/// title (plain text); any other is a body paragraph with bold/italic runs
/// converted to `**`/`*` and footnote/endnote references converted to inline
/// `[^N]` markers. Images/comments in the paragraph are tallied in `dropped`.
fn parse_paragraph(inner: &str, dropped: &mut Dropped) -> Para {
    if is_heading_one(inner) {
        return Para {
            heading: true,
            text: run_text(inner),
        };
    }
    let mut text = String::new();
    for_each_element(inner, "w:r", |run| {
        if let Some(run) = run {
            text.push_str(&run_markdown(run, dropped));
        }
    });
    Para {
        heading: false,
        text,
    }
}

/// Whether a paragraph is styled Heading 1 — by its paragraph-style id
/// (`Heading1`, case/space-insensitive; the id is stable across Word's
/// display-name localization, and is what Quill's export writes) or an explicit
/// outline level 0. A heading styled only by inheritance in `styles.xml` (which we
/// don't read) isn't detected — it folds into the previous chapter's body rather
/// than corrupting output.
fn is_heading_one(inner: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    if let Some(style) = tag_attrs(inner, "w:pStyle").and_then(|a| attr_value(a, "w:val")) {
        if norm(&style) == "heading1" {
            return true;
        }
    }
    tag_attrs(inner, "w:outlineLvl")
        .and_then(|a| attr_value(a, "w:val"))
        .as_deref()
        == Some("0")
}

/// One run's inner XML → Markdown: its `<w:t>` text wrapped per its bold/italic
/// run properties, then any footnote/endnote reference in the run appended as an
/// inline `[^N]` / `[^eN]` marker (#646). Images and comments in the run are
/// tallied in `dropped` (they have no place in the flat prose model).
fn run_markdown(run: &str, dropped: &mut Dropped) -> String {
    let mut out = wrap_emphasis(
        &run_text(run),
        run_toggle(run, "w:b"),
        run_toggle(run, "w:i"),
    );
    // A footnote/endnote reference becomes an inline marker rather than vanishing.
    for _ in 0..count_elements(run, "w:footnoteReference") {
        dropped.footnotes += 1;
        let _ = write!(out, "[^{}]", dropped.footnotes);
    }
    for _ in 0..count_elements(run, "w:endnoteReference") {
        dropped.endnotes += 1;
        let _ = write!(out, "[^e{}]", dropped.endnotes);
    }
    // Images/drawings and comment references have no flat-prose equivalent; count
    // the drop so the caller can warn. `<w:pict>` is the legacy VML image form;
    // `<w:object>` an embedded object (an equation, an OLE image).
    dropped.images += count_elements(run, "w:drawing")
        + count_elements(run, "w:pict")
        + count_elements(run, "w:object");
    dropped.comments += count_elements(run, "w:commentReference");
    out
}

/// All `<w:t>` text within `xml`, concatenated and entity-decoded.
fn run_text(xml: &str) -> String {
    let mut out = String::new();
    for_each_element(xml, "w:t", |t| {
        if let Some(t) = t {
            out.push_str(&decode_entities(t));
        }
    });
    out
}

/// Whether a run-property toggle (`w:b` / `w:i`) is on: present and not turned
/// off by `w:val="false|0|none|off"`.
fn run_toggle(run: &str, name: &str) -> bool {
    tag_attrs(run, name).is_some_and(|attrs| {
        attr_value(attrs, "w:val")
            .is_none_or(|v| !matches!(v.as_str(), "false" | "0" | "none" | "off"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap paragraph XML in a `word/document.xml` shell.
    fn document(body: &str) -> String {
        format!("<w:document><w:body>{body}</w:body></w:document>")
    }

    /// A Heading 1 chapter-title paragraph.
    fn heading(title: &str) -> String {
        format!(
            "<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>{title}</w:t></w:r></w:p>"
        )
    }

    #[test]
    fn docx_splits_on_heading_one_and_converts_emphasis() {
        let body = format!(
            "{}{}{}",
            heading("One"),
            // "The sea was *grey* and it **ended**." across three runs.
            "<w:p><w:r><w:t xml:space=\"preserve\">The sea was </w:t></w:r>\
             <w:r><w:rPr><w:i/></w:rPr><w:t>grey</w:t></w:r>\
             <w:r><w:t xml:space=\"preserve\"> and it </w:t></w:r>\
             <w:r><w:rPr><w:b/></w:rPr><w:t>ended</w:t></w:r>\
             <w:r><w:t>.</w:t></w:r></w:p>",
            heading("Two"),
        );
        let out = parse_document_xml(&document(&body), None);
        assert_eq!(out.chapters.len(), 2);
        assert_eq!(out.chapters[0].title, "One");
        assert_eq!(
            out.chapters[0].body,
            "The sea was *grey* and it **ended**.\n"
        );
        assert_eq!(out.chapters[1].title, "Two");
    }

    #[test]
    fn docx_reads_title_and_author_from_core_properties() {
        let core = "<cp:coreProperties><dc:title>Salt and Iron</dc:title>\
                    <dc:creator>J. Reader</dc:creator></cp:coreProperties>";
        let out = parse_document_xml(&document(&heading("One")), Some(core));
        assert_eq!(out.title.as_deref(), Some("Salt and Iron"));
        assert_eq!(out.author.as_deref(), Some("J. Reader"));
    }

    #[test]
    fn docx_mines_a_title_block_when_core_props_are_absent() {
        // A Word title page: plain paragraphs before the first heading.
        let body = format!(
            "{}{}{}",
            "<w:p><w:r><w:t>Salt and Iron</w:t></w:r></w:p>",
            "<w:p><w:r><w:t>by J. Reader</w:t></w:r></w:p>",
            heading("One"),
        );
        let out = parse_document_xml(&document(&body), None);
        assert_eq!(
            out.title.as_deref(),
            Some("Salt and Iron"),
            "first preamble line"
        );
        assert_eq!(out.author.as_deref(), Some("J. Reader"), "the by-line");
        // The title page is not itself a chapter.
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "One");
    }

    #[test]
    fn docx_with_no_headings_is_one_chapter() {
        let body = "<w:p><w:r><w:t>Just prose, no headings.</w:t></w:r></w:p>";
        let out = parse_document_xml(&document(body), None);
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "Manuscript");
        assert_eq!(out.chapters[0].body, "Just prose, no headings.\n");
    }

    /// #501 consistency fix: a heading-less manuscript with a "by …" attribution
    /// line mines the author the same way in both formats. Previously Markdown
    /// mined the preamble unconditionally while `.docx` mined only when real
    /// chapters existed, so this exact input gave `.docx` no author and Markdown
    /// one. Both now mine it (one shared policy), and neither invents a chapter
    /// title from the body — the heading-less doc stays "Manuscript".
    #[test]
    fn heading_less_preamble_mining_matches_across_formats() {
        use super::super::parse_markdown;

        // The same title page, no chapter heading, expressed in each format.
        let md = "by A. Writer\n\nThe night was long.\n";
        let docx_body = format!(
            "{}{}",
            "<w:p><w:r><w:t>by A. Writer</w:t></w:r></w:p>",
            "<w:p><w:r><w:t>The night was long.</w:t></w:r></w:p>",
        );

        let md_out = parse_markdown(md);
        let docx_out = parse_document_xml(&document(&docx_body), None);

        // Same author policy: the by-line is mined even with no chapters.
        assert_eq!(md_out.author.as_deref(), Some("A. Writer"));
        assert_eq!(docx_out.author.as_deref(), Some("A. Writer"));
        assert_eq!(md_out.author, docx_out.author, "author policy is unified");
        // Same chapter shape: one fallback chapter, no title invented from prose.
        assert_eq!(md_out.title, None);
        assert_eq!(docx_out.title, None);
        assert_eq!(md_out.chapters.len(), 1);
        assert_eq!(docx_out.chapters.len(), 1);
        assert_eq!(md_out.chapters[0].title, "Manuscript");
        assert_eq!(docx_out.chapters[0].title, "Manuscript");
    }

    #[test]
    fn docx_decodes_xml_entities_and_ignores_bold_off() {
        let body = "<w:p><w:r><w:rPr><w:b w:val=\"false\"/></w:rPr>\
                    <w:t>5 &lt; 6 &amp; 7 &#62; 4</w:t></w:r></w:p>";
        let out = parse_document_xml(&document(body), None);
        // Entities decoded; w:val="false" means the run isn't actually bold.
        assert_eq!(out.chapters[0].body, "5 < 6 & 7 > 4\n");
        assert!(!out.chapters[0].body.contains('*'));
    }

    #[test]
    fn round_trips_a_quill_docx_export() {
        use crate::export::docx::{to_docx, Profile};
        use crate::export::{Chapter, Document, Part, PubMeta};
        let doc = Document {
            title: "Round Trip".into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: PubMeta::default(),
            epigraph: None,
            half_title: false,
            series_books: Vec::new(),
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![
                    Chapter {
                        title: "One".into(),
                        epigraph: None,
                        body: "The night was *dark*.".into(),
                        role: None,
                    },
                    Chapter {
                        title: "Two".into(),
                        epigraph: None,
                        body: "The **end**.".into(),
                        role: None,
                    },
                ],
            }],
            back: vec![],
        };
        // The Vellum profile tags chapter titles as Heading 1 — what the importer splits on.
        let bytes = to_docx(&doc, Profile::Vellum).unwrap();
        let out = parse_docx(&bytes).unwrap();
        let titles: Vec<&str> = out.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["One", "Two"], "chapters round-trip");
        assert_eq!(
            out.chapters[0].body, "The night was *dark*.\n",
            "italic preserved"
        );
        assert_eq!(out.chapters[1].body, "The **end**.\n", "bold preserved");
    }

    #[test]
    fn parse_docx_rejects_a_non_docx() {
        assert!(parse_docx(b"this is not a zip").is_err());
    }

    #[test]
    fn docx_clean_document_reports_nothing_dropped() {
        let body = format!("{}{}", heading("One"), heading("Two"));
        let out = parse_document_xml(&document(&body), None);
        assert_eq!(
            out.dropped,
            Dropped::default(),
            "a clean document drops nothing"
        );
    }

    #[test]
    fn docx_footnotes_become_inline_markers_and_are_counted() {
        // Two footnote references in the prose (the note bodies live in
        // word/footnotes.xml, which the scanner doesn't read).
        let body = format!(
            "{}{}",
            heading("One"),
            "<w:p><w:r><w:t xml:space=\"preserve\">First claim</w:t></w:r>\
             <w:r><w:footnoteReference w:id=\"1\"/></w:r>\
             <w:r><w:t xml:space=\"preserve\"> and a second</w:t></w:r>\
             <w:r><w:footnoteReference w:id=\"2\"/></w:r>\
             <w:r><w:t>.</w:t></w:r></w:p>",
        );
        let out = parse_document_xml(&document(&body), None);
        // Footnotes are converted to inline `[^N]` markers in reading order, not
        // dropped.
        assert_eq!(out.chapters[0].body, "First claim[^1] and a second[^2].\n");
        assert_eq!(out.dropped.footnotes, 2);
        assert_eq!(out.dropped.notes(), 2);
        assert_eq!(out.dropped.lost(), 0, "footnotes are converted, not lost");
    }

    #[test]
    fn docx_endnotes_become_inline_markers_and_are_counted() {
        let body = format!(
            "{}{}",
            heading("One"),
            "<w:p><w:r><w:t xml:space=\"preserve\">A point</w:t></w:r>\
             <w:r><w:endnoteReference w:id=\"1\"/></w:r>\
             <w:r><w:t>.</w:t></w:r></w:p>",
        );
        let out = parse_document_xml(&document(&body), None);
        assert_eq!(out.chapters[0].body, "A point[^e1].\n");
        assert_eq!(out.dropped.endnotes, 1);
    }

    #[test]
    fn extract_comments_pairs_each_anchor_with_its_note() {
        // Two commented ranges in one paragraph, their notes in comments.xml.
        let document = document(
            "<w:p><w:r><w:t xml:space=\"preserve\">The </w:t></w:r>\
             <w:commentRangeStart w:id=\"0\"/>\
             <w:r><w:t>grey sea</w:t></w:r>\
             <w:commentRangeEnd w:id=\"0\"/><w:r><w:commentReference w:id=\"0\"/></w:r>\
             <w:r><w:t xml:space=\"preserve\"> was </w:t></w:r>\
             <w:commentRangeStart w:id=\"1\"/>\
             <w:r><w:t>very cold</w:t></w:r>\
             <w:commentRangeEnd w:id=\"1\"/><w:r><w:commentReference w:id=\"1\"/></w:r></w:p>",
        );
        let comments = "<w:comments>\
             <w:comment w:id=\"0\"><w:p><w:r><w:t>vivid</w:t></w:r></w:p></w:comment>\
             <w:comment w:id=\"1\"><w:p><w:r><w:t>redundant?</w:t></w:r></w:p></w:comment>\
             </w:comments>";
        let out = extract_comments_from_xml(&document, Some(comments));
        assert_eq!(
            out,
            vec![
                ImportedComment {
                    phrase: "grey sea".into(),
                    note: "vivid".into(),
                    reviewer: None,
                },
                ImportedComment {
                    phrase: "very cold".into(),
                    note: "redundant?".into(),
                    reviewer: None,
                },
            ]
        );
    }

    #[test]
    fn extract_comments_carries_the_reviewers_name() {
        // A comment authored by a named reviewer (`w:author`) carries that name;
        // an author-less comment stays unattributed (#933).
        let document = document(
            "<w:p>\
             <w:commentRangeStart w:id=\"0\"/><w:r><w:t>grey sea</w:t></w:r>\
             <w:commentRangeEnd w:id=\"0\"/><w:r><w:commentReference w:id=\"0\"/></w:r>\
             <w:r><w:t xml:space=\"preserve\"> and </w:t></w:r>\
             <w:commentRangeStart w:id=\"1\"/><w:r><w:t>cold</w:t></w:r>\
             <w:commentRangeEnd w:id=\"1\"/><w:r><w:commentReference w:id=\"1\"/></w:r></w:p>",
        );
        let comments = "<w:comments>\
             <w:comment w:id=\"0\" w:author=\"Jane Editor\"><w:p><w:r><w:t>vivid</w:t></w:r></w:p></w:comment>\
             <w:comment w:id=\"1\"><w:p><w:r><w:t>redundant?</w:t></w:r></w:p></w:comment>\
             </w:comments>";
        let out = extract_comments_from_xml(&document, Some(comments));
        assert_eq!(out[0].reviewer.as_deref(), Some("Jane Editor"));
        assert_eq!(
            out[1].reviewer, None,
            "an author-less comment is unattributed"
        );
    }

    #[test]
    fn extract_comments_ignores_quills_own_self_attribution() {
        // Quill's review-copy export stamps "Author annotation" on every comment;
        // importing its own copy back leaves the note author-authored (#933).
        let document = document(
            "<w:p><w:commentRangeStart w:id=\"0\"/><w:r><w:t>grey sea</w:t></w:r>\
             <w:commentRangeEnd w:id=\"0\"/><w:r><w:commentReference w:id=\"0\"/></w:r></w:p>",
        );
        let comments = "<w:comments>\
             <w:comment w:id=\"0\" w:author=\"Author annotation\"><w:p><w:r><w:t>note</w:t></w:r></w:p></w:comment>\
             </w:comments>";
        let out = extract_comments_from_xml(&document, Some(comments));
        assert_eq!(out[0].reviewer, None);
    }

    #[test]
    fn extract_comments_is_empty_with_no_comments_part() {
        let document = document("<w:p><w:r><w:t>Plain prose.</w:t></w:r></w:p>");
        assert!(extract_comments_from_xml(&document, None).is_empty());
    }

    #[test]
    fn extract_comments_skips_an_anchor_with_no_note() {
        // A range marker whose id has no comments.xml entry yields nothing.
        let document = document(
            "<w:p><w:commentRangeStart w:id=\"7\"/>\
             <w:r><w:t>orphan</w:t></w:r>\
             <w:commentRangeEnd w:id=\"7\"/></w:p>",
        );
        assert!(extract_comments_from_xml(&document, Some("<w:comments/>")).is_empty());
    }

    #[test]
    fn extract_comments_recovers_the_review_writers_output() {
        // The inverse of the #834 review-copy writer: build a review `.docx` whose
        // annotations become Word comments, then recover them.
        use crate::export::docx::{to_docx_review, ChapterNotes, ReviewNote};
        use crate::export::{Chapter, Document, Part, PubMeta};
        let doc = Document {
            title: "Round Trip".into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: PubMeta::default(),
            epigraph: None,
            half_title: false,
            series_books: Vec::new(),
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![Chapter {
                    title: "One".into(),
                    epigraph: None,
                    body: "The sea was grey and it ended.".into(),
                    role: None,
                }],
            }],
            back: vec![],
        };
        let notes: Vec<ChapterNotes> = vec![vec![
            ReviewNote {
                text: "grey".into(),
                note: "vivid".into(),
            },
            ReviewNote {
                text: "it ended".into(),
                note: "abrupt".into(),
            },
        ]];
        let bytes = to_docx_review(&doc, &notes).unwrap();
        let out = extract_comments(&bytes).unwrap();
        assert_eq!(
            out,
            vec![
                ImportedComment {
                    phrase: "grey".into(),
                    note: "vivid".into(),
                    reviewer: None,
                },
                ImportedComment {
                    phrase: "it ended".into(),
                    note: "abrupt".into(),
                    reviewer: None,
                },
            ],
            "each annotation round-trips to a comment on its phrase (Quill's own \
             self-attribution is not treated as a named reviewer)"
        );
    }

    #[test]
    fn docx_images_tables_and_comments_are_counted_as_dropped() {
        let body = format!(
            "{}{}{}{}",
            heading("One"),
            // An inline image (a drawing) and a comment reference in a paragraph.
            "<w:p><w:r><w:t>See figure</w:t></w:r>\
             <w:r><w:drawing><wp:inline/></w:drawing></w:r>\
             <w:r><w:commentReference w:id=\"0\"/></w:r></w:p>",
            // A one-cell table (its cell paragraph text still leaks through, as
            // before — but the table itself is now counted as dropped).
            "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>",
            // A legacy VML image.
            "<w:p><w:r><w:pict><v:shape/></w:pict></w:r></w:p>",
        );
        let out = parse_document_xml(&document(&body), None);
        assert_eq!(out.dropped.images, 2, "the drawing and the VML pict");
        assert_eq!(out.dropped.tables, 1);
        assert_eq!(out.dropped.comments, 1);
        assert_eq!(out.dropped.footnotes, 0);
        assert_eq!(out.dropped.lost(), 4, "2 images + 1 table + 1 comment");
    }
}
