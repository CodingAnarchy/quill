//! Plain-text `.txt` manuscript import (#593): a `.txt` has no Markdown headings
//! to split on, so chapters are found from `Chapter N`-style lines when present,
//! and otherwise from wide blank-line gaps (a run of two-or-more blank lines,
//! the usual hand-typed chapter separator). Reuses the shared [`assemble`] state
//! machine and the title-page mining every format shares. See the crate module
//! doc for the shared contract.

use super::{assemble, fallback_chapter, finalize_titles, mine_preamble, Imported};

/// Parse a plain-text manuscript (#593). Splits on `Chapter N` heading lines when
/// the file has any; otherwise on wide blank-line gaps. With neither, the whole
/// file is one chapter. Text is infallible, so `unreadable_bodies` is 0.
pub fn parse_text(source: &str) -> Imported {
    // Normalise CRLF so blank-line detection and joins don't trip on `\r`.
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.lines().collect();

    // Prefer explicit `Chapter N` headings; fall back to blank-line gaps. A gap
    // marker is synthetic (empty title), so a bare gap-split chapter gets a
    // `Chapter N` fallback title from `finalize_titles`. A file with neither a
    // heading nor a wide gap has no structure at all: leave it un-split so it
    // falls through to the single `"Manuscript"` chapter, like the Markdown path.
    let has_chapter_lines = lines.iter().any(|l| is_chapter_heading(l));
    let items: Vec<Item> = if has_chapter_lines {
        chapter_heading_items(&lines)
    } else if has_wide_gap(&lines) {
        gap_items(&lines)
    } else {
        // No structure → one item stream with no heading; assemble stashes it all
        // as the preamble and `fallback_chapter` makes it the single chapter.
        lines
            .iter()
            .map(|&text| Item {
                text,
                heading: false,
            })
            .collect()
    };

    let (mut chapters, preamble) = assemble(
        items,
        |item| item.heading,
        |item| item.text.trim().to_string(),
        join_lines,
    );
    let preamble: Vec<&str> = preamble.iter().map(|i| i.text).collect();

    let has_chapters = !chapters.is_empty();
    let (title, author) = mine_preamble(&preamble, None, None, has_chapters, |_p| None);

    fallback_chapter(&mut chapters, &title, join_str_lines(&preamble));
    finalize_titles(&mut chapters);

    Imported {
        title,
        author,
        chapters,
        unreadable_bodies: 0,
    }
}

/// One classified source line for the shared assembler: its text and whether it
/// opens a new chapter.
struct Item<'a> {
    text: &'a str,
    heading: bool,
}

/// Classify lines when the file has `Chapter N` headings: each such line opens a
/// chapter; every other line (blank lines included) is body.
fn chapter_heading_items<'a>(lines: &[&'a str]) -> Vec<Item<'a>> {
    lines
        .iter()
        .map(|&text| Item {
            text,
            heading: is_chapter_heading(text),
        })
        .collect()
}

/// Classify lines by blank-line gaps: a run of two-or-more blank lines is a
/// chapter separator. A synthetic (empty-title) heading item opens *every* block
/// — including the first — so gap-split mode has no title page and each block
/// becomes its own chapter (a blank title falls back to `Chapter N`).
fn gap_items<'a>(lines: &[&'a str]) -> Vec<Item<'a>> {
    let mut items: Vec<Item<'a>> = Vec::new();
    let mut blank_run = 0usize;
    let mut block_open = false; // is the current block already opened?
    for &text in lines {
        if text.trim().is_empty() {
            blank_run += 1;
            // Keep interior blank lines (they space paragraphs within a block);
            // `assemble` only appends them once a block is open.
            if block_open {
                items.push(Item {
                    text,
                    heading: false,
                });
            }
            continue;
        }
        // A non-blank line either opens the first block or, after a wide gap,
        // starts a new one.
        if !block_open || blank_run >= 2 {
            items.push(Item {
                text: "",
                heading: true,
            });
            block_open = true;
        }
        blank_run = 0;
        items.push(Item {
            text,
            heading: false,
        });
    }
    items
}

/// Whether the lines contain a wide gap — a run of two-or-more blank lines
/// between two non-blank lines — the hand-typed chapter separator gap mode splits
/// on. Leading/trailing blank runs don't count.
fn has_wide_gap(lines: &[&str]) -> bool {
    let mut blank_run = 0usize;
    let mut seen_content = false;
    for line in lines {
        if line.trim().is_empty() {
            if seen_content {
                blank_run += 1;
            }
        } else {
            if blank_run >= 2 {
                return true;
            }
            blank_run = 0;
            seen_content = true;
        }
    }
    false
}

/// Whether a line is a `Chapter N`-style heading: a line beginning with the word
/// `Chapter` (or `CHAPTER`). Matches `Chapter 1`, `Chapter One`,
/// `CHAPTER XII — The Storm`, etc.; `Chapters` (plural) is prose, not a marker.
fn is_chapter_heading(line: &str) -> bool {
    let t = line.trim();
    let Some(rest) = t
        .strip_prefix("Chapter")
        .or_else(|| t.strip_prefix("CHAPTER"))
    else {
        return false;
    };
    // `Chapter` must be a whole word: followed by whitespace or the line's end.
    rest.is_empty() || rest.starts_with(char::is_whitespace)
}

/// Join body lines, trimming surrounding blank lines; a non-empty body ends in a
/// single newline (the chapter-file convention), matching the Markdown path.
fn join_lines(lines: &[Item]) -> String {
    join_str_lines(&lines.iter().map(|i| i.text).collect::<Vec<_>>())
}

fn join_str_lines(lines: &[&str]) -> String {
    let trimmed = lines.join("\n").trim().to_string();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_chapter_heading_lines() {
        let src = "Chapter 1\n\nThe sea was grey.\n\nChapter 2\n\nAnd it ended.\n";
        let out = parse_text(src);
        assert_eq!(out.chapters.len(), 2);
        assert_eq!(out.chapters[0].title, "Chapter 1");
        assert_eq!(out.chapters[0].body, "The sea was grey.\n");
        assert_eq!(out.chapters[1].title, "Chapter 2");
        assert_eq!(out.chapters[1].body, "And it ended.\n");
    }

    #[test]
    fn named_chapter_lines_keep_their_titles() {
        let src = "Chapter One — Landfall\n\nAlpha.\n\nChapter Two\n\nBeta.\n";
        let out = parse_text(src);
        assert_eq!(out.chapters.len(), 2);
        assert_eq!(out.chapters[0].title, "Chapter One — Landfall");
        assert_eq!(out.chapters[1].title, "Chapter Two");
    }

    #[test]
    fn text_before_the_first_chapter_line_is_a_title_page() {
        // A "by …" line in the preamble is mined for the author.
        let src = "The Long Road\n\nby A. Writer\n\nChapter 1\n\nBody.\n";
        let out = parse_text(src);
        assert_eq!(out.author.as_deref(), Some("A. Writer"));
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "Chapter 1");
        assert_eq!(out.chapters[0].body, "Body.\n");
    }

    #[test]
    fn splits_on_wide_blank_line_gaps_when_no_chapter_lines() {
        // Two blank lines between blocks → a chapter break; a single blank line
        // within a block stays as a paragraph gap.
        let src = "First para.\n\nStill chapter one.\n\n\nSecond block.\n\n\nThird block.\n";
        let out = parse_text(src);
        assert_eq!(out.chapters.len(), 3, "three blocks → three chapters");
        assert_eq!(
            out.chapters[0].body, "First para.\n\nStill chapter one.\n",
            "single blank line kept as a paragraph gap"
        );
        assert_eq!(out.chapters[1].body, "Second block.\n");
        assert_eq!(out.chapters[2].body, "Third block.\n");
        // Gap-split chapters have no titles → the Chapter N fallback.
        assert_eq!(out.chapters[0].title, "Chapter 1");
        assert_eq!(out.chapters[2].title, "Chapter 3");
    }

    #[test]
    fn a_file_with_no_breaks_is_one_chapter() {
        let src = "Just some prose.\nA second line, no wide gaps.\n";
        let out = parse_text(src);
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "Manuscript");
        assert!(out.chapters[0].body.contains("second line"));
    }

    #[test]
    fn chapters_plural_is_not_a_heading() {
        // `Chapters` (plural) is prose, not a chapter marker; with no real
        // heading and no wide gap it's a single chapter.
        let src = "Chapters of my life.\nMore prose.\n";
        let out = parse_text(src);
        assert_eq!(out.chapters.len(), 1);
    }

    #[test]
    fn handles_crlf_line_endings() {
        let src = "Chapter 1\r\n\r\nBody one.\r\n\r\nChapter 2\r\n\r\nBody two.\r\n";
        let out = parse_text(src);
        assert_eq!(out.chapters.len(), 2);
        assert_eq!(out.chapters[0].body, "Body one.\n");
        assert_eq!(out.chapters[1].body, "Body two.\n");
    }
}
