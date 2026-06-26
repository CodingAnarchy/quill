//! Import an existing single-Markdown-file manuscript into Quill's chapter model
//! (#298). Pure parsing — no I/O — so it's exhaustively testable: text in, book
//! metadata plus per-chapter bodies out. The app layer creates the project and
//! writes the files; `Project::import_chapters` does the writing.
//!
//! # How it splits
//!
//! A manuscript is split into chapters on ATX headings. The *chapter level* is
//! chosen so both common shapes round-trip:
//!
//! - Quill's own Markdown export (`# Book Title` then `## Chapter`) — one level-1
//!   heading and one-or-more level-2 → split on level 2, the `#` is the title.
//! - A plain export where each chapter is a top-level `# Chapter` — split on
//!   level 1.
//!
//! Headings inside fenced code blocks don't count. Emphasis (`*`/`**`) is already
//! Markdown, so it's preserved verbatim. A leading YAML front-matter block
//! (`---`-fenced) populates the title/author; a Quill-export title page ("by
//! Author") is mined for the author too. With no headings at all, the whole file
//! becomes one chapter. Other formats (`.docx`, `.scriv`) are separate tickets.

use crate::frontmatter;

/// A manuscript parsed into book metadata and chapters.
#[derive(Debug, PartialEq, Eq)]
pub struct Imported {
    /// The book title, from front matter or the title-page `#` heading.
    pub title: Option<String>,
    /// The author, from front matter or a "by …" title-page line.
    pub author: Option<String>,
    /// The chapters, in reading order (always at least one).
    pub chapters: Vec<ImportedChapter>,
}

/// One imported chapter: its heading text and its prose body.
#[derive(Debug, PartialEq, Eq)]
pub struct ImportedChapter {
    pub title: String,
    pub body: String,
}

/// Parse a single-Markdown-file manuscript (#298).
pub fn parse_markdown(source: &str) -> Imported {
    // Book-level YAML front matter (if any) → title/author; the rest is the body.
    let (front, body) = frontmatter::split(source);
    let mut title = yaml_field(front, "title");
    let mut author = yaml_field(front, "author");

    let level = chapter_level(body);

    // Walk the lines, splitting at each chapter-level heading. Lines before the
    // first such heading are the preamble (a title page, in the export shape).
    let mut chapters: Vec<ImportedChapter> = Vec::new();
    let mut preamble: Vec<&str> = Vec::new();
    let mut cur_title: Option<&str> = None;
    let mut cur_body: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in body.lines() {
        if is_fence(line) {
            in_fence = !in_fence;
        } else if !in_fence && heading_level(line) == Some(level) {
            match cur_title.take() {
                Some(t) => chapters.push(ImportedChapter { title: t.into(), body: clean(&cur_body) }),
                None => preamble = std::mem::take(&mut cur_body),
            }
            cur_body.clear();
            cur_title = Some(heading_text(line));
            continue;
        }
        cur_body.push(line);
    }
    match cur_title {
        Some(t) => chapters.push(ImportedChapter { title: t.into(), body: clean(&cur_body) }),
        // No chapter heading was ever seen — the whole body is the preamble.
        None => preamble = cur_body,
    }

    // Mine the preamble (the title page) for anything the front matter didn't give.
    if title.is_none() {
        title = preamble_title(&preamble, level);
    }
    if author.is_none() {
        author = preamble_author(&preamble);
    }

    // No headings at all → the whole manuscript is a single chapter.
    if chapters.is_empty() {
        let body = clean(&preamble);
        let only = title.clone().unwrap_or_else(|| "Manuscript".into());
        chapters.push(ImportedChapter { title: only, body });
    }

    // A bare or blank heading still gets a usable chapter title.
    for (i, c) in chapters.iter_mut().enumerate() {
        if c.title.trim().is_empty() {
            c.title = format!("Chapter {}", i + 1);
        } else {
            c.title = c.title.trim().to_string();
        }
    }

    Imported { title, author, chapters }
}

/// The heading level chapters break on: level 2 when there's a lone title `#`
/// over `##` chapters (Quill's export), else level 1 when `#` is used per
/// chapter, else level 2 as the default.
fn chapter_level(body: &str) -> usize {
    let (mut h1, mut h2) = (0u32, 0u32);
    let mut in_fence = false;
    for line in body.lines() {
        if is_fence(line) {
            in_fence = !in_fence;
        } else if !in_fence {
            match heading_level(line) {
                Some(1) => h1 += 1,
                Some(2) => h2 += 1,
                _ => {}
            }
        }
    }
    if h1 == 1 && h2 >= 1 {
        2
    } else if h1 >= 1 {
        1
    } else {
        2
    }
}

/// The ATX heading level of a line (number of leading `#`, 1–6, followed by a
/// space), or `None` if it isn't a heading.
fn heading_level(line: &str) -> Option<usize> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
        Some(hashes)
    } else {
        None
    }
}

/// The text of a heading line (after the `#`s).
fn heading_text(line: &str) -> &str {
    line.trim_start().trim_start_matches('#').trim()
}

/// Whether a line opens or closes a fenced code block.
fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// Join body lines, trimming surrounding blank lines; a non-empty body ends in a
/// single newline (the chapter-file convention).
fn clean(lines: &[&str]) -> String {
    let trimmed = lines.join("\n").trim().to_string();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

/// The title page's `#` heading, when chapters split on level 2.
fn preamble_title(preamble: &[&str], level: usize) -> Option<String> {
    if level != 2 {
        return None;
    }
    preamble.iter().find(|l| heading_level(l) == Some(1)).map(|l| heading_text(l).to_string())
}

/// A "by …" attribution line on the title page (Quill's export writes one),
/// ignoring surrounding emphasis markers.
fn preamble_author(preamble: &[&str]) -> Option<String> {
    for line in preamble {
        let s = line.trim().trim_matches(|c| c == '*' || c == '_').trim();
        if s.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("by ")) {
            let a = s[3..].trim();
            if !a.is_empty() {
                return Some(a.to_string());
            }
        }
    }
    None
}

/// A scalar value for `key` in a YAML front-matter block (`key: value`, quotes
/// stripped), if present and non-empty.
fn yaml_field(front: Option<&str>, key: &str) -> Option<String> {
    front?.lines().find_map(|l| {
        let rest = l.trim().strip_prefix(key)?.trim_start().strip_prefix(':')?.trim();
        let v = rest.trim_matches(['"', '\'']).trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_quill_markdown_export() {
        // `#` title + `by` line, then `##` chapters — Quill's own export shape.
        let src = "# The Long Road\n\nby A. Writer\n\n## One\n\nThe night was *dark*.\n\n## Two\n\nThe **end**.\n";
        let out = parse_markdown(src);
        assert_eq!(out.title.as_deref(), Some("The Long Road"));
        assert_eq!(out.author.as_deref(), Some("A. Writer"));
        assert_eq!(out.chapters.len(), 2);
        assert_eq!(out.chapters[0].title, "One");
        assert_eq!(out.chapters[0].body, "The night was *dark*.\n", "emphasis preserved verbatim");
        assert_eq!(out.chapters[1].title, "Two");
        assert_eq!(out.chapters[1].body, "The **end**.\n");
    }

    #[test]
    fn splits_top_level_headings_when_each_chapter_is_h1() {
        let src = "# Chapter One\n\nAlpha.\n\n# Chapter Two\n\nBeta.\n";
        let out = parse_markdown(src);
        assert_eq!(out.chapters.len(), 2);
        assert_eq!(out.chapters[0].title, "Chapter One");
        assert_eq!(out.chapters[1].title, "Chapter Two");
        // No lone title heading, so no book title is inferred.
        assert_eq!(out.title, None);
    }

    #[test]
    fn yaml_front_matter_sets_title_and_author() {
        let src = "---\ntitle: \"My Book\"\nauthor: Jane Doe\n---\n\n# One\n\nText.\n# Two\n\nMore.\n";
        let out = parse_markdown(src);
        assert_eq!(out.title.as_deref(), Some("My Book"));
        assert_eq!(out.author.as_deref(), Some("Jane Doe"));
        assert_eq!(out.chapters.len(), 2);
    }

    #[test]
    fn a_file_with_no_headings_is_one_chapter() {
        let src = "Just some prose with no headings at all.\n\nA second paragraph.\n";
        let out = parse_markdown(src);
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "Manuscript");
        assert!(out.chapters[0].body.contains("second paragraph"));
    }

    #[test]
    fn a_heading_inside_a_code_fence_is_not_a_split() {
        let src = "# One\n\n```\n## not a chapter\n```\n\nbody\n\n# Two\n\nmore\n";
        let out = parse_markdown(src);
        // h1==2 → split on level 1; the fenced `##` stays inside chapter one.
        assert_eq!(out.chapters.len(), 2);
        assert!(out.chapters[0].body.contains("## not a chapter"), "fenced heading kept in the body");
    }

    #[test]
    fn deeper_headings_stay_in_the_chapter_body() {
        let src = "# Book\n\n## One\n\nIntro.\n\n### A scene\n\nScene text.\n";
        let out = parse_markdown(src);
        assert_eq!(out.title.as_deref(), Some("Book"));
        assert_eq!(out.chapters.len(), 1);
        assert!(out.chapters[0].body.contains("### A scene"), "sub-headings are part of the prose");
    }

    #[test]
    fn a_blank_heading_gets_a_fallback_title() {
        let src = "# Book\n\n## \n\nOrphan prose.\n";
        let out = parse_markdown(src);
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "Chapter 1");
    }
}
