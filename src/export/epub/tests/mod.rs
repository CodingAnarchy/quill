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

/// Read one package entry as raw bytes (fonts and images aren't UTF-8).
fn part_bytes(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
    let mut file = zip.by_name(name).expect("entry present");
    let mut out = Vec::new();
    file.read_to_end(&mut out).unwrap();
    out
}
