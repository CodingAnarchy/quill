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
//! becomes one chapter.
//!
//! Word `.docx` is also supported (#401) via [`parse_docx`]: a `.docx` is a zip
//! of XML, so we read `word/document.xml`, split chapters on Heading 1
//! paragraphs (the level Quill's own Word/Vellum export writes chapter titles
//! at), turn bold/italic runs into `**`/`*`, and read the title/author from the
//! OOXML core properties (or, failing that, a title-block preamble). The OOXML is
//! scanned directly — no XML-parser dependency, mirroring the hand-written XML on
//! the export side.
//!
//! OpenDocument `.odt` is supported too (#836) via [`parse_odt`], mirroring the
//! `.docx` path: an `.odt` is a zip of XML, so we read `content.xml`, split
//! chapters on outline-level-1 `text:h` headings, turn bold/italic `text:span`s
//! (whose emphasis lives in the automatic styles) into `**`/`*`, and read the
//! title/author from `meta.xml` (or a title-block preamble).
//!
//! Scrivener `.scriv` is supported too (#402) via [`parse_scriv`], the inverse of
//! `export::scriv`: read the `.scrivx` binder for the Manuscript draft folder's
//! order and titles (folders flatten into their text documents, since the chapter
//! model is flat), and each text document's `Files/Data/<UUID>/content.rtf`,
//! converting RTF bold/italic runs to `**`/`*`.
//!
//! Plain-text `.txt` is supported (#593) via [`parse_text`]: a `.txt` has no
//! headings, so chapters split on `Chapter N`-style lines when present, else on
//! wide blank-line gaps. A folder of per-chapter `.md` files is supported (#593)
//! via [`parse_markdown_folder`]: one chapter per `.md` file (in filename order),
//! titled by its leading `#` heading or its filename.
//!
//! # Layout
//!
//! Each import format lives in its own module — [`markdown`], [`docx`], [`odt`],
//! [`scriv`], [`text`], and [`folder`] — while the shared output types
//! ([`Imported`]/[`ImportedChapter`]),
//! the chapter-assembly helpers ([`finalize_titles`]/[`wrap_emphasis`]/
//! [`join_paragraphs`]), and the hand-rolled XML scanner the `.docx` and `.scriv`
//! parsers share live here. The public parse functions are re-exported, so
//! `crate::manuscript_import::parse_*` callers are unchanged.

mod docx;
mod folder;
mod markdown;
mod odt;
mod scriv;
mod text;

pub use docx::{extract_comments, parse_docx};
pub use folder::parse_markdown_folder;
pub use markdown::{chapter_level, heading_count, parse_markdown, parse_markdown_at};
pub use odt::parse_odt;
pub use scriv::parse_scriv;
pub use text::parse_text;

/// A tally of structural elements the scanners recognized but couldn't carry
/// into Quill's flat prose model, so the caller can warn the author rather than
/// let them vanish silently (#646).
///
/// The `.docx`/`.scriv` scanners extract paragraph text and bold/italic runs;
/// footnotes, endnotes, images, tables, and comments have no home in the chapter
/// body. Counting them keeps the "surface it, never silently drop it" contract
/// (see [`Imported`]) for structural content the way [`unreadable_bodies`] does
/// for unreadable bodies. Footnotes and endnotes are the one exception: the
/// scanners convert them to inline `[^N]` markers rather than dropping them, so a
/// non-zero note count is a "converted", not "lost", tally.
///
/// [`unreadable_bodies`]: Imported::unreadable_bodies
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Dropped {
    /// Footnotes seen — converted to inline `[^N]` markers, not dropped.
    pub footnotes: usize,
    /// Endnotes seen — converted to inline `[^N]` markers, not dropped.
    pub endnotes: usize,
    /// Images/drawings dropped (no place in the flat prose model).
    pub images: usize,
    /// Tables dropped.
    pub tables: usize,
    /// Comments/annotations dropped.
    pub comments: usize,
}

impl Dropped {
    /// How many notes were converted to inline markers (footnotes + endnotes).
    pub const fn notes(&self) -> usize {
        self.footnotes + self.endnotes
    }

    /// How many elements were lost outright — everything but footnotes/endnotes,
    /// which are converted to inline markers. Zero for a clean document.
    #[cfg(test)]
    pub const fn lost(&self) -> usize {
        self.images + self.tables + self.comments
    }
}

/// A manuscript parsed into book metadata and chapters.
///
/// # Unreadable-content policy
///
/// The three importers share one contract for content that the source *lists*
/// but whose body can't be read: surface it, never silently drop it.
///
/// - **Markdown** is infallible (text in, chapters out) — there's no per-chapter
///   I/O to fail, so nothing to count.
/// - **`.docx`** and **`.scriv`** return `io::Result`: a missing/corrupt
///   *container* (`word/document.xml`, the `.scrivx` binder) is a hard error.
///   `.docx` carries all its bodies inside that one container, so once it's read
///   there is no analogous per-body failure to count.
/// - **`.scriv`** stores each document's body in a separate `content.rtf` file,
///   so an individual body can be unreadable while the binder is fine. Rather
///   than swallow that into an empty chapter, [`parse_scriv`] counts those drops
///   in [`unreadable_bodies`] so the caller can warn the author.
///
/// # Dropped-element policy (#646)
///
/// The same surface-it contract extends to structural content the scanners can
/// recognize but not represent — footnotes, endnotes, images, tables, comments.
/// The `.docx`/`.scriv` scanners tally these in [`dropped`] (footnotes/endnotes
/// are converted to inline `[^N]` markers, the rest are lost) so the caller can
/// warn rather than report a clean success. The other formats leave it default
/// (zero): plain text and per-file Markdown carry no such structure.
///
/// [`unreadable_bodies`]: Imported::unreadable_bodies
/// [`dropped`]: Imported::dropped
#[derive(Debug, PartialEq, Eq)]
pub struct Imported {
    /// The book title, from front matter or the title-page `#` heading.
    pub title: Option<String>,
    /// The author, from front matter or a "by …" title-page line.
    pub author: Option<String>,
    /// The chapters, in reading order (always at least one).
    pub chapters: Vec<ImportedChapter>,
    /// How many listed documents came in with an empty body because their
    /// content couldn't be read (only `.scriv`; 0 for the others). Surfaced by
    /// the caller so a partially-unreadable import isn't reported as a clean
    /// success.
    pub unreadable_bodies: usize,
    /// Structural elements the scanner recognized but couldn't carry into the
    /// flat prose model (only `.docx`/`.scriv`; default/zero for the others).
    /// Surfaced by the caller alongside `unreadable_bodies` (#646).
    pub dropped: Dropped,
}

/// One imported chapter: its heading text and its prose body.
#[derive(Debug, PartialEq, Eq)]
pub struct ImportedChapter {
    pub title: String,
    pub body: String,
}

/// The shared chapter-assembly state machine. Walk a format's token stream once,
/// splitting into chapters at each heading: the run of items before the first
/// heading is the preamble; each heading opens a chapter whose body is the items
/// up to the next heading (or EOF). Returns the chapters and the preamble items.
///
/// The three importers differ only in item type (`&str` Markdown lines vs the
/// `.docx` paragraphs) and how a chapter body is built from its items (`clean`
/// vs [`join_paragraphs`]), so those are parameters:
///
/// - `is_heading` — does this item open a new chapter? (and is it the heading,
///   not body)
/// - `title_of` — the chapter title for a heading item.
/// - `join_bodies` — assemble a chapter body from its (non-heading) items.
///
/// This is the single source of truth for the flush-on-heading / stash-preamble /
/// flush-at-EOF contract; the empty-chapters fallback and the [`finalize_titles`]
/// tail are applied by the caller (they synthesize different bodies per format).
fn assemble<T>(
    items: impl IntoIterator<Item = T>,
    is_heading: impl Fn(&T) -> bool,
    title_of: impl Fn(&T) -> String,
    join_bodies: impl Fn(&[T]) -> String,
) -> (Vec<ImportedChapter>, Vec<T>) {
    let mut chapters: Vec<ImportedChapter> = Vec::new();
    let mut preamble: Vec<T> = Vec::new();
    let mut cur_title: Option<String> = None;
    let mut cur_body: Vec<T> = Vec::new();
    for item in items {
        if is_heading(&item) {
            match cur_title.take() {
                Some(title) => chapters.push(ImportedChapter {
                    title,
                    body: join_bodies(&cur_body),
                }),
                // Items before the first heading are the preamble.
                None => preamble = std::mem::take(&mut cur_body),
            }
            cur_body.clear();
            cur_title = Some(title_of(&item));
        } else {
            cur_body.push(item);
        }
    }
    match cur_title {
        Some(title) => chapters.push(ImportedChapter {
            title,
            body: join_bodies(&cur_body),
        }),
        // No heading was ever seen — the whole stream is the preamble.
        None => preamble = cur_body,
    }
    (chapters, preamble)
}

/// Mine the preamble (the run of items before the first chapter heading) for any
/// title/author the format's own metadata didn't already supply, then return it.
///
/// One policy across formats (the divergence #501 fixes):
///
/// - The **author** "by …" line is mined *unconditionally* — even a heading-less
///   document, whose whole body is the preamble, has its by-line read. Previously
///   Markdown did this but `.docx` mined only when real chapters existed, so the
///   same heading-less input gave a different author by format.
/// - The **title** is mined only when there *are* real chapters (`has_chapters`),
///   i.e. the preamble is a genuine title page rather than the whole manuscript —
///   so a heading-less document keeps its prose as a chapter body and falls back
///   to the `"Manuscript"` title rather than promoting its first line. (Markdown
///   already had this shape: its title-page `#` only appears above chapters.)
///
/// `title_from` is the format's title-page title extractor (Markdown's `#`
/// heading vs the `.docx` first non-empty line); the author rule is shared.
fn mine_preamble<S: AsRef<str>>(
    preamble: &[S],
    mut title: Option<String>,
    mut author: Option<String>,
    has_chapters: bool,
    title_from: impl FnOnce(&[S]) -> Option<String>,
) -> (Option<String>, Option<String>) {
    if title.is_none() && has_chapters {
        title = title_from(preamble);
    }
    if author.is_none() {
        author = preamble_author(preamble);
    }
    (title, author)
}

/// A "by …" attribution line on the title page (Quill's export writes one),
/// ignoring surrounding emphasis markers. Shared by every format's preamble
/// mining (over Markdown lines or `.docx` paragraphs alike).
fn preamble_author<S: AsRef<str>>(preamble: &[S]) -> Option<String> {
    for line in preamble {
        let s = line
            .as_ref()
            .trim()
            .trim_matches(|c| c == '*' || c == '_')
            .trim();
        if s.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("by ")) {
            let a = s[3..].trim();
            if !a.is_empty() {
                return Some(a.to_string());
            }
        }
    }
    None
}

/// The shared empty-stream fallback: when a parse produced no chapters, the
/// source still becomes one chapter — titled the mined book title (or
/// `"Manuscript"`), with `body` (the preamble's text for the file formats, empty
/// for `.scriv`). Centralizes the synthesize-one-chapter rule every parse shares.
fn fallback_chapter(chapters: &mut Vec<ImportedChapter>, title: &Option<String>, body: String) {
    if chapters.is_empty() {
        let only = title.clone().unwrap_or_else(|| "Manuscript".into());
        chapters.push(ImportedChapter { title: only, body });
    }
}

/// Give every chapter a usable, trimmed title — a bare or blank heading falls
/// back to `Chapter N`. Shared final step of every parse.
fn finalize_titles(chapters: &mut [ImportedChapter]) {
    for (i, c) in chapters.iter_mut().enumerate() {
        c.title = if c.title.trim().is_empty() {
            format!("Chapter {}", i + 1)
        } else {
            c.title.trim().to_string()
        };
    }
}

/// Wrap `text` in `**`/`*`/`***` for bold/italic, keeping leading/trailing
/// whitespace outside the markers so emphasis stays tight. Shared by the `.docx`
/// run reader and the `.scriv` RTF reader.
fn wrap_emphasis(text: &str, bold: bool, italic: bool) -> String {
    if text.trim().is_empty() || !(bold || italic) {
        return text.to_string();
    }
    let marks = if bold && italic {
        "***"
    } else if bold {
        "**"
    } else {
        "*"
    };
    let lead = &text[..text.len() - text.trim_start().len()];
    let trail = &text[text.trim_end().len()..];
    format!("{lead}{marks}{}{marks}{trail}", text.trim())
}

/// Join paragraph texts into a chapter body: one blank line between paragraphs
/// (empty/spacing paragraphs dropped), a single trailing newline like the
/// Markdown path's `clean`.
fn join_paragraphs(paras: &[String]) -> String {
    let kept: Vec<&str> = paras
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    let joined = kept.join("\n\n");
    if joined.is_empty() {
        String::new()
    } else {
        format!("{joined}\n")
    }
}

// --- Shared OOXML/XML scanner ---------------------------------------------
//
// A tiny hand-rolled XML scanner (no XML-parser dependency, mirroring the
// hand-written XML on the export side), shared by the `.docx` and `.scriv`
// parsers.

/// Where an opening tag ends: the byte index just past its `>`, and whether it
/// self-closes (`…/>`).
struct ElementOpen {
    after: usize,
    self_closing: bool,
}

/// The span of the next `<name …>` / `<name/>` opening tag at or after `from`:
/// `(start of attributes, index of the closing '>', self-closing?)`. Skips longer
/// names that merely share the prefix (`<w:p>` matches, `<w:pPr>` doesn't). The
/// single source of truth for the tricky prefix disambiguation; [`next_element`]
/// and [`tag_attrs`] are thin projections of it.
fn find_open(xml: &str, from: usize, name: &str) -> Option<(usize, usize, bool)> {
    let pat = format!("<{name}");
    let mut search = from;
    loop {
        let at = search + xml[search..].find(&pat)?;
        let after_name = at + pat.len();
        match xml[after_name..].chars().next()? {
            c if c == '>' || c == '/' || c.is_whitespace() => {
                let gt = after_name + xml[after_name..].find('>')?;
                return Some((after_name, gt, xml[..gt].ends_with('/')));
            }
            _ => search = after_name, // a longer name sharing the prefix; keep looking
        }
    }
}

/// Find the next `<name …>` / `<name/>` element at or after `from`.
fn next_element(xml: &str, from: usize, name: &str) -> Option<ElementOpen> {
    find_open(xml, from, name).map(|(_, gt, self_closing)| ElementOpen {
        after: gt + 1,
        self_closing,
    })
}

/// The attribute substring of the first `<name …>` element in `xml` (the text
/// between the name and the closing `>`), or `None` if absent.
fn tag_attrs<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    find_open(xml, 0, name).map(|(start, gt, _)| xml[start..gt].trim_end_matches('/').trim())
}

/// The single nesting-aware traversal primitive: invoke `f` for each
/// `<name …>…</name>` / `<name …/>` element in `xml`, in order, passing its
/// attribute substring and its inner XML — `None` inner for a self-closing tag.
/// Each element is bounded by [`matching_close`], so a same-named element nested
/// inside another is part of the outer element's inner XML, not a separate top
/// item. This is the one source of truth for the open/close/advance-with-depth
/// bookkeeping every scan shares; [`for_each_element`] and the `.scrivx` binder
/// walk are thin projections of it.
fn for_each_child<'a>(xml: &'a str, name: &str, mut f: impl FnMut(&'a str, Option<&'a str>)) {
    let close = format!("</{name}>");
    let mut pos = 0;
    while let Some((attrs_start, gt, self_closing)) = find_open(xml, pos, name) {
        let attrs = xml[attrs_start..gt].trim_end_matches('/').trim();
        if self_closing {
            f(attrs, None);
            pos = gt + 1;
            continue;
        }
        let Some(at) = matching_close(xml, name, gt + 1) else {
            return;
        };
        f(attrs, Some(&xml[gt + 1..at]));
        pos = at + close.len();
    }
}

/// Invoke `f` for each `<name>…</name>` element in `xml`, in order, with its
/// inner XML — or `f(None)` for a self-closing `<name/>`. A nesting-aware
/// projection of [`for_each_child`] that drops the attributes; centralizes the
/// open/find-close/advance bookkeeping the paragraph, run, and text scans share.
fn for_each_element(xml: &str, name: &str, mut f: impl FnMut(Option<&str>)) {
    for_each_child(xml, name, |_attrs, inner| f(inner));
}

/// How many `<name …>` opening tags appear in `xml` (self-closing or not),
/// counting every occurrence regardless of nesting. Used by the `.docx` scanner
/// to tally dropped structural elements (images, tables, comments) it can't
/// carry into the flat prose model (#646).
fn count_elements(xml: &str, name: &str) -> usize {
    let mut count = 0;
    let mut pos = 0;
    while let Some((_, gt, _)) = find_open(xml, pos, name) {
        count += 1;
        pos = gt + 1;
    }
    count
}

/// The byte index of the `</name>` that closes the element whose content starts at
/// `inner_start`, accounting for nested same-named elements. The depth bookkeeping
/// at the heart of [`for_each_child`].
fn matching_close(xml: &str, name: &str, inner_start: usize) -> Option<usize> {
    let close = format!("</{name}>");
    let mut depth = 1usize;
    let mut pos = inner_start;
    loop {
        let next_open = find_open(xml, pos, name);
        let next_close = xml[pos..].find(&close).map(|r| pos + r);
        match (next_open, next_close) {
            (Some((_, gt, self_closing)), Some(at)) if gt < at => {
                if !self_closing {
                    depth += 1;
                }
                pos = gt + 1;
            }
            (_, Some(at)) => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
                pos = at + close.len();
            }
            _ => return None,
        }
    }
}

/// The value of attribute `key` (`key="value"`) in an attribute string,
/// entity-decoded.
fn attr_value(attrs: &str, key: &str) -> Option<String> {
    let pat = format!("{key}=\"");
    let at = attrs.find(&pat)? + pat.len();
    let end = attrs[at..].find('"')?;
    Some(decode_entities(&attrs[at..at + end]))
}

/// The text of the first `<name>…</name>` element, trimmed and entity-decoded;
/// `None` when absent, self-closing, or empty. For the core properties.
fn element_text(xml: &str, name: &str) -> Option<String> {
    let open = next_element(xml, 0, name)?;
    if open.self_closing {
        return None;
    }
    let rel = xml[open.after..].find(&format!("</{name}>"))?;
    let text = decode_entities(xml[open.after..open.after + rel].trim());
    (!text.is_empty()).then_some(text)
}

/// Decode the XML entities that appear in OOXML text: the five named ones and
/// numeric (`&#NN;` / `&#xHH;`); an unrecognized `&…` is left as-is.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        if let Some((entity, semi)) = after.find(';').map(|semi| (&after[1..semi], semi)) {
            if let Some(c) = decode_entity(entity) {
                out.push(c);
                rest = &after[semi + 1..];
            } else {
                out.push('&');
                rest = &after[1..];
            }
        } else {
            out.push('&');
            rest = &after[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The most uncompressed bytes a single zip-entry read (`word/document.xml`,
/// `content.xml`, and their siblings) will decompress before giving up. An
/// import's payload is XML — a manuscript's prose plus markup — so even a large
/// novel's `document.xml` is a few tens of megabytes; 512 MiB is orders of
/// magnitude over any real document yet refuses an entry that would expand
/// without bound and exhaust memory. Mirrors `archive.rs`'s whole-archive cap,
/// scaled to a per-entry import: the caller reads only a handful of named
/// entries, not a whole project tree.
const MAX_ENTRY_UNCOMPRESSED: u64 = 512 * 1024 * 1024;

/// The largest compression ratio (uncompressed ÷ compressed) any single entry is
/// allowed, matching `archive.rs`'s bomb guard. Ordinary XML deflates a few-fold
/// (tens at most for very repetitive markup); a classic zip bomb balloons a tiny
/// entry thousandsfold. 1000 leaves honest markup ample room while catching the
/// bomb; a floor on the compressed size keeps the ratio from tripping on
/// tiny-but-innocent entries whose few bytes divide badly.
const MAX_ENTRY_RATIO: u64 = 1000;

/// Below this compressed size an entry's declared ratio isn't checked: a handful
/// of compressed bytes can legitimately expand past the ratio without being a
/// bomb, and the streamed size cap still bounds the total.
const RATIO_CHECK_FLOOR: u64 = 4 * 1024;

/// The error an import fails with when a zip entry would expand past the size or
/// ratio caps — a likely decompression bomb. Mirrors `archive.rs`'s restore
/// guard so a hostile `.docx`/`.odt` is rejected with a clear message, not an OOM.
fn bomb_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "archive rejected: an entry expands too large (possible zip bomb)",
    )
}

/// Read a zip entry to a `String` under a bounded, streaming decompression that
/// can't OOM on a hostile archive (#1098). `compressed` and `declared` are the
/// entry's stored/uncompressed sizes from its header (both untrusted); `reader`
/// is the entry's decompressing stream.
///
/// Import runs on untrusted input — a `.docx`/`.odt` is a zip a stranger may have
/// crafted — so the entry can't be slurped with `read_to_string`, which trusts
/// the declared size and would allocate (or expand) without bound on a zip bomb.
/// Instead this checks the declared ratio as a cheap first filter, then streams
/// through a fixed buffer that errors the instant the real decompressed length
/// crosses [`MAX_ENTRY_UNCOMPRESSED`] — the header size is never trusted as the
/// ceiling. A valid document (well under the cap) reads through unchanged.
fn read_zip_entry_bounded(
    reader: &mut impl std::io::Read,
    compressed: u64,
    declared: u64,
) -> std::io::Result<String> {
    // Guard the declared ratio before decompressing a byte: a classic bomb is a
    // tiny compressed entry that balloons thousandsfold. The declared size is
    // untrusted, so this is only a cheap first filter; the streamed cap below is
    // the real ceiling.
    if compressed >= RATIO_CHECK_FLOOR && declared / compressed.max(1) > MAX_ENTRY_RATIO {
        return Err(bomb_error());
    }
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut read: u64 = 0;
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        // Reading past the cap means the entry is larger than any real import —
        // refuse it rather than keep expanding a possible bomb.
        read += n as u64;
        if read > MAX_ENTRY_UNCOMPRESSED {
            return Err(bomb_error());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8(buf).map_err(std::io::Error::other)
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let num = entity.strip_prefix('#')?;
            let code = match num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `for_each_child` yields only the top-level elements, with a same-named
    /// element nested inside the first one folded into that element's inner XML —
    /// not surfaced as a separate top-level item. Two levels of nesting plus a
    /// flat sibling exercise the depth bookkeeping in both directions.
    #[test]
    fn for_each_child_bounds_deeper_and_sibling_nesting() {
        // <g a="1"> contains a nested <g> which itself contains a <g>; then a
        // flat sibling <g a="2"/> self-closes at the top level.
        let xml = r#"<g a="1">x<g a="11">y<g a="111">z</g></g>w</g><g a="2"/>"#;
        let mut seen: Vec<(String, Option<String>)> = Vec::new();
        for_each_child(xml, "g", |attrs, inner| {
            seen.push((attrs.to_string(), inner.map(str::to_string)));
        });
        assert_eq!(
            seen,
            vec![
                (
                    "a=\"1\"".to_string(),
                    // the whole nested subtree is the first element's inner XML
                    Some(r#"x<g a="11">y<g a="111">z</g></g>w"#.to_string()),
                ),
                ("a=\"2\"".to_string(), None),
            ],
            "only the two top-level <g> are yielded; nesting is bounded"
        );
    }

    /// `matching_close` skips over nested same-named elements (and self-closing
    /// ones, which don't open a level) to find the close that balances the open.
    #[test]
    fn matching_close_balances_nested_elements() {
        let xml = r#"<g><g/><g>inner</g></g>after"#;
        // The content of the outer <g> starts just past its '>': index 3.
        let inner_start = 3;
        let at = matching_close(xml, "g", inner_start).unwrap();
        assert_eq!(&xml[inner_start..at], "<g/><g>inner</g>");
        // What follows the balancing close is the trailing text.
        assert_eq!(&xml[at + "</g>".len()..], "after");
    }
}
