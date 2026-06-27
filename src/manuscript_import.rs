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

use std::io::{Cursor, Read};
use std::path::Path;

use crate::export::SCENE_BREAK;
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
                Some(t) => chapters.push(ImportedChapter {
                    title: t.into(),
                    body: clean(&cur_body),
                }),
                None => preamble = std::mem::take(&mut cur_body),
            }
            cur_body.clear();
            cur_title = Some(heading_text(line));
            continue;
        }
        cur_body.push(line);
    }
    match cur_title {
        Some(t) => chapters.push(ImportedChapter {
            title: t.into(),
            body: clean(&cur_body),
        }),
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

    finalize_titles(&mut chapters);

    Imported {
        title,
        author,
        chapters,
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

/// Give every chapter a usable, trimmed title — a bare or blank heading falls
/// back to `Chapter N`. Shared final step of both the Markdown and `.docx` parses.
fn finalize_titles(chapters: &mut [ImportedChapter]) {
    for (i, c) in chapters.iter_mut().enumerate() {
        c.title = if c.title.trim().is_empty() {
            format!("Chapter {}", i + 1)
        } else {
            c.title.trim().to_string()
        };
    }
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

// --- Word (.docx) import (#401) -------------------------------------------

/// Parse a Word `.docx` manuscript: read `word/document.xml` (and the core
/// properties) out of the zip and split it into chapters. The bytes are read
/// only — the caller never alters the source. Errors if the zip is unreadable or
/// has no `word/document.xml` (i.e. it isn't a Word document).
pub fn parse_docx(bytes: &[u8]) -> std::io::Result<Imported> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(std::io::Error::other)?;
    let document = zip_entry(&mut zip, "word/document.xml")?
        .ok_or_else(|| std::io::Error::other("not a Word document (no word/document.xml)"))?;
    let core = zip_entry(&mut zip, "docProps/core.xml")?;
    Ok(parse_document_xml(&document, core.as_deref()))
}

/// Read one entry of a `.docx` zip to a string, or `None` if it's absent.
fn zip_entry(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> std::io::Result<Option<String>> {
    match zip.by_name(name) {
        Ok(mut f) => {
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            Ok(Some(s))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(std::io::Error::other(e)),
    }
}

/// One Word paragraph: whether it's a Heading 1 (a chapter break) and its text
/// (with `*`/`**` emphasis for a body paragraph, plain for a heading title).
struct Para {
    heading: bool,
    text: String,
}

/// The pure core of [`parse_docx`]: turn the body XML (and optional core-property
/// XML) into the same [`Imported`] shape the Markdown importer produces. Splits
/// on Heading 1 paragraphs; the preamble before the first heading is a title page
/// (mined for title/author only when the core properties don't supply them).
fn parse_document_xml(document: &str, core: Option<&str>) -> Imported {
    let mut title = core.and_then(|c| element_text(c, "dc:title"));
    let mut author = core.and_then(|c| element_text(c, "dc:creator"));

    let mut chapters: Vec<ImportedChapter> = Vec::new();
    let mut preamble: Vec<String> = Vec::new();
    let mut cur_title: Option<String> = None;
    let mut cur_body: Vec<String> = Vec::new();
    for para in paragraphs(document) {
        if para.heading {
            match cur_title.take() {
                Some(t) => chapters.push(ImportedChapter {
                    title: t,
                    body: join_paragraphs(&cur_body),
                }),
                None => preamble = std::mem::take(&mut cur_body),
            }
            cur_body.clear();
            cur_title = Some(para.text);
        } else {
            cur_body.push(para.text);
        }
    }
    match cur_title {
        Some(t) => chapters.push(ImportedChapter {
            title: t,
            body: join_paragraphs(&cur_body),
        }),
        None => preamble = cur_body,
    }

    // A title page (the preamble before the first heading) fills in anything the
    // core properties didn't — the first non-empty line as the title, a "by …"
    // line as the author. Only when there are real chapters, so a heading-less
    // document keeps its whole text as the one chapter's body.
    if !chapters.is_empty() {
        if title.is_none() {
            title = preamble
                .iter()
                .map(|l| l.trim())
                .find(|l| !l.is_empty())
                .map(str::to_string);
        }
        if author.is_none() {
            let refs: Vec<&str> = preamble.iter().map(String::as_str).collect();
            author = preamble_author(&refs);
        }
    }

    if chapters.is_empty() {
        let only = title.clone().unwrap_or_else(|| "Manuscript".into());
        chapters.push(ImportedChapter {
            title: only,
            body: join_paragraphs(&preamble),
        });
    }
    finalize_titles(&mut chapters);

    Imported {
        title,
        author,
        chapters,
    }
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

/// Walk the `<w:p>` paragraphs of `word/document.xml`, in order.
fn paragraphs(document: &str) -> Vec<Para> {
    let mut out = Vec::new();
    for_each_element(document, "w:p", |inner| {
        out.push(match inner {
            Some(p) => parse_paragraph(p),
            None => Para {
                heading: false,
                text: String::new(),
            },
        });
    });
    out
}

/// One paragraph's inner XML → a [`Para`]. A Heading 1 paragraph is a chapter
/// title (plain text); any other is a body paragraph with bold/italic runs
/// converted to `**`/`*`.
fn parse_paragraph(inner: &str) -> Para {
    if is_heading_one(inner) {
        return Para {
            heading: true,
            text: run_text(inner),
        };
    }
    let mut text = String::new();
    for_each_element(inner, "w:r", |run| {
        if let Some(run) = run {
            text.push_str(&run_markdown(run));
        }
    });
    Para {
        heading: false,
        text,
    }
}

/// Whether a paragraph is styled Heading 1 — by its paragraph-style id
/// (`Heading1`, case/space-insensitive; the id is stable across Word's
/// display-name localization, and is what Quill's export writes) or an explicit
/// outline level 0. A heading styled only by inheritance in `styles.xml` (which we
/// don't read) isn't detected — it folds into the previous chapter's body rather
/// than corrupting output.
fn is_heading_one(inner: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    if let Some(style) = tag_attrs(inner, "w:pStyle").and_then(|a| attr_value(a, "w:val")) {
        if norm(&style) == "heading1" {
            return true;
        }
    }
    tag_attrs(inner, "w:outlineLvl")
        .and_then(|a| attr_value(a, "w:val"))
        .as_deref()
        == Some("0")
}

/// One run's inner XML → Markdown (its `<w:t>` text wrapped per its bold/italic
/// run properties).
fn run_markdown(run: &str) -> String {
    wrap_emphasis(
        &run_text(run),
        run_toggle(run, "w:b"),
        run_toggle(run, "w:i"),
    )
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

/// All `<w:t>` text within `xml`, concatenated and entity-decoded.
fn run_text(xml: &str) -> String {
    let mut out = String::new();
    for_each_element(xml, "w:t", |t| {
        if let Some(t) = t {
            out.push_str(&decode_entities(t));
        }
    });
    out
}

/// Whether a run-property toggle (`w:b` / `w:i`) is on: present and not turned
/// off by `w:val="false|0|none|off"`.
fn run_toggle(run: &str, name: &str) -> bool {
    match tag_attrs(run, name) {
        None => false,
        Some(attrs) => match attr_value(attrs, "w:val") {
            Some(v) => !matches!(v.as_str(), "false" | "0" | "none" | "off"),
            None => true,
        },
    }
}

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

// --- Scrivener (.scriv) import (#402) -------------------------------------

/// Parse a Scrivener `.scriv` bundle directory: read its `.scrivx` binder for the
/// Manuscript draft folder's reading order and titles, and each text document's
/// `Files/Data/<UUID>/content.rtf`. The bundle is read only. Errors if there's no
/// `.scrivx` binder (i.e. it isn't a Scrivener project).
pub fn parse_scriv(bundle: &Path) -> std::io::Result<Imported> {
    let scrivx_path = std::fs::read_dir(bundle)?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("scrivx"))
        })
        .ok_or_else(|| std::io::Error::other("not a Scrivener project (no .scrivx binder)"))?;
    let scrivx = std::fs::read_to_string(&scrivx_path)?;
    // Scrivener's project name is the bundle's name; it doesn't store a separate
    // title/author in the binder Quill writes.
    let title = bundle
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let data = bundle.join("Files").join("Data");
    Ok(parse_binder(&scrivx, title, |uuid| {
        std::fs::read_to_string(data.join(uuid).join("content.rtf")).ok()
    }))
}

/// The pure core of [`parse_scriv`]: walk the binder XML's Manuscript draft folder
/// into ordered chapters, pulling each text document's body from `read_rtf(uuid)`.
/// Folders are flattened into their text documents (the chapter model is flat);
/// the Research and Trash folders are skipped (only the draft folder is walked).
fn parse_binder(
    scrivx: &str,
    title: Option<String>,
    read_rtf: impl Fn(&str) -> Option<String>,
) -> Imported {
    let mut chapters = Vec::new();
    if let Some(draft) = draft_folder(scrivx).and_then(children_group) {
        collect_items(draft, &read_rtf, &mut chapters);
    }
    if chapters.is_empty() {
        let only = title.clone().unwrap_or_else(|| "Manuscript".into());
        chapters.push(ImportedChapter {
            title: only,
            body: String::new(),
        });
    }
    finalize_titles(&mut chapters);
    Imported {
        title,
        author: None,
        chapters,
    }
}

/// The inner XML of the `DraftFolder` binder item (the "Manuscript" folder).
fn draft_folder(scrivx: &str) -> Option<&str> {
    let mut pos = 0;
    while let Some((attrs_start, gt, _)) = find_open(scrivx, pos, "BinderItem") {
        let attrs = scrivx[attrs_start..gt].trim_end_matches('/').trim();
        let close = matching_close(scrivx, "BinderItem", gt + 1)?;
        if attr_value(attrs, "Type").as_deref() == Some("DraftFolder") {
            return Some(&scrivx[gt + 1..close]);
        }
        pos = close + "</BinderItem>".len();
    }
    None
}

/// Walk a binder item's direct children, in order: each `Text` document becomes a
/// chapter (its `content.rtf` converted from RTF); a folder recurses (its title is
/// dropped — the chapter list is flat). Nesting-aware so folders nest correctly.
fn collect_items(
    children: &str,
    read_rtf: &impl Fn(&str) -> Option<String>,
    out: &mut Vec<ImportedChapter>,
) {
    let mut pos = 0;
    while let Some((attrs_start, gt, self_closing)) = find_open(children, pos, "BinderItem") {
        let attrs = children[attrs_start..gt].trim_end_matches('/').trim();
        if self_closing {
            pos = gt + 1;
            continue;
        }
        let Some(close) = matching_close(children, "BinderItem", gt + 1) else {
            break;
        };
        let inner = &children[gt + 1..close];
        if attr_value(attrs, "Type").as_deref() == Some("Text") {
            let uuid = attr_value(attrs, "UUID").unwrap_or_default();
            let body = read_rtf(&uuid)
                .map(|rtf| rtf_to_markdown(&rtf))
                .unwrap_or_default();
            out.push(ImportedChapter {
                title: element_text(inner, "Title").unwrap_or_default(),
                body,
            });
        }
        // Recurse into any children (a folder's documents, or a text item's own).
        if let Some(grandchildren) = children_group(inner) {
            collect_items(grandchildren, read_rtf, out);
        }
        pos = close + "</BinderItem>".len();
    }
}

/// The inner XML of the first direct `<Children>` element, or `None`.
fn children_group(item: &str) -> Option<&str> {
    let (_, gt, _) = find_open(item, 0, "Children")?;
    let close = matching_close(item, "Children", gt + 1)?;
    Some(&item[gt + 1..close])
}

/// The byte index of the `</name>` that closes the element whose content starts at
/// `inner_start`, accounting for nested same-named elements.
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

/// Convert a document's RTF to Markdown: each `\par` paragraph becomes a line,
/// bold/italic runs (`\b` / `\i`, including `{\i …}` groups) become `**`/`*`, and
/// the centered scene-break glyph becomes a `---` thematic break. Best-effort: it
/// skips font/colour/style tables and unknown control words, decodes `\uN`/`\'XX`,
/// and otherwise emits text — enough for prose RTF (round-trips Quill's export).
fn rtf_to_markdown(rtf: &str) -> String {
    let chars: Vec<char> = rtf.chars().collect();
    let mut paras: Vec<String> = Vec::new();
    let mut runs: Vec<(String, bool, bool)> = Vec::new();
    let mut cur = String::new();
    let (mut bold, mut italic, mut skipping) = (false, false, false);
    let mut stack: Vec<(bool, bool, bool)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                flush_run(&mut runs, &mut cur, bold, italic);
                stack.push((bold, italic, skipping));
                i += 1;
            }
            '}' => {
                flush_run(&mut runs, &mut cur, bold, italic);
                if let Some((b, it, sk)) = stack.pop() {
                    (bold, italic, skipping) = (b, it, sk);
                }
                i += 1;
            }
            '\\' => {
                i += 1;
                let Some(&c) = chars.get(i) else { break };
                if c.is_ascii_alphabetic() {
                    let start = i;
                    while chars.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
                        i += 1;
                    }
                    let word: String = chars[start..i].iter().collect();
                    let num_start = i;
                    if chars.get(i).is_some_and(|&c| c == '-') {
                        i += 1;
                    }
                    while chars.get(i).is_some_and(char::is_ascii_digit) {
                        i += 1;
                    }
                    let param: Option<i64> =
                        chars[num_start..i].iter().collect::<String>().parse().ok();
                    if chars.get(i) == Some(&' ') {
                        i += 1; // a single trailing space delimits a control word
                    }
                    match word.as_str() {
                        "b" => {
                            flush_run(&mut runs, &mut cur, bold, italic);
                            bold = param != Some(0);
                        }
                        "i" => {
                            flush_run(&mut runs, &mut cur, bold, italic);
                            italic = param != Some(0);
                        }
                        "plain" => {
                            flush_run(&mut runs, &mut cur, bold, italic);
                            (bold, italic) = (false, false);
                        }
                        "par" => {
                            flush_run(&mut runs, &mut cur, bold, italic);
                            paras.push(runs_to_markdown(&runs));
                            runs.clear();
                        }
                        "line" | "tab" => {
                            if !skipping {
                                cur.push(' ');
                            }
                        }
                        "u" => {
                            if let Some(ch) =
                                param.and_then(|p| char::from_u32(p.rem_euclid(65536) as u32))
                            {
                                if !skipping {
                                    cur.push(ch);
                                }
                            }
                            // Skip the single substitution char that follows \uN.
                            if chars
                                .get(i)
                                .is_some_and(|&c| c != '\\' && c != '{' && c != '}')
                            {
                                i += 1;
                            }
                        }
                        w if is_skip_destination(w) => skipping = true,
                        _ => {} // unknown control word — no text
                    }
                } else {
                    match c {
                        '\\' | '{' | '}' => {
                            if !skipping {
                                cur.push(c);
                            }
                            i += 1;
                        }
                        '*' => {
                            skipping = true; // an ignorable destination group
                            i += 1;
                        }
                        '\'' => {
                            let hex: String =
                                chars[i + 1..(i + 3).min(chars.len())].iter().collect();
                            i += 3;
                            if !skipping {
                                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                                    cur.push(char::from(byte)); // ANSI byte (Latin-1)
                                }
                            }
                        }
                        _ => i += 1,
                    }
                }
            }
            '\n' | '\r' => i += 1, // raw newlines are not text in RTF
            c => {
                if !skipping {
                    cur.push(c);
                }
                i += 1;
            }
        }
    }
    flush_run(&mut runs, &mut cur, bold, italic);
    if !runs.is_empty() {
        paras.push(runs_to_markdown(&runs));
    }
    // The centered scene-break glyph round-trips back to a thematic break.
    for p in &mut paras {
        if p.trim() == SCENE_BREAK {
            *p = "---".to_string();
        }
    }
    join_paragraphs(&paras)
}

/// Render parsed RTF runs to Markdown, each wrapped per its bold/italic flags.
fn runs_to_markdown(runs: &[(String, bool, bool)]) -> String {
    runs.iter()
        .map(|(t, b, i)| wrap_emphasis(t, *b, *i))
        .collect()
}

/// Flush the accumulated text as a run with the current bold/italic flags.
fn flush_run(runs: &mut Vec<(String, bool, bool)>, cur: &mut String, bold: bool, italic: bool) {
    if !cur.is_empty() {
        runs.push((std::mem::take(cur), bold, italic));
    }
}

/// RTF destination groups whose text isn't document content (font/colour/style
/// tables, metadata) — skipped wholesale.
fn is_skip_destination(word: &str) -> bool {
    matches!(
        word,
        "fonttbl"
            | "colortbl"
            | "stylesheet"
            | "info"
            | "header"
            | "footer"
            | "pict"
            | "object"
            | "pgdsctbl"
            | "listtable"
            | "listoverridetable"
            | "rsidtbl"
            | "generator"
            | "themedata"
            | "colorschememapping"
            | "latentstyles"
            | "datastore"
    )
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

    // --- .docx import (#401) ---------------------------------------------

    /// Wrap paragraph XML in a `word/document.xml` shell.
    fn document(body: &str) -> String {
        format!("<w:document><w:body>{body}</w:body></w:document>")
    }

    /// A Heading 1 chapter-title paragraph.
    fn heading(title: &str) -> String {
        format!(
            "<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>{title}</w:t></w:r></w:p>"
        )
    }

    #[test]
    fn docx_splits_on_heading_one_and_converts_emphasis() {
        let body = format!(
            "{}{}{}",
            heading("One"),
            // "The sea was *grey* and it **ended**." across three runs.
            "<w:p><w:r><w:t xml:space=\"preserve\">The sea was </w:t></w:r>\
             <w:r><w:rPr><w:i/></w:rPr><w:t>grey</w:t></w:r>\
             <w:r><w:t xml:space=\"preserve\"> and it </w:t></w:r>\
             <w:r><w:rPr><w:b/></w:rPr><w:t>ended</w:t></w:r>\
             <w:r><w:t>.</w:t></w:r></w:p>",
            heading("Two"),
        );
        let out = parse_document_xml(&document(&body), None);
        assert_eq!(out.chapters.len(), 2);
        assert_eq!(out.chapters[0].title, "One");
        assert_eq!(
            out.chapters[0].body,
            "The sea was *grey* and it **ended**.\n"
        );
        assert_eq!(out.chapters[1].title, "Two");
    }

    #[test]
    fn docx_reads_title_and_author_from_core_properties() {
        let core = "<cp:coreProperties><dc:title>Salt and Iron</dc:title>\
                    <dc:creator>J. Reader</dc:creator></cp:coreProperties>";
        let out = parse_document_xml(&document(&heading("One")), Some(core));
        assert_eq!(out.title.as_deref(), Some("Salt and Iron"));
        assert_eq!(out.author.as_deref(), Some("J. Reader"));
    }

    #[test]
    fn docx_mines_a_title_block_when_core_props_are_absent() {
        // A Word title page: plain paragraphs before the first heading.
        let body = format!(
            "{}{}{}",
            "<w:p><w:r><w:t>Salt and Iron</w:t></w:r></w:p>",
            "<w:p><w:r><w:t>by J. Reader</w:t></w:r></w:p>",
            heading("One"),
        );
        let out = parse_document_xml(&document(&body), None);
        assert_eq!(
            out.title.as_deref(),
            Some("Salt and Iron"),
            "first preamble line"
        );
        assert_eq!(out.author.as_deref(), Some("J. Reader"), "the by-line");
        // The title page is not itself a chapter.
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "One");
    }

    #[test]
    fn docx_with_no_headings_is_one_chapter() {
        let body = "<w:p><w:r><w:t>Just prose, no headings.</w:t></w:r></w:p>";
        let out = parse_document_xml(&document(body), None);
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "Manuscript");
        assert_eq!(out.chapters[0].body, "Just prose, no headings.\n");
    }

    #[test]
    fn docx_decodes_xml_entities_and_ignores_bold_off() {
        let body = "<w:p><w:r><w:rPr><w:b w:val=\"false\"/></w:rPr>\
                    <w:t>5 &lt; 6 &amp; 7 &#62; 4</w:t></w:r></w:p>";
        let out = parse_document_xml(&document(body), None);
        // Entities decoded; w:val="false" means the run isn't actually bold.
        assert_eq!(out.chapters[0].body, "5 < 6 & 7 > 4\n");
        assert!(!out.chapters[0].body.contains('*'));
    }

    #[test]
    fn round_trips_a_quill_docx_export() {
        use crate::export::docx::{to_docx, Profile};
        use crate::export::{Chapter, Document, Part};
        let doc = Document {
            title: "Round Trip".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![
                    Chapter {
                        title: "One".into(),
                        epigraph: None,
                        body: "The night was *dark*.".into(),
                    },
                    Chapter {
                        title: "Two".into(),
                        epigraph: None,
                        body: "The **end**.".into(),
                    },
                ],
            }],
            back: vec![],
        };
        // The Vellum profile tags chapter titles as Heading 1 — what the importer splits on.
        let bytes = to_docx(&doc, Profile::Vellum).unwrap();
        let out = parse_docx(&bytes).unwrap();
        let titles: Vec<&str> = out.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["One", "Two"], "chapters round-trip");
        assert_eq!(
            out.chapters[0].body, "The night was *dark*.\n",
            "italic preserved"
        );
        assert_eq!(out.chapters[1].body, "The **end**.\n", "bold preserved");
    }

    #[test]
    fn parse_docx_rejects_a_non_docx() {
        assert!(parse_docx(b"this is not a zip").is_err());
    }

    // --- .scriv import (#402) --------------------------------------------

    /// A minimal RTF document body (one paragraph) like `export::scriv` writes.
    fn rtf(body: &str) -> String {
        format!("{{\\rtf1\\ansi\\deff0{{\\fonttbl{{\\f0\\froman Times New Roman;}}}}\n\\f0\\fs24\n\\pard\\sa180 {body}\\par\n}}")
    }

    #[test]
    fn rtf_converts_emphasis_paragraphs_and_unicode() {
        let body = "The locket {\\i gleamed} and felt {\\b heavy}. caf\\u233 ?";
        let out = rtf_to_markdown(&rtf(body));
        assert_eq!(out, "The locket *gleamed* and felt **heavy**. café\n");
    }

    #[test]
    fn rtf_skips_the_font_table_and_decodes_escapes() {
        // The fonttbl text ("Times New Roman;") must not leak into the body.
        let out = rtf_to_markdown(&rtf("a \\{brace\\} and 5 \\\\ slash"));
        assert_eq!(out, "a {brace} and 5 \\ slash\n");
        assert!(
            !out.contains("Times New Roman"),
            "font table skipped: {out}"
        );
    }

    #[test]
    fn rtf_scene_break_glyph_becomes_a_thematic_break() {
        let body = format!(
            "{{\\rtf1\\ansi\\deff0\n\\pard\\sa180 Scene one.\\par\n\\pard\\qc\\sa180 {SCENE_BREAK}\\par\n\\pard\\sa180 Scene two.\\par\n}}"
        );
        let out = rtf_to_markdown(&body);
        assert_eq!(out, "Scene one.\n\n---\n\nScene two.\n");
    }

    #[test]
    fn binder_flattens_folders_skips_research_and_trash_keeps_order() {
        let scrivx = r#"<?xml version="1.0"?><ScrivenerProject><Binder>
<BinderItem UUID="d" Type="DraftFolder"><Title>Manuscript</Title><Children>
<BinderItem UUID="u1" Type="Text"><Title>One</Title></BinderItem>
<BinderItem UUID="f1" Type="Folder"><Title>Part One</Title><Children>
<BinderItem UUID="u2" Type="Text"><Title>Two</Title></BinderItem>
</Children></BinderItem>
</Children></BinderItem>
<BinderItem UUID="r" Type="ResearchFolder"><Title>Research</Title></BinderItem>
<BinderItem UUID="t" Type="TrashFolder"><Title>Trash</Title></BinderItem>
</Binder></ScrivenerProject>"#;
        let out = parse_binder(scrivx, Some("My Novel".into()), |uuid| match uuid {
            "u1" => Some(rtf("Alpha.")),
            "u2" => Some(rtf("Beta.")),
            _ => None,
        });
        assert_eq!(out.title.as_deref(), Some("My Novel"));
        let titles: Vec<&str> = out.chapters.iter().map(|c| c.title.as_str()).collect();
        // The folder is flattened; its text doc follows the top-level one; Research
        // and Trash are skipped.
        assert_eq!(titles, vec!["One", "Two"]);
        assert_eq!(out.chapters[0].body, "Alpha.\n");
        assert_eq!(out.chapters[1].body, "Beta.\n");
    }

    #[test]
    fn round_trips_a_quill_scriv_export() {
        use crate::export::scriv::write_scriv;
        use crate::export::{Chapter, Document, Part};
        let doc = Document {
            title: "Round Trip".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![Chapter {
                title: "Prologue".into(),
                epigraph: None,
                body: "Before *it* all.".into(),
            }],
            parts: vec![Part {
                name: Some("Part One".into()),
                chapters: vec![Chapter {
                    title: "The Find".into(),
                    epigraph: None,
                    body: "The locket **gleamed**.".into(),
                }],
            }],
            back: vec![Chapter {
                title: "Epilogue".into(),
                epigraph: None,
                body: "After.".into(),
            }],
        };
        let dir =
            crate::testutil::scratch_dir("scriv-import", "round-trip").join("Round Trip.scriv");
        write_scriv(&doc, &dir).unwrap();
        let out = parse_scriv(&dir).unwrap();

        assert_eq!(
            out.title.as_deref(),
            Some("Round Trip"),
            "title from the bundle name"
        );
        // Binder reading order, folder flattened (the part title isn't a chapter).
        let titles: Vec<&str> = out.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["Prologue", "The Find", "Epilogue"]);
        assert_eq!(
            out.chapters[0].body, "Before *it* all.\n",
            "italic round-trips"
        );
        assert_eq!(
            out.chapters[1].body, "The locket **gleamed**.\n",
            "bold round-trips"
        );
    }

    #[test]
    fn parse_scriv_rejects_a_dir_without_a_binder() {
        let dir = crate::testutil::scratch_dir("scriv-import", "no-binder");
        assert!(
            parse_scriv(&dir).is_err(),
            "a directory with no .scrivx is rejected"
        );
    }
}
