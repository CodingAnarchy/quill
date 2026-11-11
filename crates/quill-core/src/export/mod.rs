//! Manuscript export (#7). A [`Document`] is the project compiled into a single,
//! ordered structure — book metadata, then parts and chapters with their
//! epigraphs and prose, in reading order. This module is the shared layer the
//! format writers build on: [`to_markdown`] renders one to a single Markdown file
//! (#190); the Word / Scrivener / Vellum writers (#191–#193) consume the same
//! `Document`.

use std::fmt::Write;

use crate::frontmatter::{series_list_lines, Epigraph, Role};

pub mod assemble;
pub mod docx;
pub mod epub;
#[cfg(test)]
mod novel_tests;
pub mod pdf;
pub mod preflight;
pub mod scriv;
#[cfg(test)]
mod story_tests;
pub mod style;
mod verse;
#[cfg(test)]
mod verse_tests;

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
    /// Whether a bare **half-title page** — the book title alone — is generated
    /// ahead of the full title page, the conventional first printed leaf of a
    /// trade book (#820). Opt-in via the `[style]` `half_title` key; `false`
    /// leaves the front matter (cover → title → copyright → body) unchanged.
    pub half_title: bool,
    /// The full `[[books]]` series list by title (#820), used to generate the
    /// "also in this series" front-matter page. Empty for a standalone book (or
    /// when the `series_list` style key disables it), so the page is omitted and
    /// a standalone export is unchanged. The book's own title is filtered out at
    /// render time (see [`crate::frontmatter::series_list_lines`]) so it never
    /// lists itself.
    pub series_books: Vec<String>,
    /// Front-matter sections (foreword, prologue, …), in order.
    pub front: Vec<Chapter>,
    /// The body, grouped into parts. A book with no defined parts has a single
    /// unnamed part holding all body chapters.
    pub parts: Vec<Part>,
    /// Back-matter sections (epilogue, appendix, …), in order.
    pub back: Vec<Chapter>,
    /// Set for a short story (#1241): its body is one piece of prose, so no
    /// writer gives it a chapter heading or a one-entry contents, and the Word
    /// manuscript takes the short story submission layout.
    pub story: Option<StoryMeta>,
    /// Set for a novel (#1302): the contact block and word count for its Word
    /// manuscript's title page, which takes standard manuscript format rather
    /// than a finished book's front matter. `None` for a story, a collection,
    /// and a series book.
    pub novel: Option<StoryMeta>,
}

/// What a submission manuscript carries beyond the prose (#1241, #1302): the
/// author's contact block and the story's or novel's word count.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoryMeta {
    /// The contact block, top-left of page one: name, address lines, email, phone.
    pub contact: Vec<String>,
    /// The story's exact word count, rounded for the manuscript by
    /// [`approximate_words`].
    pub words: usize,
}

/// The word count a submission manuscript states (#1241): "about 3,500 words",
/// rounded to the nearest hundred — or to the nearest ten under 1,000, where a
/// hundred is too coarse for flash fiction.
pub fn approximate_words(words: usize) -> String {
    let step = if words < 1_000 { 10 } else { 100 };
    let rounded = ((words + step / 2) / step * step).max(step);
    let digits = rounded.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("about {grouped} words")
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
    /// Whether the chapter is verse (#1327): a target meter or rhyme scheme, or
    /// `verse: true`, in its front matter. Its lines keep their breaks on export.
    pub verse: bool,
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

    /// The lines of this book's generated "also in this series" front-matter page
    /// (#820) — the heading, then every *other* book of the series — or an empty
    /// vector when [`Self::series_books`] holds no other title (a standalone book,
    /// or the list disabled), so a writer can skip the page. The book's own title
    /// is dropped so it never lists itself. See
    /// [`series_list_lines`](crate::frontmatter::series_list_lines).
    pub fn series_list_lines(&self) -> Vec<String> {
        series_list_lines(&self.series_books, &self.title)
    }

    /// Whether `chapter` is a short story's body (#1241) — the prose that goes
    /// under the book's own title, with no chapter heading of its own. A front or
    /// back matter section of a story keeps its heading.
    pub const fn is_story_body(&self, chapter: &Chapter) -> bool {
        self.story.is_some() && chapter.role.is_none()
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
                // A short story's body is the story itself, already titled above.
                if !doc.is_story_body(chapter) {
                    let _ = write!(out, "## {}\n\n", chapter.title);
                }
                if let Some(epigraph) = &chapter.epigraph {
                    out.push_str(&epigraph_block(epigraph));
                }
                // The body is already Markdown, so it's emitted verbatim — no
                // block classification needed (a scene-break rule like `---` is
                // already a Markdown thematic break; #199 only translates it for
                // the Word/Scrivener writers, which can't carry raw Markdown).
                // A verse chapter's lines carry hard breaks so they stay lines
                // (#1327).
                let verse_body;
                let body = if chapter.verse {
                    verse_body = verse::markdown_body(chapter.body.trim());
                    verse_body.trim()
                } else {
                    chapter.body.trim()
                };
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

/// The emphasis a [`Run`] (or word) carries — the four meaningful combinations of
/// bold and italic, named so a call site reads `TextStyle::Bold` instead of the
/// opaque `(false, true)` pair the run-splitter and RTF collector used to pass
/// around (#1083). Extends the same "don't model this as an untyped tuple" intent
/// [`Run`] already carries (#506). The [`is_bold`](TextStyle::is_bold) /
/// [`is_italic`](TextStyle::is_italic) accessors let downstream logic keep reading
/// the two flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextStyle {
    /// Neither bold nor italic — plain body text.
    Plain,
    /// Bold only.
    Bold,
    /// Italic only.
    Italic,
    /// Both bold and italic (nested `***x***`).
    BoldItalic,
}

impl TextStyle {
    /// The style for the given `(bold, italic)` emphasis flags — the bridge from the
    /// run-splitter's two accumulating booleans to the named style.
    pub const fn from_flags(bold: bool, italic: bool) -> Self {
        match (bold, italic) {
            (false, false) => Self::Plain,
            (true, false) => Self::Bold,
            (false, true) => Self::Italic,
            (true, true) => Self::BoldItalic,
        }
    }

    /// Whether the style is bold.
    pub const fn is_bold(self) -> bool {
        matches!(self, Self::Bold | Self::BoldItalic)
    }

    /// Whether the style is italic.
    pub const fn is_italic(self) -> bool {
        matches!(self, Self::Italic | Self::BoldItalic)
    }
}

/// A styled span of text — Quill's bold/italic "run", the unit both export's
/// run-splitter ([`inline_runs`]) and import's RTF run-collector produce. It
/// round-trips Quill's own export, so the type is shared rather than modeled as
/// an untyped `(String, bool, bool)` tuple on each side (#506).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
}

impl Run {
    /// A run carrying `text` with the given [`TextStyle`] emphasis (#1083).
    pub fn new(text: impl Into<String>, style: TextStyle) -> Self {
        Self {
            text: text.into(),
            bold: style.is_bold(),
            italic: style.is_italic(),
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
        return vec![Run::new(text, TextStyle::Plain)];
    }
    runs
}

/// Flush the accumulated text as a [`Run`] with the current bold/italic flags.
/// The single `mem::take`-into-a-run helper shared by the export run-splitter and
/// the import RTF run-collector (#506).
pub fn push_run(runs: &mut Vec<Run>, cur: &mut String, bold: bool, italic: bool) {
    if !cur.is_empty() {
        runs.push(Run::new(
            std::mem::take(cur),
            TextStyle::from_flags(bold, italic),
        ));
    }
}

/// Escape a string for embedding in XML (the `.docx`, `.scrivx`, and EPUB
/// markup), safe for both element text and attribute values.
///
/// Escapes the five XML predefined entities — `&`, `<`, `>`, `"`, `'`. The
/// quote escapes matter because this function feeds attribute values too
/// (e.g. the EPUB `<img … alt="{}"/>`, #297/#818): a title like
/// `The "Real" Story` would otherwise close the `alt` attribute early and
/// produce non-well-formed XHTML that epubcheck and conformant readers reject.
/// Escaping quotes in element text is harmless, so one shared escaper covers
/// every site.
///
/// First strips the C0 control characters that XML 1.0 forbids outright
/// (U+0000–U+0008, U+000B, U+000C, U+000E–U+001F) — these cannot be escaped
/// and, left raw, corrupt the whole document (Word / epubcheck reject it).
/// Tab, newline, and carriage return are legal and kept.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            _ if is_forbidden_xml_char(c) => {}
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Whether a character is a C0 control disallowed by XML 1.0 and thus
/// unrepresentable in any XML document — even escaped. Tab (`\t`), line feed
/// (`\n`), and carriage return (`\r`) are the three C0 controls XML permits, so
/// they're excluded here.
const fn is_forbidden_xml_char(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}')
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
    /// A stanza of a verse chapter (#1327): each source line kept as a line.
    Stanza(Vec<verse::VerseLine>),
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
/// In a verse chapter (`verse`), a block that would be a paragraph is a stanza
/// instead (#1327).
fn prose_blocks(body: &str, verse: bool) -> impl Iterator<Item = Prose> {
    body_blocks(body)
        .into_iter()
        .filter_map(move |block| classify_block(&block, verse))
}

/// Classify one body block (already split via [`body_blocks`]), or `None` when
/// it's empty.
fn classify_block(block: &str, verse: bool) -> Option<Prose> {
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
    if verse {
        return Some(Prose::Stanza(verse::stanza(block)));
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
pub fn is_thematic_break(block: &str) -> bool {
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
mod tests;
