//! Word (`.docx`) export in standard manuscript format (#191). A `.docx` is a zip
//! of XML parts; the XML here is hand-written so we control the manuscript
//! formatting precisely — Times New Roman 12pt, double-spaced body with
//! first-line indents, each chapter on a fresh page with its title, epigraphs set
//! apart, and Markdown `*`/`**` carried through as italic/bold runs. Consumes the
//! shared [`Document`] from #190.
//!
//! The writer is split across submodules: [`package`] holds the zip plumbing
//! (content-types, relationships, styles), [`body`] renders `word/document.xml`
//! and the paragraph/run builders, and [`series`] drives the omnibus export
//! (#332). This module holds the entry points and the running-header ([`Header`])
//! shared by both.

mod body;
mod package;
mod review;
mod series;
mod story;
#[cfg(test)]
mod tests;

use self::body::{document_xml, page_field, para, run};
use self::package::{build_docx, header_xml};
use super::{
    copyright_lines, inline_runs, prose_blocks, render_epigraph, xml_escape, Block, Document,
    Prose, PubMeta, SCENE_BREAK,
};

pub use self::review::{to_docx_review, ChapterNotes, ReviewNote};
pub use self::series::to_docx_series;

/// Which export shape a `.docx` takes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Standard manuscript format (#191): a title page, each chapter on a fresh
    /// page under a centered bold title, double-spaced Times New Roman body.
    Manuscript,
    /// Vellum import (#193): chapter and part titles as Heading 1 (Vellum splits
    /// chapters on it), no manuscript title page — Vellum applies its own design.
    Vellum,
}

/// Compile a [`Document`] to the bytes of a `.docx` file in the given [`Profile`].
pub fn to_docx(doc: &Document, profile: Profile) -> std::io::Result<Vec<u8>> {
    let header = match (profile, &doc.story) {
        (Profile::Manuscript, Some(_)) => story::story_header(doc),
        _ => running_header(doc.author.as_deref(), Some(&doc.title), profile),
    };
    build_docx(document_xml(doc, profile), profile, header)
}

/// The running-header content for the manuscript profile, or [`None`] when it
/// carries no header (the Vellum profile, which applies its own).
///
/// Standard manuscript format runs `Author / Title / page #` across the top of
/// every page; the page number is a live `PAGE` field. Either field may be
/// absent (missing author or title), in which case only what's present appears.
type Header = Option<String>;

/// Build the [`Header`] for a profile: the manuscript running header (author,
/// title, and a live page-number field), or [`None`] for Vellum.
fn running_header(author: Option<&str>, title: Option<&str>, profile: Profile) -> Header {
    if profile != Profile::Manuscript {
        return None;
    }
    // Right-aligned `Surname / Title / N` — the page number a live PAGE field so
    // it renumbers when the manuscript reflows.
    let mut runs = String::new();
    if let Some(author) = author {
        runs.push_str(&run(&format!("{author} / "), false, false, None));
    }
    if let Some(title) = title {
        runs.push_str(&run(&format!("{title} / "), false, false, None));
    }
    runs.push_str(&page_field());
    Some(header_xml(&para(r#"<w:jc w:val="right"/>"#, runs)))
}
