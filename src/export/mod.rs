//! Manuscript export (#7). A [`Document`] is the project compiled into a single,
//! ordered structure — book metadata, then parts and chapters with their
//! epigraphs and prose, in reading order. This module is the shared layer the
//! format writers build on: [`to_markdown`] renders one to a single Markdown file
//! (#190); the Word / Scrivener / Vellum writers (#191–#193) consume the same
//! `Document`.

use std::fmt::Write;

use crate::frontmatter::{Epigraph, Role};

pub mod docx;
pub mod epub;
pub mod pdf;
pub mod preflight;
pub mod scriv;
pub mod style;

pub use style::{EmbeddedFont, EmbeddedImage, ExportStyle, FontRole, FontStyle, ImageRole};

/// A project compiled for export: the book's metadata and its contents as a tree
/// — front matter, the body (grouped into parts), and back matter — so a binder
/// format (Scrivener, #192) can nest it and a linear format (Markdown/Word) can
/// flatten it via [`Document::blocks`].
pub struct Document {
    pub title: String,
    pub subtitle: Option<String>,
    pub author: Option<String>,
    pub series: Option<String>,
    /// Book-level publishing metadata (#583) — ISBN, publisher, publication date,
    /// language, and rights — emitted into the EPUB OPF / PDF / DOCX and the
    /// generated copyright page.
    pub publishing: PubMeta,
    /// The book epigraph, rendered at the start.
    pub epigraph: Option<Epigraph>,
    /// Front-matter sections (foreword, prologue, …), in order.
    pub front: Vec<Chapter>,
    /// The body, grouped into parts. A book with no defined parts has a single
    /// unnamed part holding all body chapters.
    pub parts: Vec<Part>,
    /// Back-matter sections (epilogue, appendix, …), in order.
    pub back: Vec<Chapter>,
}

/// Book-level publishing metadata (#583): the fields a published book carries
/// beyond its title block — ISBN, publisher, publication date, language, and the
/// rights/copyright line. Each is optional; absent fields are simply omitted from
/// the OPF, the copyright page, and the PDF/DOCX. `language` is the BCP-47 tag the
/// EPUB drives `dc:language`/`xml:lang` from; [`PubMeta::language`] supplies the
/// `en` fallback when unset (replacing the old hardcoded `en`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PubMeta {
    pub isbn: Option<String>,
    pub publisher: Option<String>,
    /// Publication date (`YYYY` or `YYYY-MM-DD`).
    pub date: Option<String>,
    /// BCP-47 language tag (e.g. `en`, `fr`, `es-419`). Read via [`Self::language`].
    pub lang: Option<String>,
    /// The rights / copyright line (e.g. `© 2026 A. Writer. All rights reserved.`).
    pub rights: Option<String>,
}

impl PubMeta {
    /// The BCP-47 language tag, falling back to `en` when unset — the single place
    /// the EPUB `dc:language`/`xml:lang` default lives (#583), no longer hardcoded.
    pub fn language(&self) -> &str {
        self.lang.as_deref().unwrap_or("en")
    }

    /// Whether a copyright page should be rendered: true once any field that
    /// belongs on it (publisher, date, ISBN, or an explicit rights line) is set.
    /// A book with no publishing metadata gets no copyright page, so the default
    /// output is unchanged.
    const fn has_copyright_page(&self) -> bool {
        self.rights.is_some()
            || self.publisher.is_some()
            || self.date.is_some()
            || self.isbn.is_some()
    }
}

/// The lines of the generated copyright page (#583), in order, for a book whose
/// publishing metadata warrants one — the rights line, then publisher, the
/// publication-date line, and the ISBN. Each writer renders these lines in its own
/// markup (centered/italic prose), the way [`render_epigraph`] is shared. Returns
/// an empty vector when no copyright page is warranted, so callers can skip the
/// section entirely.
pub fn copyright_lines(meta: &PubMeta) -> Vec<String> {
    if !meta.has_copyright_page() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    if let Some(rights) = &meta.rights {
        lines.push(rights.clone());
    }
    if let Some(publisher) = &meta.publisher {
        lines.push(format!("Published by {publisher}"));
    }
    if let Some(date) = &meta.date {
        lines.push(format!("First published {date}"));
    }
    if let Some(isbn) = &meta.isbn {
        lines.push(format!("ISBN: {isbn}"));
    }
    lines
}

/// A named (or unnamed) grouping of body chapters.
pub struct Part {
    /// The part's title, or `None` for the catch-all group of ungrouped body
    /// chapters (rendered without a part heading / folder).
    pub name: Option<String>,
    pub chapters: Vec<Chapter>,
}

/// A chapter or front/back-matter section: its title, optional epigraph, and prose
/// body (front matter already stripped).
pub struct Chapter {
    pub title: String,
    pub epigraph: Option<Epigraph>,
    pub body: String,
    /// The section's role (#179), when it is a front/back-matter section rather
    /// than a plain body chapter (`None`). Carried through so the EPUB writer can
    /// tag each section with the precise `epub:type` (#695) — a foreword,
    /// dedication, glossary, … — instead of the generic front/back-matter band.
    pub role: Option<Role>,
}

/// The book flattened to a linear sequence — part headings interleaved with
/// chapters — for the Markdown and Word writers.
pub enum Block<'a> {
    Part(&'a str),
    Chapter(&'a Chapter),
}

impl Document {
    /// Front matter, then each part (a heading when named) and its chapters, then
    /// back matter — in reading order.
    pub fn blocks(&self) -> Vec<Block<'_>> {
        let mut out = Vec::new();
        out.extend(self.front.iter().map(Block::Chapter));
        for part in &self.parts {
            if let Some(name) = &part.name {
                out.push(Block::Part(name));
            }
            out.extend(part.chapters.iter().map(Block::Chapter));
        }
        out.extend(self.back.iter().map(Block::Chapter));
        out
    }
}

/// Render a [`Document`] as a single Markdown document (#190): a title block,
/// then each part/chapter under a heading with its epigraph set apart as a
/// block quote. Clean, pandoc-friendly Markdown — the manuscript's native form.
pub fn to_markdown(doc: &Document) -> String {
    let mut out = String::new();
    if let Some(series) = &doc.series {
        out.push_str(series);
        out.push_str("\n\n");
    }
    let _ = write!(out, "# {}\n\n", doc.title);
    if let Some(subtitle) = &doc.subtitle {
        let _ = write!(out, "*{subtitle}*\n\n");
    }
    if let Some(author) = &doc.author {
        let _ = write!(out, "by {author}\n\n");
    }
    if let Some(epigraph) = &doc.epigraph {
        out.push_str(&epigraph_block(epigraph));
    }

    for block in doc.blocks() {
        match block {
            Block::Part(name) => {
                let _ = write!(out, "## {name}\n\n");
            }
            Block::Chapter(chapter) => {
                let _ = write!(out, "## {}\n\n", chapter.title);
                if let Some(epigraph) = &chapter.epigraph {
                    out.push_str(&epigraph_block(epigraph));
                }
                // The body is already Markdown, so it's emitted verbatim — no
                // block classification needed (a scene-break rule like `---` is
                // already a Markdown thematic break; #199 only translates it for
                // the Word/Scrivener writers, which can't carry raw Markdown).
                let body = chapter.body.trim();
                if !body.is_empty() {
                    out.push_str(body);
                    out.push_str("\n\n");
                }
            }
        }
    }
    out
}

/// Render a whole series as one Markdown document (#332): the series title and
/// author once, then each book in order (each via [`to_markdown`], so its title
/// leads a top-level section and its parts/chapters nest beneath). The books
/// carry no `series`/`author` line of their own — the header above stands for the
/// whole series.
pub fn to_markdown_series(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
) -> String {
    let mut out = String::new();
    if let Some(series) = series {
        out.push_str(series);
        out.push_str("\n\n");
    }
    if let Some(author) = author {
        let _ = write!(out, "by {author}\n\n");
    }
    for book in books {
        out.push_str(&to_markdown(book));
    }
    out
}

/// Drop a chapter body's leading `# {title}` line when it just repeats the
/// chapter title (the compiled heading already carries it), so the export doesn't
/// show the title twice. A body whose first heading differs from the title is
/// left untouched — only a true duplicate is removed. Mirrors the outline pane's
/// `redundant_title_heading`.
pub fn strip_repeated_title(body: &str, title: &str) -> String {
    let trimmed = body.trim_start_matches('\n');
    let Some(rest) = trimmed.strip_prefix("# ") else {
        return body.to_string();
    };
    let (heading, after) = rest.split_once('\n').unwrap_or((rest, ""));
    if heading.trim() == title.trim() {
        after.trim_start_matches('\n').to_string()
    } else {
        body.to_string()
    }
}

/// A styled span of text — Quill's bold/italic "run", the unit both export's
/// run-splitter ([`inline_runs`]) and import's RTF run-collector produce. It
/// round-trips Quill's own export, so the type is shared rather than modeled as
/// an untyped `(String, bool, bool)` tuple on each side (#506).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub(crate) text: String,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
}

impl Run {
    /// A run carrying `text` with the given emphasis.
    pub(crate) fn new(text: impl Into<String>, bold: bool, italic: bool) -> Self {
        Self {
            text: text.into(),
            bold,
            italic,
        }
    }
}

/// Split a line into runs by `*` (italic) and `**` (bold) markers — Quill's
/// emphasis convention. Other text (including `_`) is taken literally. Shared by
/// the Word and Scrivener writers, which carry emphasis through to their own
/// run markup.
fn inline_runs(text: &str) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut cur = String::new();
    let (mut bold, mut italic) = (false, false);
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' {
            if chars.get(i + 1) == Some(&'*') {
                push_run(&mut runs, &mut cur, bold, italic);
                bold = !bold;
                i += 2;
                continue;
            }
            push_run(&mut runs, &mut cur, bold, italic);
            italic = !italic;
            i += 1;
            continue;
        }
        cur.push(chars[i]);
        i += 1;
    }
    push_run(&mut runs, &mut cur, bold, italic);
    // Unbalanced markers mean the `*`s weren't emphasis (a stray `*` in prose,
    // e.g. "5 * 3"). Don't drop them and over-italicize — emit the line verbatim.
    if bold || italic {
        return vec![Run::new(text, false, false)];
    }
    runs
}

/// Flush the accumulated text as a [`Run`] with the current bold/italic flags.
/// The single `mem::take`-into-a-run helper shared by the export run-splitter and
/// the import RTF run-collector (#506).
pub fn push_run(runs: &mut Vec<Run>, cur: &mut String, bold: bool, italic: bool) {
    if !cur.is_empty() {
        runs.push(Run::new(std::mem::take(cur), bold, italic));
    }
}

/// Escape `& < >` for embedding text in XML (the `.docx` and `.scrivx` markup).
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The scene-break glyph the writers emit for a thematic break: a centered `#`,
/// the standard manuscript / Scrivener scene divider, which Vellum also
/// recognizes as a scene break on import (#199). The `.scriv` importer reads it
/// back to a thematic break, so it's shared rather than re-declared.
pub const SCENE_BREAK: &str = "#";

/// One block of a chapter body (the body split on blank lines), classified so
/// each format writer renders it in its own markup. Shared by the Word and
/// Scrivener writers so scene breaks, headings, and prose are recognized
/// identically.
#[derive(Debug)]
enum Prose {
    /// A scene break — a thematic-break line (`---`, `* * *`, …).
    SceneBreak,
    /// A heading line (`#`…), markers stripped.
    Heading(String),
    /// A normal paragraph, soft-wrapped lines joined into one.
    Paragraph(String),
    /// A fenced code block (#522), rendered verbatim and monospaced — line breaks
    /// and indentation preserved, no emphasis parsed. The inner content, fences
    /// removed.
    Preformatted(String),
}

/// Split a chapter body into blocks for classification. Blocks are separated by
/// blank lines, except a fenced code block (```` ``` ````) is kept whole even
/// when it contains blank lines, so preformatted text survives intact (#522).
fn body_blocks(body: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut in_fence = false;
    let flush = |cur: &mut Vec<&str>, blocks: &mut Vec<String>| {
        if !cur.is_empty() {
            blocks.push(cur.join("\n"));
            cur.clear();
        }
    };
    for line in body.lines() {
        let is_fence = line.trim_start().starts_with("```");
        if in_fence {
            cur.push(line);
            if is_fence {
                flush(&mut cur, &mut blocks);
                in_fence = false;
            }
        } else if is_fence {
            flush(&mut cur, &mut blocks);
            cur.push(line);
            in_fence = true;
        } else if line.trim().is_empty() {
            flush(&mut cur, &mut blocks);
        } else {
            cur.push(line);
        }
    }
    flush(&mut cur, &mut blocks);
    blocks
}

/// The inner content of a fenced code block, with the fence delimiter lines (and
/// any info string) removed and the line breaks and indentation between them kept
/// verbatim. `None` if `block` isn't fence-led (#522).
fn fenced_code(block: &str) -> Option<String> {
    let mut lines = block.lines();
    if !lines.next()?.trim_start().starts_with("```") {
        return None;
    }
    let rest: Vec<&str> = lines.collect();
    // Drop the closing fence when present (an unterminated fence runs to the end).
    let inner = match rest.last() {
        Some(last) if last.trim_start().starts_with("```") => &rest[..rest.len() - 1],
        _ => &rest[..],
    };
    Some(inner.join("\n"))
}

/// The classified prose blocks of a chapter body, in order. Splits on blank
/// lines (a fenced code block kept whole) and classifies each block, dropping
/// empties — the shared body-segmentation seam every format writer renders from,
/// so a change to segmentation lands in one place. Each writer maps the yielded
/// [`Prose`] to its own markup; any per-format state (the EPUB drop-cap
/// `opening` flag) is computed by the caller from the yielded variant.
fn prose_blocks(body: &str) -> impl Iterator<Item = Prose> {
    body_blocks(body)
        .into_iter()
        .filter_map(|block| classify_block(&block))
}

/// Classify one body block (already split via [`body_blocks`]), or `None` when
/// it's empty.
fn classify_block(block: &str) -> Option<Prose> {
    if let Some(code) = fenced_code(block) {
        return Some(Prose::Preformatted(code));
    }
    let text = block.trim();
    if text.is_empty() {
        return None;
    }
    if is_thematic_break(text) {
        return Some(Prose::SceneBreak);
    }
    if let Some(heading) = text.strip_prefix('#') {
        return Some(Prose::Heading(
            heading.trim_start_matches('#').trim().to_string(),
        ));
    }
    // Soft-wrapped lines join into one paragraph.
    let joined = text
        .split('\n')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");
    Some(Prose::Paragraph(joined))
}

/// A CommonMark thematic break: a single line of three or more matching `-`,
/// `_`, or `*`, with optional spaces between and nothing else — Quill's
/// scene-break marker. A line with any other character (an em-dash, "5 * 3") is
/// not one, so prose isn't mistaken for a break.
fn is_thematic_break(block: &str) -> bool {
    if block.contains('\n') {
        return false;
    }
    let marks: Vec<char> = block.chars().filter(|c| !c.is_whitespace()).collect();
    marks.len() >= 3 && matches!(marks[0], '-' | '_' | '*') && marks.iter().all(|&c| c == marks[0])
}

/// Shared epigraph driver: render each text line through `line_fn` and, when an
/// attribution is present, render it through `attribution_fn` (which receives the
/// bare attribution, prefixing the conventional `— ` itself). One place owns the
/// line/attribution ordering; each format supplies only its per-line and
/// attribution markup.
fn render_epigraph(
    epigraph: &Epigraph,
    line_fn: impl Fn(&str) -> String,
    attribution_fn: impl Fn(&str) -> String,
) -> String {
    let mut out = String::new();
    for line in epigraph.text.lines() {
        out.push_str(&line_fn(line));
    }
    if let Some(attribution) = &epigraph.attribution {
        out.push_str(&attribution_fn(attribution));
    }
    out
}

/// An epigraph as a Markdown block quote — each line prefixed `> `, the
/// attribution on its own dimmed line — set apart from the surrounding prose.
fn epigraph_block(epigraph: &Epigraph) -> String {
    let mut out = render_epigraph(
        epigraph,
        |line| {
            if line.is_empty() {
                ">\n".to_string()
            } else {
                format!("> {line}\n")
            }
        },
        |attribution| format!(">\n> — {attribution}\n"),
    );
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn doc() -> Document {
        Document {
            title: "Red State Earth".into(),
            subtitle: Some("a novel".into()),
            author: Some("A. Writer".into()),
            series: Some("The Cycle".into()),
            publishing: PubMeta::default(),
            epigraph: Some(epigraph("In the beginning.", Some("Anon"))),
            front: vec![chapter("Prologue", None, "Before it all.")],
            parts: vec![Part {
                name: Some("Part One".into()),
                chapters: vec![chapter(
                    "The Find",
                    Some(epigraph("A line.", None)),
                    "The locket *gleamed*.",
                )],
            }],
            back: vec![],
        }
    }

    #[test]
    fn series_markdown_lists_books_in_order_under_the_series_title() {
        let book = |title: &str, body: &str| Document {
            title: title.into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: PubMeta::default(),
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter("Ch", None, body)],
            }],
            back: vec![],
        };
        let md = to_markdown_series(
            &[book("Book One", "alpha"), book("Book Two", "beta")],
            Some("The Saga"),
            Some("A. Writer"),
        );
        assert!(md.starts_with("The Saga\n\n"), "{md}");
        let one = md.find("# Book One").expect("book one heading");
        let two = md.find("# Book Two").expect("book two heading");
        assert!(one < two, "books in order");
        assert!(md.contains("alpha") && md.contains("beta"));
    }

    #[test]
    fn a_fenced_code_block_is_kept_whole_and_classified_preformatted() {
        // A fence with a blank line inside must not be split on the blank line.
        let body = "Intro line.\n\n```\nZENITH // 2053\n\nPRE-IGNIT\n```\n\nAfter.";
        let blocks = body_blocks(body);
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        assert_eq!(blocks[0], "Intro line.");
        assert_eq!(blocks[2], "After.");
        // The fenced block is preformatted: fences gone, inner breaks preserved.
        match classify_block(&blocks[1]) {
            Some(Prose::Preformatted(code)) => {
                assert_eq!(code, "ZENITH // 2053\n\nPRE-IGNIT");
            }
            other => panic!("expected Preformatted, got {other:?}"),
        }
    }

    #[test]
    fn an_unterminated_fence_runs_to_the_end_of_the_body() {
        // No closing fence: everything after the opener is the preformatted block.
        let blocks = body_blocks("```\nlog one\nlog two");
        assert_eq!(blocks.len(), 1);
        match classify_block(&blocks[0]) {
            Some(Prose::Preformatted(code)) => assert_eq!(code, "log one\nlog two"),
            other => panic!("expected Preformatted, got {other:?}"),
        }
    }

    #[test]
    fn markdown_export_preserves_a_fenced_code_block_verbatim() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: PubMeta::default(),
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter(
                    "C",
                    None,
                    "Before.\n\n```\nLOG: --ok\n```\n\nAfter.",
                )],
            }],
            back: vec![],
        };
        // The body is emitted verbatim, so the fence (and its literal `--`) survive.
        assert!(
            to_markdown(&d).contains("```\nLOG: --ok\n```"),
            "{}",
            to_markdown(&d)
        );
    }

    #[test]
    fn smart_typography_glyphs_pass_through_to_markdown() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: PubMeta::default(),
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter(
                    "C",
                    None,
                    "He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}",
                )],
            }],
            back: vec![],
        };
        assert!(
            to_markdown(&d).contains("He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}"),
            "curly glyphs reach the Markdown/Vellum output"
        );
    }

    #[test]
    fn renders_the_book_header_and_epigraph() {
        let md = to_markdown(&doc());
        assert!(md.starts_with("The Cycle\n\n# Red State Earth\n\n*a novel*\n\nby A. Writer\n\n"));
        assert!(md.contains("> In the beginning.\n>\n> — Anon\n"), "{md}");
    }

    #[test]
    fn renders_items_in_order_with_headings() {
        let md = to_markdown(&doc());
        let prologue = md.find("## Prologue").unwrap();
        let part = md.find("## Part One").unwrap();
        let chapter = md.find("## The Find").unwrap();
        assert!(
            prologue < part && part < chapter,
            "reading order preserved:\n{md}"
        );
        // The chapter epigraph renders before its body; prose is verbatim.
        let find = &md[chapter..];
        assert!(
            find.find("> A line.").unwrap() < find.find("The locket *gleamed*.").unwrap(),
            "{md}"
        );
    }

    #[test]
    fn strips_a_body_heading_only_when_it_repeats_the_title() {
        // A leading `# Title` that matches is dropped (the compiled heading has it).
        assert_eq!(
            strip_repeated_title("# The Find\n\nProse.\n", "The Find"),
            "Prose.\n"
        );
        // A differing heading is kept (don't lose the author's text).
        assert_eq!(
            strip_repeated_title("# Prologue\n\nBefore.\n", "A Prologue"),
            "# Prologue\n\nBefore.\n"
        );
        // No leading heading → untouched.
        assert_eq!(
            strip_repeated_title("Just prose.\n", "Title"),
            "Just prose.\n"
        );
    }

    #[test]
    fn parses_nested_and_plain_emphasis() {
        assert_eq!(inline_runs("plain"), vec![Run::new("plain", false, false)]);
        assert_eq!(
            inline_runs("a *b* c"),
            vec![
                Run::new("a ", false, false),
                Run::new("b", false, true),
                Run::new(" c", false, false)
            ]
        );
        assert_eq!(inline_runs("**bold**"), vec![Run::new("bold", true, false)]);
        // Underscores are literal (not emphasis), so snake_case survives.
        assert_eq!(
            inline_runs("snake_case"),
            vec![Run::new("snake_case", false, false)]
        );
        // A stray/unbalanced `*` is kept literal, not turned into runaway italics.
        assert_eq!(
            inline_runs("5 * 3 = 15"),
            vec![Run::new("5 * 3 = 15", false, false)]
        );
    }

    #[test]
    fn xml_escape_escapes_the_markup_chars() {
        assert_eq!(xml_escape("a & b < c > d"), "a &amp; b &lt; c &gt; d");
    }

    #[test]
    fn recognizes_thematic_breaks_but_not_prose() {
        // The scene-break forms an author might type.
        for br in ["---", "***", "___", "* * *", "- - -", "----"] {
            assert!(is_thematic_break(br), "{br:?} is a scene break");
            assert!(
                matches!(classify_block(br), Some(Prose::SceneBreak)),
                "{br:?}"
            );
        }
        // Prose that merely contains the same characters is not a break.
        for prose in [
            "a — dash",
            "5 * 3 = 15",
            "--",
            "well - maybe",
            "* one bullet",
        ] {
            assert!(!is_thematic_break(prose), "{prose:?} is prose, not a break");
        }
        // Headings and paragraphs still classify as before.
        assert!(matches!(classify_block("## A scene"), Some(Prose::Heading(h)) if h == "A scene"));
        assert!(classify_block("  ").is_none());
    }

    #[test]
    fn markdown_keeps_a_scene_break_as_a_thematic_break() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: PubMeta::default(),
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter("One", None, "Scene one.\n\n---\n\nScene two.")],
            }],
            back: vec![],
        };
        assert!(
            to_markdown(&d).contains("Scene one.\n\n---\n\nScene two."),
            "{}",
            to_markdown(&d)
        );
    }

    #[test]
    fn a_multiline_epigraph_quotes_every_line() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: PubMeta::default(),
            epigraph: Some(epigraph("Roses are red\n\nViolets are blue", None)),
            front: vec![],
            parts: vec![],
            back: vec![],
        };
        let md = to_markdown(&d);
        assert!(
            md.contains("> Roses are red\n>\n> Violets are blue\n"),
            "{md}"
        );
    }
}
