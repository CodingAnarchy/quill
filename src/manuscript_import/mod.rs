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
//! Scrivener `.scriv` is supported too (#402) via [`parse_scriv`], the inverse of
//! `export::scriv`: read the `.scrivx` binder for the Manuscript draft folder's
//! order and titles (folders flatten into their text documents, since the chapter
//! model is flat), and each text document's `Files/Data/<UUID>/content.rtf`,
//! converting RTF bold/italic runs to `**`/`*`.
//!
//! # Layout
//!
//! Each import format lives in its own module — [`markdown`], [`docx`], and
//! [`scriv`] — while the shared output types ([`Imported`]/[`ImportedChapter`]),
//! the chapter-assembly helpers ([`finalize_titles`]/[`wrap_emphasis`]/
//! [`join_paragraphs`]), and the hand-rolled XML scanner the `.docx` and `.scriv`
//! parsers share live here. The public parse functions are re-exported, so
//! `crate::manuscript_import::parse_*` callers are unchanged.

mod docx;
mod markdown;
mod scriv;

pub use docx::parse_docx;
pub use markdown::parse_markdown;
pub use scriv::parse_scriv;

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
/// [`unreadable_bodies`]: Imported::unreadable_bodies
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
}

/// One imported chapter: its heading text and its prose body.
#[derive(Debug, PartialEq, Eq)]
pub struct ImportedChapter {
    pub title: String,
    pub body: String,
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

/// Invoke `f` for each `<name>…</name>` element in `xml`, in order, with its
/// inner XML — or `f(None)` for a self-closing `<name/>`. Centralizes the
/// open/find-close/advance bookkeeping the paragraph, run, and text scans share.
fn for_each_element(xml: &str, name: &str, mut f: impl FnMut(Option<&str>)) {
    let close = format!("</{name}>");
    let mut i = 0;
    while let Some(open) = next_element(xml, i, name) {
        if open.self_closing {
            f(None);
            i = open.after;
            continue;
        }
        let Some(rel) = xml[open.after..].find(&close) else {
            return;
        };
        f(Some(&xml[open.after..open.after + rel]));
        i = open.after + rel + close.len();
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
        match after.find(';').map(|semi| (&after[1..semi], semi)) {
            Some((entity, semi)) => match decode_entity(entity) {
                Some(c) => {
                    out.push(c);
                    rest = &after[semi + 1..];
                }
                None => {
                    out.push('&');
                    rest = &after[1..];
                }
            },
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
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
