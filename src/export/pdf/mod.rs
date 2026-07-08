//! PDF export (#463) and the print-ready interior profile (#464). A PDF is
//! hand-written the same way the `.docx` OOXML and `.epub` XHTML are — a header, a
//! body of numbered objects (catalog, page tree, fonts, per-page content streams,
//! and a bookmark outline), an xref table, and a trailer. We lay the book out
//! ourselves; only glyph metrics are read from the bundled fonts (see [`font`]).
//!
//! Two [`Layout`]s share that machinery: the **screen** profile (#463) is US-Letter
//! for a readable proof; the **print** profile (#464) is a chosen trim size with
//! mirrored inside/outside margins and a binding gutter, running heads, recto
//! chapter starts — what print-on-demand services require. Both embed their fonts
//! and render text via Type0 /Identity-H composites (#492; see [`font`]), so the PDF
//! shows the full Unicode range its fonts cover (Latin, Greek, Cyrillic, symbols,
//! ornaments — CJK with an author-supplied CJK font). Each face is subset to the
//! glyphs used (#495), so files stay small. Designed typography (font choice,
//! ornaments) is the styling epic; cover/bleed are a separate artifact.

mod cover;
mod draw;
mod finish;
// `font` is otherwise private to the PDF writer, but the EPUB writer reuses its
// glyph subsetter for embedded fonts (#709).
pub mod font;
mod frontmatter;
mod icc;
mod images;
mod layout;
mod meta;
mod toc;
mod wrap;
mod write;

use self::draw::mono_wrap;
use self::font::Fonts;
use self::images::{decode_images, PdfImage};
use self::layout::Layout;
use self::meta::{PageMeta, StyleBits};
use self::toc::{doc_toc_titles, toc_entries};
use self::wrap::{segments, words_of, wrap_lines, Hyphenate, Seg};
use self::write::OutlineItem;
use super::{
    body_blocks, classify_block, copyright_lines, inline_runs, Chapter, Document, ExportStyle,
    FontStyle, ImageRole, Prose, PubMeta, Run, SCENE_BREAK,
};
use crate::frontmatter::Epigraph;

pub use self::cover::{to_wrap_cover, WrapGeometry, DEFAULT_CALIPER_INCHES};
pub use self::layout::{trim_points, PrintConfig, DEFAULT_TRIM, TRIM_NAMES};

// Type sizes and vertical rhythm, shared by both profiles (in points).
pub(super) const BODY_SIZE: f32 = 12.0;
pub(super) const BODY_LEADING: f32 = 15.0;
pub(super) const INDENT: f32 = 18.0; // first-line paragraph indent (~1.5em)
const HEADING_SIZE: f32 = 17.0;
const HEADING_LEADING: f32 = 21.0;
const TITLE_SIZE: f32 = 26.0;
const PART_SIZE: f32 = 20.0;
const EPIGRAPH_SIZE: f32 = 11.0;
const EPIGRAPH_LEADING: f32 = 14.0;
const EPIGRAPH_INSET: f32 = 36.0; // left/right inset for the set-apart block
pub(super) const MONO_SIZE: f32 = 10.5; // fixed-width face for fenced blocks (#524)
const MONO_LEADING: f32 = 13.0;
const HEAD_SIZE: f32 = 10.0; // running heads + folios in the print profile

// The print-quality DPI floor (#696). KDP/IngramSpark want cover art at ≥300 DPI and
// flag under-resolution; a cover that fills the media box at fewer effective DPI
// (pixel_width ÷ media-box inches) prints soft, so we warn (never hard-fail).
const MIN_COVER_DPI: f32 = 300.0;
const PTS_PER_INCH: f32 = 72.0;

/// Warnings for `[style]` settings the PDF writers can't honor (#490/#492), to
/// surface alongside the export: SVG image ornaments can't be embedded (PDF takes
/// raster only), and glyphs no embedded font covers (e.g. CJK without a CJK font)
/// can't render — the scene break falls back to `#` and ornaments skip.
pub fn pdf_style_warnings(style: &ExportStyle) -> Vec<String> {
    let mut warnings = Vec::new();
    let svgs: Vec<&str> = [
        (ImageRole::SceneBreak, "scene break"),
        (ImageRole::ChapterHeading, "chapter heading"),
        (ImageRole::Title, "title"),
        (ImageRole::Cover, "cover"),
    ]
    .into_iter()
    .filter(|(role, _)| {
        style
            .image(*role)
            .is_some_and(|i| i.media_type == "image/svg+xml")
    })
    .map(|(_, label)| label)
    .collect();
    if !svgs.is_empty() {
        warnings.push(format!(
            "style: SVG image ornaments ({}) can't render in PDF (raster only)",
            svgs.join(", ")
        ));
    }
    let fonts = Fonts::new(style);
    let glyphs: Vec<&str> = [
        &style.scene_break,
        &style.title_ornament,
        &style.part_ornament,
    ]
    .into_iter()
    .flatten()
    .filter(|g| !g.chars().all(|c| fonts.covers(c)))
    .map(String::as_str)
    .collect();
    if !glyphs.is_empty() {
        warnings.push(format!(
            "style: glyph(s) {} aren't in the PDF font set and were skipped",
            glyphs.join(" ")
        ));
    }
    warnings
}

/// The effective DPI a `width_px`-wide image resolves to when scaled to fill a
/// `media_w_pts`-wide media box: pixels ÷ inches. `0` for a degenerate media box.
fn effective_dpi(width_px: u32, media_w_pts: f32) -> f32 {
    let inches = media_w_pts / PTS_PER_INCH;
    if inches <= 0.0 {
        return 0.0;
    }
    width_px as f32 / inches
}

/// A print-resolution warning for a cover that would fill a `media_w_pts`-wide media
/// box below [`MIN_COVER_DPI`] (#696), or `None` when it's at/above the floor. The
/// image scales to cover the whole media box, so its effective DPI is fixed by the
/// source pixel width; a low-res cover silently prints blurry, so we surface it.
fn cover_dpi_warning(width_px: u32, media_w_pts: f32) -> Option<String> {
    let dpi = effective_dpi(width_px, media_w_pts);
    (dpi < MIN_COVER_DPI).then(|| {
        format!(
            "cover: effective resolution is {dpi:.0} DPI (under {MIN_COVER_DPI:.0}); \
             print services want ≥{MIN_COVER_DPI:.0} DPI — the cover may print blurry"
        )
    })
}

/// Print-resolution warnings for the export (#696), to surface alongside the style
/// warnings: the cover image, scaled to fill the `media_w_pts`-wide media box, is
/// checked against the [`MIN_COVER_DPI`] floor. Mirrors [`pdf_style_warnings`] and
/// uses the same `media_w` `cover_page` scales to, so the reported warning matches the
/// emitted PDF. Undecodable/absent covers yield nothing (that's a style warning's job).
pub fn pdf_dpi_warnings(style: &ExportStyle, media_w_pts: f32) -> Vec<String> {
    style
        .image(ImageRole::Cover)
        .and_then(|img| image::load_from_memory(&img.bytes).ok())
        .and_then(|decoded| cover_dpi_warning(decoded.width(), media_w_pts))
        .into_iter()
        .collect()
}

/// The Letter media-box width (points) the screen profile scales a cover to fill,
/// exposed so the exporter can compute [`pdf_dpi_warnings`] for a screen PDF.
pub const SCREEN_MEDIA_W: f32 = 612.0;

/// Compile a [`Document`] to a screen PDF (#463): US-Letter, embedded Type0 fonts,
/// honoring the designed `[style]` (#489/#492).
// Returns `io::Result` for a uniform export signature (the other writers are
// fallible and callers in app::exporting drive them through one fallible path).
#[allow(clippy::unnecessary_wraps)]
pub fn to_pdf(doc: &Document, style: &ExportStyle) -> std::io::Result<Vec<u8>> {
    Ok(render_book(doc, style, Layout::screen(style)))
}

/// Compile a whole series to one screen PDF (#332/#463).
// Fallible signature kept for a uniform export path (see `to_pdf`).
#[allow(clippy::unnecessary_wraps)]
pub fn to_pdf_series(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
    style: &ExportStyle,
) -> std::io::Result<Vec<u8>> {
    Ok(render_series(
        books,
        series,
        author,
        style,
        Layout::screen(style),
    ))
}

/// Compile a [`Document`] to a print-ready interior PDF (#464) at the given trim
/// (points) with a `bleed` allowance (#652, points; `0` for none): mirrored margins +
/// gutter, running heads, recto starts, embedded fonts, honoring the designed
/// `[style]` (#489). A non-zero bleed enlarges the media box and lets the cover fill
/// past the trim edge; body margins are unchanged.
// Fallible signature kept for a uniform export path (see `to_pdf`).
#[allow(clippy::unnecessary_wraps)]
pub fn to_pdf_print(
    doc: &Document,
    trim_w: f32,
    trim_h: f32,
    bleed: f32,
    style: &ExportStyle,
) -> std::io::Result<Vec<u8>> {
    Ok(render_book(
        doc,
        style,
        Layout::print(trim_w, trim_h, bleed, style),
    ))
}

/// Compile a whole series to one print-ready interior PDF (#464), with a `bleed`
/// allowance (#652, points; `0` for none).
// Fallible signature kept for a uniform export path (see `to_pdf`).
#[allow(clippy::unnecessary_wraps)]
pub fn to_pdf_series_print(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
    trim_w: f32,
    trim_h: f32,
    bleed: f32,
    style: &ExportStyle,
) -> std::io::Result<Vec<u8>> {
    Ok(render_series(
        books,
        series,
        author,
        style,
        Layout::print(trim_w, trim_h, bleed, style),
    ))
}

/// Lay out one standalone book onto a fresh `Pdf` with `layout` and serialize it.
fn render_book(doc: &Document, style: &ExportStyle, layout: Layout) -> Vec<u8> {
    lay_out_book(doc, style, layout).finish()
}

/// The number of interior pages a standalone book lays out to at the given trim
/// (points) with a `bleed` allowance (#464/#652) — what the wrap cover's spine width is
/// derived from (#819). Lays the print interior out exactly as [`to_pdf_print`] does
/// (so the count matches the emitted interior, blank recto-start pages included) and
/// reads `pages.len()` instead of serializing. The cover generator calls this to size
/// the spine to the very book that ships beside it.
pub fn print_page_count(
    doc: &Document,
    trim_w: f32,
    trim_h: f32,
    bleed: f32,
    style: &ExportStyle,
) -> usize {
    lay_out_book(doc, style, Layout::print(trim_w, trim_h, bleed, style))
        .pages
        .len()
}

/// Lay out one standalone book onto a fresh `Pdf` with `layout`, returning the built
/// builder ahead of serialization — shared by [`render_book`] (which finishes it) and
/// [`print_page_count`] (which reads the page count off it, #819).
fn lay_out_book(doc: &Document, style: &ExportStyle, layout: Layout) -> Pdf {
    let mut pdf = Pdf::new(layout, StyleBits::from_style(style), decode_images(style));
    pdf.set_heads(&doc.title, doc.author.as_deref().unwrap_or(&doc.title));
    pdf.set_info(&doc.title, doc.author.as_deref().unwrap_or(""));
    pdf.cover_page();
    // The half-title (#820) is the leaf ahead of the full title page, when enabled.
    if doc.half_title {
        pdf.half_title_page(&doc.title);
    }
    pdf.title_page(
        doc.series.as_deref(),
        &doc.title,
        doc.subtitle.as_deref(),
        doc.author.as_deref(),
        doc.epigraph.as_ref(),
    );
    pdf.copyright_page(&doc.publishing);
    // The "also in this series" page (#820), when this is a series title's book.
    pdf.series_list_page(&doc.series_list_lines());
    // Reserve the Contents page(s) in the front matter before the body is laid out,
    // so the body lands on the right pages; fill them once the folios are known.
    let titles = doc_toc_titles(doc);
    let toc = pdf.reserve_contents(&titles);
    // Everything laid out so far is front matter (roman folios); the body starts here.
    pdf.begin_body();
    pdf.outline = book_outline(&mut pdf, doc);
    let entries = toc_entries(&pdf.outline);
    pdf.fill_contents(toc, &entries);
    pdf
}

/// Lay out a series (a series title page, then each book behind a divider) and
/// serialize it.
fn render_series(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
    style: &ExportStyle,
    layout: Layout,
) -> Vec<u8> {
    let mut pdf = Pdf::new(layout, StyleBits::from_style(style), decode_images(style));
    let title = series.unwrap_or("Series");
    pdf.set_heads(title, author.unwrap_or(title));
    pdf.set_info(title, author.unwrap_or(""));
    pdf.cover_page();
    // A half-title ahead of the omnibus title page, when the first book enables it
    // (#820) — the omnibus's own front matter, so it carries no series-list page.
    if books.first().is_some_and(|b| b.half_title) {
        pdf.half_title_page(title);
    }
    pdf.title_page(None, title, None, author, None);
    // The omnibus's front-matter copyright page is series-level — the first book's
    // (#711). Each book's own copyright is reprinted on its divider page below, so a
    // later book's rights/ISBN aren't silently dropped.
    if let Some(first) = books.first() {
        pdf.copyright_page(&first.publishing);
    }
    // Each book is a top-level Contents entry (like a part) nesting its own sections.
    let mut titles = Vec::new();
    for book in books {
        titles.push((book.title.clone(), true));
        titles.extend(doc_toc_titles(book).into_iter().map(|(t, _)| (t, false)));
    }
    let toc = pdf.reserve_contents(&titles);
    // The series/title/copyright/Contents are front matter (roman); books are the body.
    pdf.begin_body();
    let mut outline = Vec::new();
    for book in books {
        pdf.set_heads(&book.title, author.unwrap_or(&book.title));
        let page = pdf.divider_page(&book.title, PART_SIZE, None, Align::Center);
        // Reprint this book's own copyright beneath its divider (#711).
        pdf.divider_copyright(&book.publishing);
        let mut item = OutlineItem::new(book.title.clone(), page);
        item.children = book_outline(&mut pdf, book);
        outline.push(item);
    }
    pdf.outline = outline;
    let entries = toc_entries(&pdf.outline);
    pdf.fill_contents(toc, &entries);
    pdf.finish()
}

/// Lay out one book's sections (front, parts, back) and return the outline subtree:
/// front/back chapters and an unnamed part's chapters sit at this level; a named
/// part nests its chapters beneath it.
fn book_outline(pdf: &mut Pdf, doc: &Document) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    for ch in &doc.front {
        items.push(OutlineItem::new(ch.title.clone(), pdf.chapter(ch)));
    }
    for part in &doc.parts {
        match &part.name {
            Some(name) => {
                // A named part divider honors the heading alignment and the part
                // ornament glyph (#489).
                let ornament = pdf.bits.part_ornament.clone();
                let align = pdf.bits.heading_align;
                let page = pdf.divider_page(name, PART_SIZE, ornament.as_deref(), align);
                let mut item = OutlineItem::new(name.clone(), page);
                for ch in &part.chapters {
                    item.children
                        .push(OutlineItem::new(ch.title.clone(), pdf.chapter(ch)));
                }
                items.push(item);
            }
            None => {
                for ch in &part.chapters {
                    items.push(OutlineItem::new(ch.title.clone(), pdf.chapter(ch)));
                }
            }
        }
    }
    for ch in &doc.back {
        items.push(OutlineItem::new(ch.title.clone(), pdf.chapter(ch)));
    }
    items
}

/// Accumulates per-page content as the book is laid out, then [`Pdf::finish`]
/// decorates the pages (running heads, folios) and serializes the file.
struct Pdf {
    layout: Layout,
    bits: StyleBits,
    images: Vec<PdfImage>,
    pages: Vec<String>,
    meta: Vec<PageMeta>,
    y: f32,
    outline: Vec<OutlineItem>,
    head_verso: String,
    head_recto: String,
    /// The document title and author for the `/Info` dictionary (#692), sourced from
    /// the manifest. Empty when unknown; an empty field is omitted from `/Info`.
    doc_title: String,
    doc_author: String,
    /// The first body page index (#651): pages before it are front matter, numbered
    /// in lowercase roman; the body from here is Arabic starting at 1. `usize::MAX`
    /// until the body begins, so every page laid out so far counts as front matter.
    body_start: usize,
}

impl Pdf {
    /// One chapter on a fresh page: the title (honoring the heading alignment), an
    /// optional epigraph, then the body — the opening paragraph dropped-capped when
    /// styled. Returns the page it starts on (for its bookmark).
    fn chapter(&mut self, ch: &Chapter) -> usize {
        let page = self.open_section();
        self.y -= HEADING_LEADING * 2.0;
        // A chapter-heading image sits above the title (#490).
        self.place_image(
            ImageRole::ChapterHeading,
            self.layout.text_w(),
            HEADING_LEADING * 4.0,
            HEADING_LEADING,
        );
        let align = self.bits.heading_align;
        self.heading_line(&ch.title, HEADING_SIZE, HEADING_LEADING * 1.6, align);
        if let Some(e) = &ch.epigraph {
            self.epigraph(e);
            self.y -= BODY_LEADING;
        }
        let mut opened = false;
        for block in body_blocks(&ch.body) {
            match classify_block(&block) {
                None => {}
                Some(Prose::SceneBreak) => self.scene_break_line(),
                Some(Prose::Heading(h)) => self.sub_heading(&h),
                Some(Prose::Preformatted(code)) => self.preformatted(&code),
                Some(Prose::Paragraph(text)) => {
                    let runs = inline_runs(&text);
                    if !opened && self.bits.drop_cap {
                        self.dropped_paragraph(&runs);
                    } else {
                        self.paragraph(&runs);
                    }
                    opened = true;
                }
            }
        }
        page
    }

    /// A preformatted (fenced) block: each line drawn verbatim in the bundled
    /// monospace face (DejaVu Sans Mono, #524), left-aligned, its breaks and
    /// indentation preserved and no emphasis applied, set apart by space above and
    /// below (#522). Lines too wide for the text column are wrapped at character
    /// boundaries — preserving alignment, since the face is fixed-width — so nothing
    /// runs off the page edge.
    fn preformatted(&mut self, code: &str) {
        self.need(MONO_LEADING * 1.5);
        self.y -= BODY_LEADING * 0.5;
        let col = self.layout.text_w();
        for line in code.lines() {
            for piece in mono_wrap(&self.layout, line, col) {
                self.need(MONO_LEADING);
                if !piece.is_empty() {
                    let left = self.left();
                    self.draw_mono_line(left, self.y, &piece);
                }
                self.y -= MONO_LEADING;
            }
        }
        self.y -= BODY_LEADING * 0.5;
    }

    /// The scene-break divider — the configured glyph, centered. A glyph the PDF
    /// fonts can't show (a decorative ornament outside WinAnsi) falls back to the
    /// default `#` so a divider still appears (the fallback is warned about in
    /// [`pdf_style_warnings`]).
    fn scene_break_line(&mut self) {
        self.need(BODY_LEADING * 2.0);
        self.y -= BODY_LEADING * 0.5;
        if self.image(ImageRole::SceneBreak).is_some() {
            // An image divider replaces the glyph (#490).
            self.place_image(
                ImageRole::SceneBreak,
                self.layout.text_w() * 0.4,
                BODY_LEADING * 2.0,
                BODY_LEADING * 0.5,
            );
            return;
        }
        let glyph = if self.layout.renders(&self.bits.scene_break) {
            self.bits.scene_break.clone()
        } else {
            SCENE_BREAK.to_string()
        };
        self.centered(&glyph, false, false, false, BODY_SIZE, BODY_LEADING * 1.5);
    }

    /// A body sub-heading (`##` …), bold, honoring the heading alignment.
    fn sub_heading(&mut self, h: &str) {
        self.need(BODY_LEADING * 2.0);
        self.y -= BODY_LEADING * 0.5;
        let align = self.bits.heading_align;
        self.heading_line(h, BODY_SIZE, BODY_LEADING, align);
    }

    /// A bold heading line at `align` (chapter/part titles, sub-headings), drawn in
    /// the heading font (#638).
    fn heading_line(&mut self, text: &str, size: f32, leading: f32, align: Align) {
        let words = words_of(&[Run::new(text, true, false)], true);
        self.flow(&words, self.layout.text_w(), 0.0, size, leading, align);
    }

    /// The chapter's opening paragraph with a three-line drop-cap initial: the first
    /// character is set large at the left margin and the first lines are indented to
    /// clear it (#489 / mirrors the EPUB `.opening` first-letter treatment).
    fn dropped_paragraph(&mut self, runs: &[Run]) {
        const LINES: usize = 3;
        let mut words = words_of(runs, false);
        let Some(first_word) = words.first_mut() else {
            return;
        };
        let Some(initial) = first_word.text.chars().next() else {
            return;
        };
        first_word.text = first_word.text.chars().skip(1).collect();
        if first_word.text.is_empty() {
            words.remove(0);
        }
        let cap_size = BODY_LEADING * LINES as f32;
        let cap_w = self
            .layout
            .measure(&initial.to_string(), cap_size, FontStyle::REGULAR, false)
            + 2.0;
        // Reserve the cap block so the initial and its first lines stay together.
        self.need(BODY_LEADING * LINES as f32);
        let left = self.left();
        let cap_baseline = (LINES as f32 - 1.0).mul_add(-BODY_LEADING, self.y);
        self.draw_line(
            left,
            cap_baseline,
            &[Seg {
                text: initial.to_string(),
                style: FontStyle::REGULAR,
                heading: false,
            }],
            cap_size,
        );
        // The first LINES lines are inset by the cap width; the rest run full width.
        let full = self.layout.text_w();
        let line_w = |i: usize| if i < LINES { full - cap_w } else { full };
        let lines = wrap_lines(&self.layout, &words, BODY_SIZE, Hyphenate::On, line_w);
        let last = lines.len().saturating_sub(1);
        for (i, (line, _w)) in lines.into_iter().enumerate() {
            self.need(BODY_LEADING);
            let inset = if i < LINES { cap_w } else { 0.0 };
            let x = self.left() + inset;
            // Justify like a normal paragraph (#650): every line but the last, and never
            // a one-word line. The target is the line's own (possibly cap-inset) width.
            if i != last && line.len() > 1 {
                self.draw_justified_line(x, self.y, &line, BODY_SIZE, line_w(i));
            } else {
                let segs = segments(&line);
                self.draw_line(x, self.y, &segs, BODY_SIZE);
            }
            self.y -= BODY_LEADING;
        }
    }

    /// An epigraph: the lines set apart (inset, italic, single-spaced) with the
    /// attribution on its own line.
    fn epigraph(&mut self, e: &Epigraph) {
        let width = 2.0f32.mul_add(-EPIGRAPH_INSET, self.layout.text_w());
        for line in e.text.lines() {
            let words = words_of(&[Run::new(line, false, true)], false);
            self.flow(
                &words,
                width,
                0.0,
                EPIGRAPH_SIZE,
                EPIGRAPH_LEADING,
                Align::LeftInset(EPIGRAPH_INSET),
            );
        }
        if let Some(by) = &e.attribution {
            let words = words_of(&[Run::new(format!("— {by}"), false, true)], false);
            self.flow(
                &words,
                width,
                0.0,
                EPIGRAPH_SIZE,
                EPIGRAPH_LEADING,
                Align::RightInset(EPIGRAPH_INSET),
            );
        }
    }
}

/// Where a wrapped line sits in its column.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Align {
    Left,
    Center,
    /// Body text justified to both margins (#650): each line but the last stretches
    /// its inter-word spaces to fill the column; the last line is flush-left. Headings
    /// and inset/preformatted blocks never use this.
    Justify,
    /// Flush to a left edge inset by this many points (epigraph body).
    LeftInset(f32),
    /// Flush to a right edge inset by this many points (epigraph attribution).
    RightInset(f32),
}
#[cfg(test)]
mod tests;
