//! The series/omnibus driver for the `.docx` writer: compile many books into one
//! document (#332), with a series title page and series-level copyright, then each
//! book page-broken under its own title with its own reprinted copyright (#711).

use super::body::{book_body, copyright_page, para, run, toc_page, wrap_document};
use super::package::build_docx;
use super::{copyright_lines, running_header, Document, Profile, PubMeta};

/// Compile a whole series (one book per [`Document`], in order) to one `.docx`
/// (#332): a series title page (manuscript profile), then each book under its own
/// title, page-broken from the last. Single book → the same body as
/// [`to_docx`](super::to_docx).
///
/// # Series publishing metadata (#711)
///
/// An omnibus is one publication, so the front-matter copyright page is
/// series-level — the first book's [`PubMeta`]. Per-book rights/publisher/ISBN
/// are not silently dropped, though: each book's own copyright block is reprinted
/// under its title (manuscript profile), the way a print omnibus reproduces each
/// original edition's copyright. The Vellum profile omits both (it applies its
/// own front matter and splits on Heading 1).
pub fn to_docx_series(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
    profile: Profile,
) -> std::io::Result<Vec<u8>> {
    let mut body = String::new();
    // Series title page (manuscript only — Vellum splits on Heading 1, so stray
    // leading text would import as junk).
    if profile == Profile::Manuscript {
        // A half-title ahead of the omnibus title, when the first book enables it
        // (#820) — the omnibus's own front matter, so it carries no series-list page.
        if books.first().is_some_and(|b| b.half_title) {
            if let Some(series) = series {
                body.push_str(&para(
                    r#"<w:pageBreakBefore/><w:spacing w:before="3600"/><w:jc w:val="center"/>"#,
                    run(series, true, false, Some(28)),
                ));
            }
        }
        if let Some(series) = series {
            body.push_str(&para(
                r#"<w:spacing w:before="2400"/><w:jc w:val="center"/>"#,
                run(series, true, false, Some(40)),
            ));
        }
        if let Some(author) = author {
            body.push_str(&para(
                r#"<w:jc w:val="center"/>"#,
                run(&format!("by {author}"), false, false, None),
            ));
        }
        // The omnibus-level copyright page is the first book's (#711); each book's
        // own copyright is reprinted under its title below.
        if let Some(first) = books.first() {
            body.push_str(&copyright_page(&first.publishing));
        }
        body.push_str(&toc_page());
    }
    for book in books {
        body.push_str(&book_heading(&book.title, profile));
        // A later book's rights/ISBN aren't silently dropped (#711): reprint this
        // book's own copyright block under its title (manuscript profile only —
        // Vellum applies its own front matter).
        if profile == Profile::Manuscript {
            body.push_str(&book_copyright(&book.publishing));
        }
        body.push_str(&book_body(book, profile));
    }
    let header = running_header(author, series, profile);
    build_docx(wrap_document(&body, &header), profile, header)
}

/// A series book's own copyright block (#711), reprinted under its title in a
/// series omnibus so a later book's rights/ISBN aren't silently dropped. Unlike
/// [`copyright_page`], it does not force a fresh page — it sits directly under the
/// page-broken book title — but stacks the same centered lines. Empty when the
/// book has no publishing metadata, leaving the output unchanged.
fn book_copyright(publishing: &PubMeta) -> String {
    let lines = copyright_lines(publishing);
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for line in lines {
        out.push_str(&para(
            r#"<w:spacing w:before="120"/><w:jc w:val="center"/>"#,
            run(&line, false, false, None),
        ));
    }
    out
}

/// A series book title: a page-broken centered line, larger than a chapter
/// heading (manuscript), or a Heading 1 (Vellum, where it reads as a break).
fn book_heading(title: &str, profile: Profile) -> String {
    match profile {
        // Outline level 1 so a series book title also lands in the TOC.
        Profile::Manuscript => para(
            r#"<w:pageBreakBefore/><w:spacing w:before="360" w:after="360" w:line="240" w:lineRule="auto"/><w:jc w:val="center"/><w:outlineLvl w:val="0"/>"#,
            run(title, true, false, Some(32)),
        ),
        Profile::Vellum => para(
            r#"<w:pStyle w:val="Heading1"/>"#,
            run(title, false, false, None),
        ),
    }
}
