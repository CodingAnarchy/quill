//! Tests for the EPUB writer (#297): package structure, XHTML rendering,
//! `[style]` parity, embedded/subset fonts, images, `epub:type` semantics, and
//! landmarks. Split by concern (#928) into submodules to keep each file under
//! the ~1,000-line convention (#716); shared fixtures/helpers live here and
//! each submodule pulls them in with `use super::*`. Pure move — no behavior
//! change.

use super::*;
use crate::export::{Chapter, Document, ExportStyle, Part};
use crate::frontmatter::{Role, SERIES_LIST_HEADING};
use std::io::Read;

mod content;
mod fonts;
mod images;
mod package;
mod semantics;

const MODIFIED: &str = "2026-06-27T00:00:00Z";

fn epigraph(text: &str, by: Option<&str>) -> Epigraph {
    Epigraph {
        text: text.to_string(),
        attribution: by.map(str::to_string),
    }
}

fn chapter(title: &str, epigraph: Option<Epigraph>, body: &str) -> Chapter {
    Chapter {
        title: title.to_string(),
        epigraph,
        body: body.to_string(),
        role: None,
    }
}

/// A front/back-matter section carrying a specific [`Role`] (#695), so a test
/// can assert its `epub:type` reaches the output.
fn section(title: &str, role: Role, body: &str) -> Chapter {
    Chapter {
        title: title.to_string(),
        epigraph: None,
        body: body.to_string(),
        role: Some(role),
    }
}

fn doc() -> Document {
    Document {
        title: "Red <State> Earth".into(),
        subtitle: Some("a novel".into()),
        author: Some("A. Writer".into()),
        series: Some("The Cycle".into()),
        publishing: PubMeta::default(),
        epigraph: Some(epigraph("In the beginning.", Some("Anon"))),
        half_title: false,
        series_books: Vec::new(),
        front: vec![chapter("Prologue", None, "Before it all.")],
        parts: vec![Part {
            name: Some("Part One".into()),
            chapters: vec![chapter(
                "The Find",
                Some(epigraph("A line.", None)),
                "The locket *gleamed* and felt **heavy**.\n\n## A scene\n\nScene one.\n\n---\n\nScene two.",
            )],
        }],
        back: vec![chapter("Afterword", None, "After.")],
        story: None,
    }
}

/// Read one zip entry's text by name.
fn part(bytes: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
    let mut file = zip.by_name(name).expect("entry present");
    let mut s = String::new();
    file.read_to_string(&mut s).unwrap();
    s
}

fn names(bytes: &[u8]) -> Vec<String> {
    let zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
    zip.file_names().map(str::to_string).collect()
}

/// A lightweight well-formedness assertion for a rendered XHTML string, without
/// pulling in an XML-parser dependency. Inside each tag it checks every
/// attribute value is delimited by a matching quote with no raw quote of the
/// delimiting kind in between — the exact failure mode a raw `"` in an `alt`
/// value causes (the attribute closes early, leaving stray text before `>`). A
/// raw `<` inside an attribute value is also caught.
fn assert_well_formed_xml(xhtml: &str) {
    let bytes = xhtml.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // Skip comments/CDATA/PI/doctype wholesale — they don't carry the
        // attribute quoting we're validating.
        if xhtml[i..].starts_with("<!") || xhtml[i..].starts_with("<?") {
            let end = xhtml[i..].find('>').expect("markup declaration is closed");
            i += end + 1;
            continue;
        }
        // Inside a tag: scan attributes until the closing `>`.
        i += 1;
        while i < bytes.len() && bytes[i] != b'>' {
            let c = bytes[i];
            if c == b'"' || c == b'\'' {
                let quote = c;
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    assert!(
                        bytes[i] != b'<',
                        "raw '<' inside an attribute value: {xhtml}"
                    );
                    i += 1;
                }
                assert!(
                    i < bytes.len(),
                    "unterminated attribute value (a raw quote closed it early): {xhtml}"
                );
                i += 1; // consume the closing quote
            } else {
                i += 1;
            }
        }
        assert!(i < bytes.len(), "unclosed tag: {xhtml}");
        i += 1; // consume '>'
    }
}

/// Read one package entry as raw bytes (fonts and images aren't UTF-8).
fn part_bytes(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
    let mut file = zip.by_name(name).expect("entry present");
    let mut out = Vec::new();
    file.read_to_end(&mut out).unwrap();
    out
}
