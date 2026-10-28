//! The short story exports (#1241): the Word manuscript's submission layout,
//! and every other writer leaving out the story's chapter heading and a
//! one-entry contents. Novels are covered by each writer's own tests.

use std::io::{Cursor, Read};

use super::docx::{to_docx, Profile};
use super::epub::to_epub;
use super::pdf::to_pdf;
use super::{
    approximate_words, to_markdown, Chapter, Document, ExportStyle, Part, PubMeta, StoryMeta,
};
use crate::config::AuthorConfig;
use crate::frontmatter::Role;

const PROSE: &str = "The bell rang at low tide.\n\n* * *\n\nNobody *came*.\n";

/// A short story: one untitled body chapter, plus a dedication.
fn story() -> Document {
    Document {
        title: "The Drowned Bell".into(),
        subtitle: None,
        author: Some("Mara Quill".into()),
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        half_title: false,
        series_books: Vec::new(),
        front: vec![Chapter {
            title: "For M".into(),
            epigraph: None,
            body: "For M.\n".into(),
            role: Some(Role::Dedication),
        }],
        parts: vec![Part {
            name: None,
            chapters: vec![Chapter {
                title: "The Drowned Bell".into(),
                epigraph: None,
                body: PROSE.into(),
                role: None,
            }],
        }],
        back: Vec::new(),
        story: Some(StoryMeta {
            contact: vec![
                "Mara Quill".into(),
                "12 Harbour Row".into(),
                "Portsea, PO1 2AB".into(),
                "mara@example.com".into(),
            ],
            words: 3_456,
        }),
    }
}

/// One part of a zip (a `.docx` or `.epub`) as text.
fn zip_part(bytes: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut out).unwrap();
    out
}

/// Every part name in a zip.
fn zip_names(bytes: &[u8]) -> Vec<String> {
    let zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    zip.file_names().map(str::to_string).collect()
}

#[test]
fn word_counts_round_the_way_submissions_state_them() {
    assert_eq!(approximate_words(3_456), "about 3,500 words");
    assert_eq!(approximate_words(3_449), "about 3,400 words");
    assert_eq!(approximate_words(12_960), "about 13,000 words");
    assert_eq!(approximate_words(734), "about 730 words");
    assert_eq!(approximate_words(3), "about 10 words", "never 'about 0'");
}

#[test]
fn contact_lines_fall_back_to_the_byline() {
    let none = AuthorConfig::default();
    assert_eq!(none.contact_lines(Some("Mara Quill")), vec!["Mara Quill"]);
    assert!(none.contact_lines(None).is_empty());
    let full = AuthorConfig {
        name: Some("Mara A. Quill".into()),
        address: vec!["12 Harbour Row".into(), " ".into()],
        email: Some("mara@example.com".into()),
        phone: None,
    };
    assert_eq!(
        full.contact_lines(Some("Mara Quill")),
        vec!["Mara A. Quill", "12 Harbour Row", "mara@example.com"]
    );
}

#[test]
fn the_word_manuscript_takes_the_submission_layout() {
    let bytes = to_docx(&story(), Profile::Manuscript).unwrap();
    let xml = zip_part(&bytes, "word/document.xml");
    // Contact block and rounded count on page one, then the title and byline.
    let contact = xml.find("12 Harbour Row").unwrap();
    let count = xml.find("about 3,500 words").unwrap();
    let title = xml.find("The Drowned Bell").unwrap();
    let byline = xml.find("by Mara Quill").unwrap();
    let prose = xml.find("The bell rang at low tide.").unwrap();
    assert!(count < contact && contact < title && title < byline && byline < prose);
    // No contents, and no page break anywhere before the story ends.
    assert!(!xml.contains("TOC \\o"), "no table of contents");
    let end = xml.find(">END<").expect("an END line closes the story");
    assert!(
        !xml[..end].contains("pageBreakBefore"),
        "the story starts on page one"
    );
    // The title appears once (the title block), never as a chapter heading.
    assert_eq!(xml.matches("The Drowned Bell").count(), 1);
    // Scenes split on a centered `#`.
    assert!(xml.contains(">#<"));
    // The dedication follows the story on its own page.
    assert!(xml.find("For M.").unwrap() > end);
    let header = zip_part(&bytes, "word/header1.xml");
    assert!(header.contains("Quill / ") && header.contains("The Drowned Bell / "));
}

#[test]
fn the_word_manuscript_works_with_no_contact_details() {
    let mut doc = story();
    doc.story = Some(StoryMeta {
        contact: Vec::new(),
        words: 800,
    });
    let xml = zip_part(
        &to_docx(&doc, Profile::Manuscript).unwrap(),
        "word/document.xml",
    );
    assert!(xml.contains("about 800 words"));
}

#[test]
fn markdown_does_not_repeat_the_title_as_a_chapter_heading() {
    let md = to_markdown(&story());
    assert_eq!(md.matches("The Drowned Bell").count(), 1, "{md}");
    assert!(
        md.contains("## For M"),
        "a dedication keeps its heading: {md}"
    );
    assert!(md.contains("Nobody *came*."));
}

#[test]
fn the_epub_gives_the_story_no_chapter_heading() {
    let bytes = to_epub(&story(), "2026-10-27T00:00:00Z", &ExportStyle::default()).unwrap();
    let chapters: Vec<String> = zip_names(&bytes)
        .into_iter()
        .filter(|n| n.contains("chap"))
        .map(|n| zip_part(&bytes, &n))
        .collect();
    let story_page = chapters
        .iter()
        .find(|c| c.contains("The bell rang"))
        .expect("the story's page");
    assert!(!story_page.contains("<h1>"), "{story_page}");
    assert!(chapters.iter().any(|c| c.contains("<h1>For M</h1>")));
}

#[test]
fn the_pdf_has_no_contents_page_for_a_story() {
    let style = ExportStyle::default();
    let story_pages = count_pages(&to_pdf(&story(), &style).unwrap());
    let mut novel = story();
    novel.story = None;
    let novel_pages = count_pages(&to_pdf(&novel, &style).unwrap());
    assert_eq!(
        story_pages + 1,
        novel_pages,
        "the novel's extra page is its contents"
    );
}

fn count_pages(pdf: &[u8]) -> usize {
    pdf.windows(b"/Type /Page /Parent".len())
        .filter(|w| *w == b"/Type /Page /Parent")
        .count()
}
