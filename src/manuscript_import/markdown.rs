//! The single-Markdown-file manuscript parser (#298): split on ATX headings,
//! mine the title page and YAML front matter for title/author. See the crate
//! module doc for the splitting rules.

use crate::frontmatter;

use super::{assemble, fallback_chapter, finalize_titles, mine_preamble, Imported};

/// Parse a single-Markdown-file manuscript (#298).
pub fn parse_markdown(source: &str) -> Imported {
    // Book-level YAML front matter (if any) → title/author; the rest is the body.
    let (front, body) = frontmatter::split(source);
    let title = yaml_field(front, "title");
    let author = yaml_field(front, "author");

    let level = chapter_level(body);

    // Split at each chapter-level heading; lines before the first such heading
    // are the preamble (a title page, in the export shape). A `Line` carries its
    // pre-classified heading-ness so the split is fence-aware in one pass.
    let (mut chapters, preamble) = assemble(
        classify_lines(body, level),
        |line| line.heading,
        |line| heading_text(line.text).to_string(),
        |lines| clean(&lines.iter().map(|l| l.text).collect::<Vec<_>>()),
    );
    let preamble: Vec<&str> = preamble.iter().map(|l| l.text).collect();

    // Mine the preamble (the title page) for anything the front matter didn't give.
    let has_chapters = !chapters.is_empty();
    let (title, author) = mine_preamble(&preamble, title, author, has_chapters, |p| {
        preamble_title(p, level)
    });

    // No headings at all → the whole manuscript is a single chapter.
    fallback_chapter(&mut chapters, &title, clean(&preamble));

    finalize_titles(&mut chapters);

    Imported {
        title,
        author,
        chapters,
        // Markdown is infallible: there's no per-chapter I/O to fail.
        unreadable_bodies: 0,
    }
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

/// One source line, pre-classified for the shared assembler: its text and
/// whether it's a chapter-level heading (a heading inside a code fence isn't).
struct Line<'a> {
    text: &'a str,
    heading: bool,
}

/// Classify each body line as a chapter-level heading or not, tracking fenced
/// code blocks so a `#` inside a fence stays in the prose. Feeds [`assemble`].
fn classify_lines(body: &str, level: usize) -> Vec<Line<'_>> {
    let mut in_fence = false;
    body.lines()
        .map(|text| {
            if is_fence(text) {
                in_fence = !in_fence;
                Line {
                    text,
                    heading: false,
                }
            } else {
                Line {
                    text,
                    heading: !in_fence && heading_level(text) == Some(level),
                }
            }
        })
        .collect()
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
    preamble
        .iter()
        .find(|l| heading_level(l) == Some(1))
        .map(|l| heading_text(l).to_string())
}

/// A scalar value for `key` in a YAML front-matter block (`key: value`, quotes
/// stripped), if present and non-empty.
fn yaml_field(front: Option<&str>, key: &str) -> Option<String> {
    front?.lines().find_map(|l| {
        let rest = l
            .trim()
            .strip_prefix(key)?
            .trim_start()
            .strip_prefix(':')?
            .trim();
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
        assert_eq!(
            out.chapters[0].body, "The night was *dark*.\n",
            "emphasis preserved verbatim"
        );
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
        let src =
            "---\ntitle: \"My Book\"\nauthor: Jane Doe\n---\n\n# One\n\nText.\n# Two\n\nMore.\n";
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
        assert!(
            out.chapters[0].body.contains("## not a chapter"),
            "fenced heading kept in the body"
        );
    }

    #[test]
    fn deeper_headings_stay_in_the_chapter_body() {
        let src = "# Book\n\n## One\n\nIntro.\n\n### A scene\n\nScene text.\n";
        let out = parse_markdown(src);
        assert_eq!(out.title.as_deref(), Some("Book"));
        assert_eq!(out.chapters.len(), 1);
        assert!(
            out.chapters[0].body.contains("### A scene"),
            "sub-headings are part of the prose"
        );
    }

    #[test]
    fn a_blank_heading_gets_a_fallback_title() {
        let src = "# Book\n\n## \n\nOrphan prose.\n";
        let out = parse_markdown(src);
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "Chapter 1");
    }
}
