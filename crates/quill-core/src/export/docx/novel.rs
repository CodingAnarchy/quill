//! The novel submission manuscript (#1302): standard manuscript format, as an
//! agent or editor expects it, rather than a finished book's front matter. The
//! title page carries the author's contact block top-left with the rounded word
//! count at the right, and the title and byline about halfway down. There's no
//! half-title, copyright page, series list, or contents. Each chapter starts on a
//! new page about a third of the way down, scene breaks are a centered `#`, a
//! centered "END" closes the novel ahead of any back matter, and the running
//! header reads `Surname / Short title / page`.

use super::body::{epigraph_paras, para, prose_paragraph, run, wrap_document};
use super::story::{contact_block, story_header};
use super::{prose_blocks, Document};
use crate::export::{Chapter, StoryMeta};

/// How far the title sits below the contact block: about halfway down the page.
const TITLE_DROP: &str = "5040";

/// How far a chapter title sits down its page: about a third of the way.
const CHAPTER_DROP: &str = "2880";

/// The `word/document.xml` for a novel's submission manuscript.
pub(super) fn novel_document_xml(doc: &Document, novel: &StoryMeta) -> String {
    let mut body = contact_block(novel);
    body.push_str(&para(
        &format!(r#"<w:spacing w:before="{TITLE_DROP}" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#),
        run(&doc.title, false, false, None),
    ));
    if let Some(subtitle) = &doc.subtitle {
        body.push_str(&para(
            r#"<w:spacing w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
            run(subtitle, false, false, None),
        ));
    }
    if let Some(author) = &doc.author {
        body.push_str(&para(
            r#"<w:spacing w:before="240" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/>"#,
            run(&format!("by {author}"), false, false, None),
        ));
    }
    // The book's epigraph is the author's, so it stays: on its own page ahead
    // of the first chapter, where a title page has no room for it.
    if let Some(epigraph) = &doc.epigraph {
        body.push_str(&para(
            &format!(r#"<w:pageBreakBefore/><w:spacing w:before="{CHAPTER_DROP}"/>"#),
            String::new(),
        ));
        body.push_str(&epigraph_paras(epigraph));
    }
    for chapter in &doc.front {
        body.push_str(&chapter_xml(chapter));
    }
    for part in &doc.parts {
        if let Some(name) = &part.name {
            body.push_str(&heading(name));
        }
        for chapter in &part.chapters {
            body.push_str(&chapter_xml(chapter));
        }
    }
    body.push_str(&para(
        r#"<w:spacing w:before="480"/><w:jc w:val="center"/>"#,
        run("END", false, false, None),
    ));
    // Acknowledgments, an afterword: after the novel, each on its own page.
    for chapter in &doc.back {
        body.push_str(&chapter_xml(chapter));
    }
    wrap_document(&body, &story_header(doc))
}

/// A chapter or section: its title on a new page, its epigraph, its prose.
fn chapter_xml(chapter: &Chapter) -> String {
    let mut out = heading(&chapter.title);
    if let Some(epigraph) = &chapter.epigraph {
        out.push_str(&epigraph_paras(epigraph));
    }
    for prose in prose_blocks(&chapter.body) {
        out.push_str(&prose_paragraph(&prose));
    }
    out
}

/// A part or chapter title, centered on a fresh page about a third of the way
/// down. Outline level 1 keeps it in Word's navigation pane.
fn heading(title: &str) -> String {
    para(
        &format!(
            r#"<w:pageBreakBefore/><w:spacing w:before="{CHAPTER_DROP}" w:after="480" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/><w:outlineLvl w:val="0"/>"#
        ),
        run(title, false, false, None),
    )
}
