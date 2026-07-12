//! `.odt` (OpenDocument Text) import (#836): an `.odt` is a zip of XML, like a
//! `.docx`, so read `content.xml` and split chapters on outline-level-1 headings
//! (`text:h`), turning bold/italic spans into `**`/`*`. The book title/author come
//! from `meta.xml` (or, failing that, a title-block preamble). Mirrors [`docx`],
//! sharing the same XML scanner and chapter-assembly state machine in `mod.rs`.
//!
//! ODT differs from `.docx` in how emphasis is carried: a `<text:span>` names an
//! automatic style (`text:style-name="T1"`) whose `<style:text-properties>`
//! (`fo:font-weight="bold"`, `fo:font-style="italic"`) live in
//! `<office:automatic-styles>`. So we first index those styles, then resolve each
//! span's name against the index — the analogue of a `.docx` run's inline
//! `<w:rPr>`.
//!
//! [`docx`]: super::docx

use std::collections::HashMap;
use std::fmt::Write;
use std::io::Cursor;

use super::{
    assemble, attr_value, count_elements, decode_entities, element_text, fallback_chapter,
    finalize_titles, find_open, for_each_child, join_paragraphs, matching_close, mine_preamble,
    read_zip_entry_bounded, tag_attrs, wrap_emphasis, Dropped, Imported,
};

/// Automatic text styles indexed by name (`T1` → bold/italic), so a
/// `<text:span text:style-name="T1">` can be resolved to emphasis.
type StyleIndex = HashMap<String, TextStyle>;

/// Parse an OpenDocument `.odt` manuscript: read `content.xml` (and the `meta.xml`
/// metadata) out of the zip and split it into chapters. The bytes are read only —
/// the caller never alters the source. Errors if the zip is unreadable or has no
/// `content.xml` (i.e. it isn't an OpenDocument text document).
pub fn parse_odt(bytes: &[u8]) -> std::io::Result<Imported> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(std::io::Error::other)?;
    let content = zip_entry(&mut zip, "content.xml")?
        .ok_or_else(|| std::io::Error::other("not an OpenDocument text file (no content.xml)"))?;
    let meta = zip_entry(&mut zip, "meta.xml")?;
    Ok(parse_content_xml(&content, meta.as_deref()))
}

/// Read one entry of an `.odt` zip to a string, or `None` if it's absent. The
/// read is bounded ([`read_zip_entry_bounded`]) so a crafted or oversized archive
/// is rejected rather than allowed to OOM the process (#1098).
fn zip_entry(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> std::io::Result<Option<String>> {
    match zip.by_name(name) {
        Ok(mut f) => {
            let (compressed, declared) = (f.compressed_size(), f.size());
            let s = read_zip_entry_bounded(&mut f, compressed, declared)?;
            Ok(Some(s))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(std::io::Error::other(e)),
    }
}

/// One ODT block: whether it's an outline-level-1 heading (a chapter break) and
/// its text (with `*`/`**` emphasis for a body paragraph, plain for a heading
/// title).
struct Para {
    heading: bool,
    text: String,
}

/// Whether an automatic text style turns bold and/or italic on.
#[derive(Clone, Copy, Default)]
struct TextStyle {
    bold: bool,
    italic: bool,
}

/// The pure core of [`parse_odt`]: turn the `content.xml` (and optional `meta.xml`)
/// into the same [`Imported`] shape the Markdown importer produces. Splits on
/// outline-level-1 `text:h` headings via the shared [`assemble`]; the preamble
/// before the first heading is a title page, mined for any title/author `meta.xml`
/// didn't supply under the one shared [`mine_preamble`] policy.
fn parse_content_xml(content: &str, meta: Option<&str>) -> Imported {
    let title = meta.and_then(|m| element_text(m, "dc:title"));
    let author = meta.and_then(|m| element_text(m, "dc:creator"));

    // Index the automatic styles so a `<text:span>` can be resolved to bold/italic
    // by its style name — ODT's equivalent of a `.docx` run's inline `<w:rPr>`.
    let styles = text_styles(content);

    // Tally structural content the flat prose model can't carry (#646). Footnotes
    // and endnotes are converted to inline `[^N]` markers as blocks are read;
    // images/tables/comments are counted per-block. Tables are counted at the
    // document level (a `<table:table>` is a sibling of the paragraphs).
    let mut dropped = Dropped::default();

    // Split on outline-level-1 headings; the run before the first heading is a
    // title page. Each `Para` is pre-classified, so the shared assembler just
    // flushes.
    let (mut chapters, preamble) = assemble(
        paragraphs(content, &styles, &mut dropped),
        |para| para.heading,
        |para| para.text.clone(),
        |paras| join_paragraphs(&body_texts(paras)),
    );
    let preamble: Vec<String> = preamble.into_iter().map(|p| p.text).collect();

    // The title page fills in anything `meta.xml` didn't — the first non-empty line
    // as the title, a "by …" line as the author (#501: mined with the same
    // unconditional policy the Markdown path uses).
    let has_chapters = !chapters.is_empty();
    let (title, author) = mine_preamble(&preamble, title, author, has_chapters, |p| {
        p.iter()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .map(str::to_string)
    });

    // A heading-less document keeps its whole text as the one chapter's body.
    fallback_chapter(&mut chapters, &title, join_paragraphs(&preamble));
    finalize_titles(&mut chapters);

    Imported {
        title,
        author,
        chapters,
        // An `.odt` carries every body inside the single `content.xml` container;
        // once that's read there's no per-body read to fail, so the analogous
        // unreadable-body count is always 0 (see [`Imported`]).
        unreadable_bodies: 0,
        dropped,
    }
}

/// The texts of a run of body paragraphs, for [`join_paragraphs`].
fn body_texts(paras: &[Para]) -> Vec<String> {
    paras.iter().map(|p| p.text.clone()).collect()
}

/// Index the automatic text styles by name: a `<style:style style:name="T1"
/// style:family="text">` carries a `<style:text-properties>` whose
/// `fo:font-weight="bold"` / `fo:font-style="italic"` say whether spans naming it
/// are bold/italic. ODT's equivalent of a `.docx` run's inline `<w:rPr>`, hoisted
/// out into `<office:automatic-styles>`.
fn text_styles(content: &str) -> StyleIndex {
    let mut styles = StyleIndex::new();
    for_each_child(content, "style:style", |attrs, inner| {
        // Only text-family styles carry span emphasis; skip paragraph/table styles.
        if attr_value(attrs, "style:family").as_deref() != Some("text") {
            return;
        }
        let (Some(name), Some(inner)) = (attr_value(attrs, "style:name"), inner) else {
            return;
        };
        styles.insert(
            name,
            TextStyle {
                bold: style_on(inner, "fo:font-weight", &["bold"]),
                italic: style_on(inner, "fo:font-style", &["italic", "oblique"]),
            },
        );
    });
    styles
}

/// Whether the style's `<style:text-properties>` sets `attr` to one of `on`
/// (e.g. `fo:font-weight="bold"`), turning that emphasis on. Any other value
/// (`normal`) leaves it off.
fn style_on(inner: &str, attr: &str, on: &[&str]) -> bool {
    tag_attrs(inner, "style:text-properties")
        .and_then(|a| attr_value(a, attr))
        .is_some_and(|v| on.contains(&v.as_str()))
}

/// Walk the `<text:h>` and `<text:p>` blocks of `content.xml` in document order,
/// tallying dropped structural content into `dropped` and converting
/// footnote/endnote references to inline `[^N]` markers as it goes. A `text:h` at
/// outline level 1 is a chapter break; everything else is a body paragraph.
fn paragraphs(content: &str, styles: &StyleIndex, dropped: &mut Dropped) -> Vec<Para> {
    // Scope the walk to `<office:text>` so styles/metadata elsewhere in
    // `content.xml` aren't read as prose; a minimal fixture may omit the wrapper,
    // in which case scan the whole document. Tables live at this level, so count
    // them here (their cell paragraphs still leak through, as in `.docx`).
    let body = block_inner(content, "office:text").unwrap_or(content);
    dropped.tables += count_elements(body, "table:table");
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(block) = next_block(body, pos) {
        out.push(parse_block(&block, styles, dropped));
        pos = block.next;
    }
    out
}

/// A located `<text:h>` / `<text:p>` block: its start offset (for ordering), tag
/// name, attributes, inner XML (`None` if self-closing), and the offset just past
/// its close.
struct Block<'a> {
    start: usize,
    heading: bool,
    attrs: &'a str,
    inner: Option<&'a str>,
    next: usize,
}

/// The next `<text:h>` or `<text:p>` block at or after `from`, whichever starts
/// first — so headings and their following paragraphs stay in reading order.
fn next_block<'a>(body: &'a str, from: usize) -> Option<Block<'a>> {
    let h = block_at(body, from, "text:h", true);
    let p = block_at(body, from, "text:p", false);
    match (h, p) {
        (Some(h), Some(p)) => Some(if h.start <= p.start { h } else { p }),
        (h, p) => h.or(p),
    }
}

/// The first `<name …>…</name>` / `<name …/>` element at or after `from`. A
/// same-named element nested inside is folded into the outer element's inner XML.
fn block_at<'a>(xml: &'a str, from: usize, name: &str, heading: bool) -> Option<Block<'a>> {
    let (attrs_start, gt, self_closing) = find_open(xml, from, name)?;
    let start = xml[..attrs_start].rfind('<')?;
    let attrs = xml[attrs_start..gt].trim_end_matches('/').trim();
    if self_closing {
        return Some(Block {
            start,
            heading,
            attrs,
            inner: None,
            next: gt + 1,
        });
    }
    let at = matching_close(xml, name, gt + 1)?;
    Some(Block {
        start,
        heading,
        attrs,
        inner: Some(&xml[gt + 1..at]),
        next: at + name.len() + 3, // past "</name>"
    })
}

/// The inner XML of the first `<name>…</name>` block element, or `None` if it's
/// absent or self-closing. Used to scope the paragraph walk to `<office:text>`.
fn block_inner<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let (_, gt, self_closing) = find_open(xml, 0, name)?;
    if self_closing {
        return None;
    }
    let at = matching_close(xml, name, gt + 1)?;
    Some(&xml[gt + 1..at])
}

/// One block → a [`Para`]. An outline-level-1 heading is a chapter title (plain
/// text); anything else is a body paragraph with bold/italic spans converted to
/// `**`/`*` and footnote/endnote references to inline `[^N]` markers.
fn parse_block(block: &Block<'_>, styles: &StyleIndex, dropped: &mut Dropped) -> Para {
    let is_chapter = block.heading && outline_level(block.attrs) == Some(1);
    let Some(inner) = block.inner else {
        return Para {
            heading: is_chapter,
            text: String::new(),
        };
    };
    if is_chapter {
        // A title carries no Markdown; flatten spans to plain text.
        let mut text = String::new();
        inline_text(inner, &mut text);
        return Para {
            heading: true,
            text,
        };
    }
    Para {
        heading: false,
        text: block_markdown(inner, styles, dropped),
    }
}

/// A heading's `text:outline-level`, if present. Level 1 is a chapter break (the
/// level Quill's own ODT-style export writes chapter titles at); deeper levels
/// fold into the current chapter's body like any paragraph.
fn outline_level(attrs: &str) -> Option<u32> {
    attr_value(attrs, "text:outline-level").and_then(|v| v.parse().ok())
}

/// A body block's inline content → Markdown: its text with each `<text:span>`
/// wrapped per its named style's bold/italic, footnote/endnote references appended
/// as inline `[^N]` / `[^eN]` markers, and images/comments tallied in `dropped`.
fn block_markdown(inner: &str, styles: &StyleIndex, dropped: &mut Dropped) -> String {
    let mut out = String::new();
    inline_markdown(inner, styles, TextStyle::default(), &mut out);
    // A footnote/endnote is a `<text:note>` carried inline; convert it to a marker
    // rather than letting its body leak into the prose or vanish.
    for _ in 0..count_notes(inner, "footnote") {
        dropped.footnotes += 1;
        let _ = write!(out, "[^{}]", dropped.footnotes);
    }
    for _ in 0..count_notes(inner, "endnote") {
        dropped.endnotes += 1;
        let _ = write!(out, "[^e{}]", dropped.endnotes);
    }
    // Images (`<draw:frame>`/`<draw:image>`) and comments (`<office:annotation>`)
    // have no flat-prose equivalent; count the drop so the caller can warn.
    dropped.images += count_elements(inner, "draw:frame");
    dropped.comments += count_elements(inner, "office:annotation");
    out
}

/// How many `<text:note text:note-class="footnote|endnote">` notes appear
/// (recursively) in `inner`. The class distinguishes footnotes from endnotes on
/// the shared element.
fn count_notes(inner: &str, class: &str) -> usize {
    let mut count = 0;
    for_each_child(inner, "text:note", |attrs, _| {
        if attr_value(attrs, "text:note-class").as_deref() == Some(class) {
            count += 1;
        }
    });
    count
}

/// Append a block's inline content to `out` as Markdown, resolving emphasis.
///
/// ODT inline content interleaves raw text with elements: `<text:span>` (emphasis
/// via a named style, nestable), `<text:s>`/`<text:tab>`/`<text:line-break>`
/// (whitespace), and `<text:note>` (a footnote/endnote — its body is *not* prose,
/// so it's skipped here and surfaced as a marker by [`block_markdown`]). We scan
/// left to right, emitting text runs between elements and recursing into spans
/// with the parent style's emphasis inherited (so a bold span inside an italic one
/// is `***`).
fn inline_markdown(inner: &str, styles: &StyleIndex, active: TextStyle, out: &mut String) {
    let mut pos = 0;
    while pos < inner.len() {
        let Some(lt) = inner[pos..].find('<').map(|r| pos + r) else {
            emit_text(&inner[pos..], active, out);
            return;
        };
        emit_text(&inner[pos..lt], active, out);
        let Some(elem) = next_inline(inner, lt) else {
            // A stray `<` with no recognizable tag; emit it literally and move on.
            emit_text("<", active, out);
            pos = lt + 1;
            continue;
        };
        match elem.kind {
            Inline::Span => {
                let style = span_style(elem.attrs, styles, active);
                if let Some(body) = elem.inner {
                    inline_markdown(body, styles, style, out);
                }
            }
            Inline::Space => out.push_str(&" ".repeat(space_count(elem.attrs))),
            Inline::Tab => out.push('\t'),
            Inline::LineBreak => out.push('\n'),
            // A note's body is surfaced as a `[^N]` marker, not inlined here.
            Inline::Note | Inline::Skip => {}
        }
        pos = elem.next;
    }
}

/// Append a block's inline content to `out` as plain text (emphasis flattened) —
/// for a heading title, which carries no Markdown.
fn inline_text(inner: &str, out: &mut String) {
    inline_markdown(inner, &StyleIndex::new(), TextStyle::default(), out);
}

/// Emit a raw text run — entity-decoded, wrapped in the active emphasis markers.
fn emit_text(raw: &str, active: TextStyle, out: &mut String) {
    let decoded = decode_entities(raw);
    if decoded.is_empty() {
        return;
    }
    out.push_str(&wrap_emphasis(&decoded, active.bold, active.italic));
}

/// The emphasis of a `<text:span text:style-name="T1">`, its named style merged
/// over the inherited `active` emphasis (so a bold span inside an italic one is
/// both). A span with no/unknown style inherits unchanged.
fn span_style(attrs: &str, styles: &StyleIndex, active: TextStyle) -> TextStyle {
    let named = attr_value(attrs, "text:style-name")
        .and_then(|n| styles.get(&n).copied())
        .unwrap_or_default();
    TextStyle {
        bold: active.bold || named.bold,
        italic: active.italic || named.italic,
    }
}

/// The kinds of ODT inline element the walker distinguishes.
enum Inline {
    /// `<text:span>` — emphasis via a named style.
    Span,
    /// `<text:s>` — one or more literal spaces.
    Space,
    /// `<text:tab>` — a tab.
    Tab,
    /// `<text:line-break>` — a line break.
    LineBreak,
    /// `<text:note>` — a footnote/endnote (body skipped; surfaced as a marker).
    Note,
    /// Any other element (`<draw:frame>`, `<office:annotation>`, …) — skipped for
    /// prose; counted as dropped separately.
    Skip,
}

/// A located inline element: its kind, attributes, inner XML, and the offset just
/// past its close.
struct InlineElem<'a> {
    kind: Inline,
    attrs: &'a str,
    inner: Option<&'a str>,
    next: usize,
}

/// Parse the element opening at byte `lt` (the `<`) into an [`InlineElem`]. `None`
/// if `lt` isn't the start of a recognizable tag (a stray `<`).
fn next_inline(xml: &str, lt: usize) -> Option<InlineElem<'_>> {
    // The tag name runs from just past `<` to the first whitespace, `>`, or `/`.
    let after_lt = lt + 1;
    let name_end = xml[after_lt..]
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .map(|r| after_lt + r)?;
    let name = &xml[after_lt..name_end];
    let kind = match name {
        "text:span" => Inline::Span,
        "text:s" => Inline::Space,
        "text:tab" => Inline::Tab,
        "text:line-break" => Inline::LineBreak,
        "text:note" => Inline::Note,
        _ => Inline::Skip,
    };
    let (attrs_start, gt, self_closing) = find_open(xml, lt, name)?;
    let attrs = xml[attrs_start..gt].trim_end_matches('/').trim();
    if self_closing {
        return Some(InlineElem {
            kind,
            attrs,
            inner: None,
            next: gt + 1,
        });
    }
    let at = matching_close(xml, name, gt + 1)?;
    Some(InlineElem {
        kind,
        attrs,
        inner: Some(&xml[gt + 1..at]),
        next: at + name.len() + 3, // past "</name>"
    })
}

/// The number of spaces a `<text:s text:c="N"/>` stands for (default 1).
fn space_count(attrs: &str) -> usize {
    attr_value(attrs, "text:c")
        .and_then(|c| c.parse().ok())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::io::Write as _;

    /// Build an `.odt` zip in memory whose `content.xml` is `body` bytes,
    /// deflated — so a highly repetitive payload lands with a bomb-like ratio.
    fn odt_zip(body: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("content.xml", opts).unwrap();
            zip.write_all(body).unwrap();
            zip.finish().unwrap();
        }
        buf
    }

    /// Wrap block XML in a minimal `content.xml` shell with an `<office:text>`
    /// body; `styles` is the `<office:automatic-styles>` block (may be empty).
    fn content(styles: &str, body: &str) -> String {
        format!(
            "<office:document-content>\
             <office:automatic-styles>{styles}</office:automatic-styles>\
             <office:body><office:text>{body}</office:text></office:body>\
             </office:document-content>"
        )
    }

    /// An outline-level-1 heading (a chapter title).
    fn heading(title: &str) -> String {
        format!(
            "<text:h text:style-name=\"Heading_20_1\" text:outline-level=\"1\">{title}</text:h>"
        )
    }

    /// A `<style:style>` for a text span with the given emphasis attributes.
    fn text_style(name: &str, props: &str) -> String {
        format!(
            "<style:style style:name=\"{name}\" style:family=\"text\">\
             <style:text-properties {props}/></style:style>"
        )
    }

    #[test]
    fn odt_splits_on_outline_level_one_and_converts_emphasis() {
        // Italic + bold spans naming styles from the automatic-styles block.
        let styles = format!(
            "{}{}",
            text_style("T1", "fo:font-style=\"italic\""),
            text_style("T2", "fo:font-weight=\"bold\""),
        );
        let body = format!(
            "{}{}{}",
            heading("One"),
            "<text:p>The sea was <text:span text:style-name=\"T1\">grey</text:span> \
             and it <text:span text:style-name=\"T2\">ended</text:span>.</text:p>",
            heading("Two"),
        );
        let out = parse_content_xml(&content(&styles, &body), None);
        assert_eq!(out.chapters.len(), 2);
        assert_eq!(out.chapters[0].title, "One");
        assert_eq!(
            out.chapters[0].body,
            "The sea was *grey* and it **ended**.\n"
        );
        assert_eq!(out.chapters[1].title, "Two");
    }

    /// A bold span nested inside an italic one inherits both, so its text is
    /// `***bold-italic***`. Emphasis is wrapped per text run (as in `.docx`), so
    /// the surrounding italic runs each get their own `*…*` rather than one span
    /// wrapping the whole — each word still renders with the right emphasis.
    #[test]
    fn odt_nested_bold_in_italic_is_bold_italic() {
        let styles = format!(
            "{}{}",
            text_style("Ital", "fo:font-style=\"italic\""),
            text_style("Bold", "fo:font-weight=\"bold\""),
        );
        let body = format!(
            "{}{}",
            heading("One"),
            "<text:p><text:span text:style-name=\"Ital\">a \
             <text:span text:style-name=\"Bold\">b</text:span> c</text:span></text:p>",
        );
        let out = parse_content_xml(&content(&styles, &body), None);
        // Italic "a " / bold-italic "b" / italic " c", each run wrapped tightly.
        assert_eq!(out.chapters[0].body, "*a* ***b*** *c*\n");
    }

    #[test]
    fn odt_reads_title_and_author_from_meta() {
        let meta = "<office:document-meta><office:meta>\
                    <dc:title>Salt and Iron</dc:title>\
                    <dc:creator>J. Reader</dc:creator>\
                    </office:meta></office:document-meta>";
        let out = parse_content_xml(&content("", &heading("One")), Some(meta));
        assert_eq!(out.title.as_deref(), Some("Salt and Iron"));
        assert_eq!(out.author.as_deref(), Some("J. Reader"));
    }

    #[test]
    fn odt_mines_a_title_block_when_meta_is_absent() {
        let body = format!(
            "{}{}{}",
            "<text:p>Salt and Iron</text:p>",
            "<text:p>by J. Reader</text:p>",
            heading("One"),
        );
        let out = parse_content_xml(&content("", &body), None);
        assert_eq!(
            out.title.as_deref(),
            Some("Salt and Iron"),
            "first preamble line"
        );
        assert_eq!(out.author.as_deref(), Some("J. Reader"), "the by-line");
        assert_eq!(out.chapters.len(), 1, "the title page is not a chapter");
        assert_eq!(out.chapters[0].title, "One");
    }

    #[test]
    fn odt_with_no_headings_is_one_chapter() {
        let body = "<text:p>Just prose, no headings.</text:p>";
        let out = parse_content_xml(&content("", body), None);
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "Manuscript");
        assert_eq!(out.chapters[0].body, "Just prose, no headings.\n");
    }

    #[test]
    fn odt_deeper_headings_fold_into_the_chapter_body() {
        // Only outline level 1 splits; a level-2 heading is body text.
        let body = format!(
            "{}{}{}",
            heading("One"),
            "<text:h text:outline-level=\"2\">A Section</text:h>",
            "<text:p>Prose.</text:p>",
        );
        let out = parse_content_xml(&content("", &body), None);
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "One");
        assert_eq!(out.chapters[0].body, "A Section\n\nProse.\n");
    }

    #[test]
    fn odt_decodes_entities_and_expands_whitespace() {
        let body = format!(
            "{}{}",
            heading("One"),
            "<text:p>5 &lt; 6<text:s text:c=\"2\"/>&amp;<text:tab/>7</text:p>",
        );
        let out = parse_content_xml(&content("", &body), None);
        assert_eq!(out.chapters[0].body, "5 < 6  &\t7\n");
    }

    #[test]
    fn odt_ignores_a_style_that_turns_emphasis_off() {
        // A style explicitly `normal` is not emphasis.
        let styles = text_style("T1", "fo:font-weight=\"normal\" fo:font-style=\"normal\"");
        let body = format!(
            "{}{}",
            heading("One"),
            "<text:p><text:span text:style-name=\"T1\">plain</text:span></text:p>",
        );
        let out = parse_content_xml(&content(&styles, &body), None);
        assert_eq!(out.chapters[0].body, "plain\n");
        assert!(!out.chapters[0].body.contains('*'));
    }

    #[test]
    fn odt_clean_document_reports_nothing_dropped() {
        let body = format!("{}{}", heading("One"), heading("Two"));
        let out = parse_content_xml(&content("", &body), None);
        assert_eq!(
            out.dropped,
            Dropped::default(),
            "a clean document drops nothing"
        );
    }

    #[test]
    fn odt_footnotes_and_endnotes_become_inline_markers() {
        let body = format!(
            "{}{}",
            heading("One"),
            "<text:p>First claim<text:note text:note-class=\"footnote\">\
             <text:note-body><text:p>the note</text:p></text:note-body></text:note> \
             and a point<text:note text:note-class=\"endnote\">\
             <text:note-body><text:p>end</text:p></text:note-body></text:note>.</text:p>",
        );
        let out = parse_content_xml(&content("", &body), None);
        // Note bodies don't leak into the prose; markers are appended instead.
        assert_eq!(out.chapters[0].body, "First claim and a point.[^1][^e1]\n");
        assert_eq!(out.dropped.footnotes, 1);
        assert_eq!(out.dropped.endnotes, 1);
        assert_eq!(out.dropped.notes(), 2);
        assert_eq!(out.dropped.lost(), 0, "notes are converted, not lost");
    }

    #[test]
    fn odt_images_tables_and_comments_are_counted_as_dropped() {
        let body = format!(
            "{}{}{}{}",
            heading("One"),
            "<text:p>See figure\
             <draw:frame><draw:image xlink:href=\"pic.png\"/></draw:frame>\
             <office:annotation><dc:creator>R</dc:creator></office:annotation></text:p>",
            "<table:table><table:table-row><table:table-cell>\
             <text:p>cell</text:p></table:table-cell></table:table-row></table:table>",
            "<text:p><draw:frame><draw:image xlink:href=\"two.png\"/></draw:frame></text:p>",
        );
        let out = parse_content_xml(&content("", &body), None);
        assert_eq!(out.dropped.images, 2, "two draw:frame images");
        assert_eq!(out.dropped.tables, 1);
        assert_eq!(out.dropped.comments, 1);
        assert_eq!(out.dropped.footnotes, 0);
        assert_eq!(out.dropped.lost(), 4, "2 images + 1 table + 1 comment");
    }

    #[test]
    fn parse_odt_rejects_a_non_odt() {
        assert!(parse_odt(b"this is not a zip").is_err());
    }

    #[test]
    fn parse_odt_rejects_a_zip_bomb_entry() {
        // A `content.xml` of many megabytes of one repeated byte deflates
        // thousandsfold — a decompression bomb. The bounded read must refuse it
        // with an `InvalidData` error rather than expand it into memory (#1098).
        let bomb = vec![b'A'; 64 * 1024 * 1024];
        let bytes = odt_zip(&bomb);
        assert!(bytes.len() < 1024 * 1024, "the bomb compresses small");
        let err = parse_odt(&bytes).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn parse_odt_accepts_a_normal_document_after_the_bound() {
        // A valid, ordinary `.odt` well under the cap still parses — the guard
        // rejects bombs, not honest documents.
        let bytes = odt_zip(content("", &heading("One")).as_bytes());
        let out = parse_odt(&bytes).unwrap();
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "One");
    }

    proptest! {
        /// Feeding arbitrary bytes to `parse_odt` never panics: a malformed or
        /// truncated archive surfaces an `Err`, never a crash (#1098). Import
        /// runs on untrusted input, so this is the no-panic bar on that surface.
        #[test]
        fn parse_odt_never_panics_on_arbitrary_bytes(
            bytes in prop::collection::vec(any::<u8>(), 0..4096)
        ) {
            let _ = parse_odt(&bytes);
        }

        /// A well-formed `.odt` zip carrying an arbitrary (possibly truncated or
        /// non-UTF-8) `content.xml` body never panics — the zip layer is valid,
        /// only its payload is hostile.
        #[test]
        fn parse_odt_never_panics_on_arbitrary_entry_body(
            body in prop::collection::vec(any::<u8>(), 0..4096)
        ) {
            let bytes = odt_zip(&body);
            let _ = parse_odt(&bytes);
        }
    }

    /// Build a real `.odt` zip in memory (as the `.docx` tests build a `.docx`) and
    /// parse it end to end: headings split into chapters and emphasis maps to
    /// Markdown — proving the zip reader and `content.xml`/`meta.xml` wiring, not
    /// just the pure core.
    #[test]
    fn parse_odt_reads_a_real_zip() {
        use std::io::Write as _;
        use zip::write::SimpleFileOptions;

        let styles = text_style("T1", "fo:font-style=\"italic\"");
        let body = format!(
            "{}{}",
            heading("One"),
            "<text:p>The night was <text:span text:style-name=\"T1\">dark</text:span>.</text:p>",
        );
        let content_xml = content(&styles, &body);
        let meta_xml = "<office:document-meta><office:meta>\
                        <dc:title>Night</dc:title><dc:creator>A. Writer</dc:creator>\
                        </office:meta></office:document-meta>";

        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts = SimpleFileOptions::default();
            // The `mimetype` entry an `.odt` carries first; not required by the
            // reader, but written so the fixture is a faithful `.odt`.
            zip.start_file("mimetype", opts).unwrap();
            zip.write_all(b"application/vnd.oasis.opendocument.text")
                .unwrap();
            zip.start_file("content.xml", opts).unwrap();
            zip.write_all(content_xml.as_bytes()).unwrap();
            zip.start_file("meta.xml", opts).unwrap();
            zip.write_all(meta_xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }

        let out = parse_odt(&buf).unwrap();
        assert_eq!(out.title.as_deref(), Some("Night"));
        assert_eq!(out.author.as_deref(), Some("A. Writer"));
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "One");
        assert_eq!(out.chapters[0].body, "The night was *dark*.\n");
    }
}
