//! The novel's Word manuscript (#1302): standard manuscript format, with a
//! submission title page and none of a finished book's front matter.

use std::io::{Cursor, Read};

use super::docx::{to_docx, Profile};
use super::{Chapter, Document, Part, PubMeta, StoryMeta};
use crate::frontmatter::{Epigraph, Role};

/// A novel with a dedication, a named part of two chapters, acknowledgments, a
/// book epigraph, and an ISBN.
fn novel() -> Document {
    let chapter = |title: &str, body: &str, role: Option<Role>| Chapter {
        title: title.into(),
        epigraph: None,
        body: body.into(),
        role,
    };
    Document {
        title: "The Lamp Room".into(),
        subtitle: None,
        author: Some("Mara Vey".into()),
        series: None,
        publishing: PubMeta {
            isbn: Some("9781402894626".into()),
            ..PubMeta::default()
        },
        epigraph: Some(Epigraph {
            text: "Some of us are looking at the stars.".into(),
            attribution: Some("Oscar Wilde".into()),
        }),
        half_title: true,
        series_books: Vec::new(),
        front: vec![chapter(
            "Dedication",
            "For my father.\n",
            Some(Role::Dedication),
        )],
        parts: vec![Part {
            name: Some("The Shore".into()),
            chapters: vec![
                chapter(
                    "Low Tide",
                    "The boat had gone.\n\n* * *\n\nIt was *gone*.\n",
                    None,
                ),
                chapter("The Ledger", "She found it.\n", None),
            ],
        }],
        back: vec![chapter(
            "Acknowledgments",
            "Thanks to the keepers.\n",
            Some(Role::Acknowledgments),
        )],
        story: None,
        novel: Some(StoryMeta {
            contact: vec![
                "Mara Vey".into(),
                "12 Harbour Row".into(),
                "mara@example.com".into(),
            ],
            words: 85_449,
        }),
    }
}

/// One part of the `.docx` as text.
fn docx_part(bytes: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("a zip");
    let mut out = String::new();
    zip.by_name(name)
        .expect("the part is in the package")
        .read_to_string(&mut out)
        .expect("the part is text");
    out
}

#[test]
fn the_title_page_carries_the_contact_block_and_word_count() {
    let bytes = to_docx(&novel(), Profile::Manuscript).expect("export");
    let xml = docx_part(&bytes, "word/document.xml");
    let count = xml
        .find("about 85,400 words")
        .expect("the rounded word count");
    let contact = xml.find("12 Harbour Row").expect("the contact block");
    let title = xml.find("The Lamp Room").expect("the title");
    let byline = xml.find("by Mara Vey").expect("the byline");
    assert!(
        count < contact && contact < title && title < byline,
        "{xml}"
    );
    // The title page is page one: nothing breaks a page before the byline.
    assert!(!xml[..byline].contains("pageBreakBefore"), "{xml}");
}

#[test]
fn a_finished_books_front_matter_is_left_out() {
    let bytes = to_docx(&novel(), Profile::Manuscript).expect("export");
    let xml = docx_part(&bytes, "word/document.xml");
    assert!(!xml.contains("9781402894626"), "no copyright page");
    assert!(
        !xml.contains("TOC \\o") && !xml.contains(">Contents<"),
        "no contents"
    );
    // The half-title would repeat the title ahead of the title page.
    assert_eq!(xml.matches(">The Lamp Room<").count(), 1, "{xml}");
}

#[test]
fn chapters_start_new_pages_and_end_closes_the_novel() {
    let bytes = to_docx(&novel(), Profile::Manuscript).expect("export");
    let xml = docx_part(&bytes, "word/document.xml");
    // The epigraph, the dedication, the part, and each chapter and section
    // open a page.
    assert_eq!(xml.matches("<w:pageBreakBefore/>").count(), 6, "{xml}");
    let epigraph = xml.find("looking at the stars").expect("the book epigraph");
    let dedication = xml.find("For my father.").expect("the dedication");
    let last = xml.find("She found it.").expect("the last chapter");
    let end = xml.find(">END<").expect("END closes the novel");
    let thanks = xml
        .find("Thanks to the keepers.")
        .expect("the acknowledgments");
    assert!(epigraph < dedication && dedication < last && last < end && end < thanks);
    assert!(xml.contains(">#<"), "scenes split on a centered #");
    assert!(xml.contains("<w:i/>"), "emphasis as italics");
}

#[test]
fn the_running_header_names_the_surname_and_title() {
    let bytes = to_docx(&novel(), Profile::Manuscript).expect("export");
    let header = docx_part(&bytes, "word/header1.xml");
    assert!(
        header.contains(">Vey / <") && header.contains(">The Lamp Room / <"),
        "{header}"
    );
    assert!(!header.contains("Mara"), "the surname only: {header}");
}

#[test]
fn the_vellum_word_file_is_unchanged() {
    // The Vellum file feeds Vellum's own design, not an agent.
    let bytes = to_docx(&novel(), Profile::Vellum).expect("export");
    let xml = docx_part(&bytes, "word/document.xml");
    assert!(!xml.contains("about 85,400 words"), "{xml}");
    assert!(!xml.contains(">END<"), "{xml}");
}
