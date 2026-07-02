//! Word `.docx` import (#401): a `.docx` is a zip of XML, so read
//! `word/document.xml` (and the OOXML core properties) and split chapters on
//! Heading 1 paragraphs, turning bold/italic runs into `**`/`*`. See the crate
//! module doc for the rules; the shared XML scanner lives in `mod.rs`.

use std::io::{Cursor, Read};

use super::{
    assemble, attr_value, count_elements, decode_entities, element_text, fallback_chapter,
    finalize_titles, for_each_element, join_paragraphs, mine_preamble, tag_attrs, wrap_emphasis,
    Dropped, Imported,
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
        out.push(match inner {
            Some(p) => parse_paragraph(p, dropped),
            None => Para {
                heading: false,
                text: String::new(),
            },
        });
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
        out.push_str(&format!("[^{}]", dropped.footnotes));
    }
    for _ in 0..count_elements(run, "w:endnoteReference") {
        dropped.endnotes += 1;
        out.push_str(&format!("[^e{}]", dropped.endnotes));
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
    match tag_attrs(run, name) {
        None => false,
        Some(attrs) => match attr_value(attrs, "w:val") {
            Some(v) => !matches!(v.as_str(), "false" | "0" | "none" | "off"),
            None => true,
        },
    }
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
        use crate::export::{Chapter, Document, Part};
        let doc = Document {
            title: "Round Trip".into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: Default::default(),
            epigraph: None,
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
