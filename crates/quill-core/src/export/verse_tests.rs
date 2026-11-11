//! Verse chapters keep their lines on export (#1327).

use std::io::{Cursor, Read};
use std::path::Path;

use super::docx::{to_docx, Profile};
use super::verse::{markdown_body, stanza, VerseLine};
use super::{
    classify_block, epub, to_markdown, Chapter, Document, ExportStyle, Part, Prose, PubMeta,
};

const POEM: &str = "The tide came in across the stone\n  and left the harbour wall alone,\n\nThe lamps were *lit* along the quay\n    and no one sailed\n\n* * *\n\nA last line that runs on long enough to need to wrap across the width of a printed page somewhere.\n";

fn chapter(title: &str, body: &str, verse: bool) -> Chapter {
    Chapter {
        title: title.into(),
        epigraph: None,
        body: body.into(),
        role: None,
        verse,
    }
}

fn book(verse: bool) -> Document {
    Document {
        title: "Harbour".into(),
        subtitle: None,
        author: Some("Mara Vey".into()),
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        half_title: false,
        series_books: Vec::new(),
        front: Vec::new(),
        parts: vec![Part {
            name: None,
            chapters: vec![
                chapter("The Tide", POEM, verse),
                chapter("After", "Prose runs on\nacross a soft wrap.\n", false),
            ],
        }],
        back: Vec::new(),
        story: None,
        novel: None,
    }
}

#[test]
fn a_stanza_keeps_its_lines_and_indents() {
    assert_eq!(
        stanza("one\n  two\n\tthree\n                     deep"),
        [
            VerseLine {
                indent: 0,
                text: "one".into()
            },
            VerseLine {
                indent: 1,
                text: "two".into()
            },
            VerseLine {
                indent: 2,
                text: "three".into()
            },
            VerseLine {
                indent: 8,
                text: "deep".into()
            },
        ]
    );
    match classify_block("one\ntwo", true) {
        Some(Prose::Stanza(lines)) => assert_eq!(lines.len(), 2),
        other => panic!("a stanza, got {other:?}"),
    }
    // Structure is still structure in a verse chapter.
    assert!(matches!(
        classify_block("* * *", true),
        Some(Prose::SceneBreak)
    ));
    assert!(matches!(
        classify_block("## Part", true),
        Some(Prose::Heading(_))
    ));
    match classify_block("one\ntwo", false) {
        Some(Prose::Paragraph(text)) => assert_eq!(text, "one two"),
        other => panic!("a paragraph, got {other:?}"),
    }
}

#[test]
fn markdown_keeps_verse_lines_with_hard_breaks() {
    assert_eq!(
        markdown_body("a\nb\n\n# H\nc\nd\n\n```\nx\ny\n```\n"),
        "a\\\nb\n\n# H\nc\\\nd\n\n```\nx\ny\n```\n"
    );
    let md = to_markdown(&book(true));
    assert!(md.contains("across the stone\\\n  and left"), "{md}");
    assert!(
        md.contains("Prose runs on\nacross a soft wrap."),
        "prose is untouched: {md}"
    );
    assert!(!to_markdown(&book(false)).contains('\\'));
}

/// The EPUB's chapter files, concatenated.
fn epub_text(doc: &Document) -> String {
    let bytes = epub::to_epub(doc, "2026-11-11T00:00:00Z", &ExportStyle::default()).expect("epub");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("a zip");
    let mut out = String::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).expect("an entry");
        let name = file.name().expect("a name").to_string();
        let ext = Path::new(&name).extension().and_then(|e| e.to_str());
        if matches!(ext, Some("xhtml" | "css")) {
            file.read_to_string(&mut out).expect("text");
        }
    }
    out
}

#[test]
fn the_epub_sets_a_paragraph_a_line_with_a_hanging_indent() {
    let text = epub_text(&book(true));
    assert!(
        text.contains("<div class=\"stanza\"><p>The tide came in across the stone</p><p class=\"i1\">and left the harbour wall alone,</p></div>"),
        "{text}"
    );
    assert!(text.contains(
        "<p>The lamps were <em>lit</em> along the quay</p><p class=\"i2\">and no one sailed</p>"
    ));
    assert!(text.contains(".stanza p { text-indent: -2em; margin: 0 0 0 2em;"));
    assert!(
        text.contains("<p>Prose runs on across a soft wrap.</p>"),
        "prose still joins"
    );
    assert!(!epub_text(&book(false)).contains("class=\"stanza\""));
}

/// The Word file's body XML.
fn docx_body(doc: &Document, profile: Profile) -> String {
    let bytes = to_docx(doc, profile).expect("docx");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("a zip");
    let mut out = String::new();
    zip.by_name("word/document.xml")
        .expect("the body")
        .read_to_string(&mut out)
        .expect("text");
    out
}

#[test]
fn word_sets_a_paragraph_a_line_with_a_hanging_indent() {
    for profile in [Profile::Manuscript, Profile::Vellum] {
        let xml = docx_body(&book(true), profile);
        let first = xml
            .find(">The tide came in across the stone<")
            .expect("line one");
        let second = xml
            .find(">and left the harbour wall alone,<")
            .expect("line two");
        let between = &xml[first..second];
        assert!(between.contains("</w:p>"), "each line is its own paragraph");
        assert!(
            xml.contains(r#"<w:ind w:left="1080" w:hanging="720"/>"#),
            "an indented line"
        );
        assert!(xml.contains(r#"<w:ind w:left="720" w:hanging="720"/>"#));
        assert!(
            xml.contains(">Prose runs on across a soft wrap.<"),
            "prose still joins"
        );
    }
}
