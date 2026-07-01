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

mod draw;
mod font;
mod images;
mod layout;
mod wrap;
mod write;

use std::collections::BTreeMap;

use self::draw::{mono_wrap, num};
use self::font::{Fonts, GidMap, ObjMap};
use self::images::{decode_images, emit_images, PdfImage};
use self::layout::Layout;
use self::wrap::{segments, words_of, wrap_lines, Seg};
use self::write::{flate_stream, flatten_outline, remap_glyph_ids, serialize, OutlineItem};
use super::{
    body_blocks, classify_block, copyright_lines, inline_runs, Chapter, Document, ExportStyle,
    FontStyle, ImageRole, Prose, PubMeta, Run, SCENE_BREAK,
};
use crate::frontmatter::Epigraph;

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

// The printed table-of-contents page (#603): its heading, list rhythm, and the gutter
// reserved at the right of each line for the right-aligned folio. The gutter is sized
// for a four-digit folio so a title wraps to the same width whether the reservation
// pass or the fill pass measures it — the actual (shorter) folio is right-aligned in it.
const CONTENTS_TITLE: &str = "Contents";
const TOC_LEADING: f32 = BODY_LEADING * 1.4;
const TOC_FOLIO_GUTTER: f32 = 32.0;

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

/// Compile a [`Document`] to a screen PDF (#463): US-Letter, embedded Type0 fonts,
/// honoring the designed `[style]` (#489/#492).
pub fn to_pdf(doc: &Document, style: &ExportStyle) -> std::io::Result<Vec<u8>> {
    Ok(render_book(doc, style, Layout::screen(style)))
}

/// Compile a whole series to one screen PDF (#332/#463).
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
    let mut pdf = Pdf::new(layout, StyleBits::from_style(style), decode_images(style));
    pdf.set_heads(&doc.title, doc.author.as_deref().unwrap_or(&doc.title));
    pdf.cover_page();
    pdf.title_page(
        doc.series.as_deref(),
        &doc.title,
        doc.subtitle.as_deref(),
        doc.author.as_deref(),
        doc.epigraph.as_ref(),
    );
    pdf.copyright_page(&doc.publishing);
    // Reserve the Contents page(s) in the front matter before the body is laid out,
    // so the body lands on the right pages; fill them once the folios are known.
    let titles = doc_toc_titles(doc);
    let toc = pdf.reserve_contents(&titles);
    // Everything laid out so far is front matter (roman folios); the body starts here.
    pdf.begin_body();
    pdf.outline = book_outline(&mut pdf, doc);
    let entries = toc_entries(&pdf.outline);
    pdf.fill_contents(toc, &entries);
    pdf.finish()
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
    pdf.cover_page();
    pdf.title_page(None, title, None, author, None);
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

/// The Contents titles a book contributes, in reading order, each flagged as a part
/// heading (named part) or a plain entry (front/back/chapter). Used to size the
/// reserved Contents page(s) before the body — and the folios — are laid out; the
/// order matches [`book_outline`], so [`toc_entries`] pairs titles to folios.
fn doc_toc_titles(doc: &Document) -> Vec<(String, bool)> {
    let mut titles: Vec<(String, bool)> =
        doc.front.iter().map(|c| (c.title.clone(), false)).collect();
    for part in &doc.parts {
        if let Some(name) = &part.name {
            titles.push((name.clone(), true));
        }
        titles.extend(part.chapters.iter().map(|c| (c.title.clone(), false)));
    }
    titles.extend(doc.back.iter().map(|c| (c.title.clone(), false)));
    titles
}

/// One planned Contents line: where it lands (TOC-relative page + baseline), the
/// wrapped title segments to draw, and — on the first line of an entry — which entry's
/// folio to right-align beside it. Positions depend only on the titles, so the reserve
/// and fill passes plan identically (see [`Pdf::plan_contents`]).
struct TocLine {
    page: usize,
    y: f32,
    runs: Vec<Seg>,
    size: f32,
    folio_entry: Option<usize>,
}

/// The reserved Contents section: the page index it starts at, how many pages it spans,
/// and the planned lines (folios filled in later).
struct TocPlan {
    start: usize,
    lines: Vec<TocLine>,
}

/// Borrow drawable segments as `glyph_block`'s `(text, style, heading)` tuples.
fn seg_refs(segs: &[Seg]) -> impl Iterator<Item = (&str, FontStyle, bool)> {
    segs.iter().map(|s| (s.text.as_str(), s.style, s.heading))
}

/// Flatten the built outline into per-entry folios, depth-first so a part precedes the
/// chapters nested beneath it — the printed reading order, matching [`doc_toc_titles`].
/// A part heading groups its chapters and carries no folio (`None`); an entry's folio
/// is its starting page index (the printed label is index + 1).
fn toc_entries(items: &[OutlineItem]) -> Vec<Option<usize>> {
    let mut out = Vec::new();
    for item in items {
        let part = !item.children.is_empty();
        out.push(if part { None } else { Some(item.page) });
        out.extend(toc_entries(&item.children));
    }
    out
}

/// What a page is, for decoration: a section opener and blank pages get no running
/// head; blank pages (inserted for recto starts) get no folio either.
struct PageMeta {
    verso: String,
    recto: String,
    opener: bool,
    blank: bool,
}

/// The lowercase roman numeral for `n` (1 → "i", 4 → "iv", 2026 → "mmxxvi"). Used to
/// number front-matter folios by convention (#651); a zero yields the empty string.
fn roman(mut n: usize) -> String {
    const UNITS: [(usize, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut s = String::new();
    for (value, glyph) in UNITS {
        while n >= value {
            s.push_str(glyph);
            n -= value;
        }
    }
    s
}

/// The non-font `[style]` settings the layout consults (#489): the scene-break
/// glyph, heading alignment, drop cap, and ornament glyphs. Fonts live in the
/// `Layout`'s [`FontPlan`]; a default `StyleBits` reproduces the plain output.
struct StyleBits {
    scene_break: String,
    heading_align: Align,
    drop_cap: bool,
    title_ornament: Option<String>,
    part_ornament: Option<String>,
}

impl StyleBits {
    fn from_style(style: &ExportStyle) -> Self {
        StyleBits {
            scene_break: style.scene_break().to_string(),
            // Headings aren't justified; map justify/left to flush-left.
            heading_align: match style.heading_align() {
                Some("left") | Some("justify") => Align::Left,
                Some("right") => Align::RightInset(0.0),
                _ => Align::Center,
            },
            drop_cap: style.drop_cap(),
            title_ornament: style.title_ornament.clone(),
            part_ornament: style.part_ornament.clone(),
        }
    }
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
    /// The first body page index (#651): pages before it are front matter, numbered
    /// in lowercase roman; the body from here is Arabic starting at 1. `usize::MAX`
    /// until the body begins, so every page laid out so far counts as front matter.
    body_start: usize,
}

impl Pdf {
    fn new(layout: Layout, bits: StyleBits, images: Vec<PdfImage>) -> Self {
        Pdf {
            layout,
            bits,
            images,
            pages: Vec::new(),
            meta: Vec::new(),
            y: 0.0,
            outline: Vec::new(),
            head_verso: String::new(),
            head_recto: String::new(),
            body_start: usize::MAX,
        }
    }

    /// Mark the body as beginning at the next page to be started (#651): everything
    /// laid out so far (cover/title/copyright/Contents) is front matter, numbered in
    /// roman; from here folios run Arabic from 1. Called once, before the body.
    fn begin_body(&mut self) {
        self.body_start = self.pages.len();
    }

    /// The printed folio label for page `i` under the roman-front/Arabic-body scheme
    /// (#651), or `None` when the page carries no folio (blank/cover pages, and the
    /// unnumbered opening title page). Front-matter pages are counted in roman from the
    /// first numbered front page; body pages in Arabic from 1. Both count only
    /// non-blank pages, so inserted blank versos don't consume a folio.
    fn folio_label(&self, i: usize) -> Option<String> {
        if self.meta[i].blank {
            return None;
        }
        if i >= self.body_start {
            // Arabic, counting non-blank pages since the body began.
            let n = (self.body_start..=i)
                .filter(|&j| !self.meta[j].blank)
                .count();
            return Some(n.to_string());
        }
        // Roman, counting non-blank front-matter pages. The very first numbered front
        // page is the title page, which by convention shows no folio though it counts.
        let n = (0..=i).filter(|&j| !self.meta[j].blank).count();
        if n <= 1 {
            return None;
        }
        Some(roman(n))
    }

    /// The decoded image ornament for `role`, if one was configured and decodable.
    fn image(&self, role: ImageRole) -> Option<&PdfImage> {
        self.images.iter().find(|i| i.role == role)
    }

    /// Place an image ornament for `role` centered in the text column, scaled to fit
    /// `max_w`×`max_h` (aspect preserved), and advance past it. No-op if absent.
    fn place_image(&mut self, role: ImageRole, max_w: f32, max_h: f32, gap_after: f32) {
        let Some((name, w, h)) = self
            .image(role)
            .map(|i| (i.name.clone(), i.width as f32, i.height as f32))
        else {
            return;
        };
        let scale = (max_w / w).min(max_h / h);
        let (dw, dh) = (w * scale, h * scale);
        self.need(dh);
        let x = self.left() + (self.layout.text_w() - dw) / 2.0;
        let bottom = self.y - dh;
        self.cur().push_str(&format!(
            "q {} 0 0 {} {} {} cm /{name} Do Q\n",
            num(dw),
            num(dh),
            num(x),
            num(bottom)
        ));
        self.y -= dh + gap_after;
    }

    fn cur(&mut self) -> &mut String {
        self.pages.last_mut().expect("a page was started")
    }

    fn page_idx(&self) -> usize {
        self.pages.len() - 1
    }

    fn left(&self) -> f32 {
        self.layout.left(self.page_idx())
    }

    /// Set the running-head text used from here on (verso/recto). Recorded per page.
    fn set_heads(&mut self, verso: &str, recto: &str) {
        self.head_verso = verso.to_string();
        self.head_recto = recto.to_string();
    }

    /// Begin a fresh page; the baseline returns to the top of the text area.
    fn start_page(&mut self) {
        self.pages.push(String::new());
        self.meta.push(PageMeta {
            verso: self.head_verso.clone(),
            recto: self.head_recto.clone(),
            opener: false,
            blank: false,
        });
        self.y = self.layout.text_top();
    }

    /// Start a section's first page, returning its index. In the print profile this
    /// lands on a recto, inserting a blank verso when the next page would be a verso.
    fn open_section(&mut self) -> usize {
        if self.layout.recto_starts && self.pages.len() % 2 == 1 {
            self.start_page();
            self.meta.last_mut().unwrap().blank = true;
        }
        self.start_page();
        self.meta.last_mut().unwrap().opener = true;
        self.page_idx()
    }

    /// Drop to a new page if the next `height` wouldn't fit above the bottom margin.
    fn need(&mut self, height: f32) {
        if self.y - height < self.layout.text_bottom() {
            self.start_page();
        }
    }

    /// The book cover (#582), when a cover image is configured: a full-page page 0
    /// holding the cover image, scaled to cover the whole media box — the trim plus any
    /// bleed allowance (#652) — with the aspect ratio preserved and centered, so
    /// full-bleed art reaches the page edge with no white border (POD needs this).
    /// Overflow past the media box is cropped by the page (`clip_media` clips the
    /// content to the media box). The page carries no running head or folio. No-op when
    /// no cover is set, leaving the output unchanged.
    fn cover_page(&mut self) {
        let Some((name, w, h)) = self
            .image(ImageRole::Cover)
            .map(|i| (i.name.clone(), i.width as f32, i.height as f32))
        else {
            return;
        };
        self.start_page();
        // No running head or folio on the cover.
        self.meta.last_mut().unwrap().blank = true;
        let (page_w, page_h) = (self.layout.media_w(), self.layout.media_h());
        // Scale to cover the whole media box (bleed included), keeping the aspect ratio,
        // and center; overflow is cropped so there's no white margin at the trim edge.
        let scale = (page_w / w).max(page_h / h);
        let (dw, dh) = (w * scale, h * scale);
        let x = (page_w - dw) / 2.0;
        let y = (page_h - dh) / 2.0;
        // Clip to the media box so cover-fill overflow (a differing aspect ratio) is
        // cropped at the page edge rather than spilling past it.
        self.cur().push_str(&format!(
            "q 0 0 {} {} re W n {} 0 0 {} {} {} cm /{name} Do Q\n",
            num(page_w),
            num(page_h),
            num(dw),
            num(dh),
            num(x),
            num(y)
        ));
    }

    /// The book title page: series, title, subtitle, author, and the book epigraph,
    /// centered and set a third of the way down.
    fn title_page(
        &mut self,
        series: Option<&str>,
        title: &str,
        subtitle: Option<&str>,
        author: Option<&str>,
        epigraph: Option<&Epigraph>,
    ) {
        self.open_section();
        if self.image(ImageRole::Title).is_some() {
            // A title image sits at the top, with the text block below it (#490).
            self.y = self.layout.text_top() - BODY_LEADING;
            self.place_image(
                ImageRole::Title,
                self.layout.text_w(),
                self.layout.page_h * 0.3,
                BODY_LEADING * 2.0,
            );
        } else {
            self.y = self.layout.trim_y(0.62);
        }
        if let Some(s) = series {
            self.centered(s, false, false, false, BODY_SIZE, BODY_LEADING * 2.0);
        }
        // The book title uses the heading font (#638).
        self.centered(title, true, false, true, TITLE_SIZE, TITLE_SIZE * 1.4);
        if let Some(s) = subtitle {
            self.centered(
                s,
                false,
                true,
                false,
                EPIGRAPH_SIZE + 3.0,
                BODY_LEADING * 1.6,
            );
        }
        if let Some(a) = author {
            self.centered(
                &format!("by {a}"),
                false,
                false,
                false,
                BODY_SIZE,
                BODY_LEADING,
            );
        }
        if let Some(o) = self
            .bits
            .title_ornament
            .clone()
            .filter(|o| self.layout.renders(o))
        {
            self.centered(&o, false, false, false, HEADING_SIZE, BODY_LEADING);
        }
        if let Some(e) = epigraph {
            self.y -= BODY_LEADING * 3.0;
            self.epigraph(e);
        }
    }

    /// The copyright page (#583), when publishing metadata warrants one: the rights
    /// line, publisher, publication date, and ISBN, centered low on a fresh page.
    /// Slots into the conventional front-matter order after the title page (cover →
    /// title → copyright → body). No-op when no metadata is set, leaving the output
    /// unchanged. Not a bookmark.
    fn copyright_page(&mut self, publishing: &PubMeta) {
        let lines = copyright_lines(publishing);
        if lines.is_empty() {
            return;
        }
        self.open_section();
        // Set the block low on the page, the conventional copyright-page placement.
        self.y = self.layout.trim_y(0.3);
        for line in lines {
            self.centered(&line, false, false, false, EPIGRAPH_SIZE, EPIGRAPH_LEADING);
        }
    }

    /// Plan the Contents page(s): wrap each title (heading first, then every entry)
    /// into lines paginated across TOC pages, returning where each line lands. The
    /// layout depends only on the titles (part vs. entry) — the folio gutter is a
    /// fixed reserve — so the reservation pass and the fill pass agree on the page
    /// count and line positions, and the fill pass can attach the actual folios.
    fn plan_contents(&self, titles: &[(String, bool)]) -> Vec<TocLine> {
        let col = self.layout.text_w();
        let title_w = col - TOC_FOLIO_GUTTER;
        let top = self.layout.text_top();
        let bottom = self.layout.text_bottom();
        let mut lines = Vec::new();
        let mut page = 0usize;
        let mut y = top;
        // Wrap `text` (bold headings) at `size`/`leading` into `width`, advancing y and
        // paginating, tagging the first line of the block with `entry` for its folio.
        let emit = |lines: &mut Vec<TocLine>,
                    page: &mut usize,
                    y: &mut f32,
                    text: &str,
                    bold: bool,
                    size: f32,
                    leading: f32,
                    width: f32,
                    entry: Option<usize>| {
            let words = words_of(&[Run::new(text, bold, false)], false);
            let wrapped = wrap_lines(&self.layout, &words, size, |_| width);
            for (i, (line, _w)) in wrapped.into_iter().enumerate() {
                if *y - leading < bottom {
                    *page += 1;
                    *y = top;
                }
                lines.push(TocLine {
                    page: *page,
                    y: *y,
                    runs: segments(&line),
                    size,
                    // The folio rides the first line of the entry (i == 0).
                    folio_entry: entry.filter(|_| i == 0),
                });
                *y -= leading;
            }
        };
        // The "Contents" heading, then a gap.
        emit(
            &mut lines,
            &mut page,
            &mut y,
            CONTENTS_TITLE,
            true,
            HEADING_SIZE,
            HEADING_LEADING * 1.6,
            col,
            None,
        );
        for (idx, (title, part)) in titles.iter().enumerate() {
            if *part {
                y -= TOC_LEADING * 0.5; // set a part heading apart from prior chapters
            }
            let size = if *part { BODY_SIZE + 1.0 } else { BODY_SIZE };
            emit(
                &mut lines,
                &mut page,
                &mut y,
                title,
                *part,
                size,
                TOC_LEADING,
                title_w,
                Some(idx),
            );
        }
        lines
    }

    /// Reserve the front-matter Contents page(s) (#603): plan the layout, open the
    /// section (recto start + blank verso like any other), and push that many empty
    /// pages so the body that follows lands on its true folios. Returns the plan for
    /// [`fill_contents`]. No-op (returns `None`) when the book has no sections.
    fn reserve_contents(&mut self, titles: &[(String, bool)]) -> Option<TocPlan> {
        if titles.is_empty() {
            return None;
        }
        let lines = self.plan_contents(titles);
        let pages = lines.iter().map(|l| l.page).max().unwrap_or(0) + 1;
        let start = self.open_section();
        for _ in 1..pages {
            self.start_page();
        }
        Some(TocPlan { start, lines })
    }

    /// Fill the reserved Contents page(s) with the planned lines, drawing each title
    /// and the right-aligned folio of the entry it heads (the folio label matches the
    /// page decoration's roman-front/Arabic-body scheme, #651). Runs after the body so
    /// folios are final.
    fn fill_contents(&mut self, plan: Option<TocPlan>, folios: &[Option<usize>]) {
        let Some(plan) = plan else { return };
        debug_assert_eq!(
            plan.lines
                .iter()
                .filter(|l| l.folio_entry.is_some())
                .count(),
            folios.len(),
            "every Contents entry got a planned line"
        );
        let col = self.layout.text_w();
        for line in &plan.lines {
            let idx = plan.start + line.page;
            let left = self.layout.left(idx);
            let mut block = self.glyph_block(left, line.y, line.size, seg_refs(&line.runs));
            // The right-aligned folio on the entry's first line, labeled under the
            // same scheme the page decoration uses (#651).
            if let Some(label) = line
                .folio_entry
                .and_then(|i| folios.get(i).copied().flatten())
                .and_then(|folio| self.folio_label(folio))
            {
                let w = self
                    .layout
                    .measure(&label, line.size, FontStyle::REGULAR, false);
                let x = left + col - w;
                block.push_str(&self.text_op(x, line.y, &label, line.size));
            }
            self.pages[idx].push_str(&block);
        }
    }

    /// A standalone divider page (a part or a series book): the title (with the given
    /// alignment), and an optional ornament glyph beneath it.
    fn divider_page(
        &mut self,
        title: &str,
        size: f32,
        ornament: Option<&str>,
        align: Align,
    ) -> usize {
        let page = self.open_section();
        self.y = self.layout.trim_y(0.5);
        self.heading_line(title, size, size * 1.4, align);
        if let Some(o) = ornament.filter(|o| self.layout.renders(o)) {
            self.centered(o, false, false, false, HEADING_SIZE, BODY_LEADING);
        }
        page
    }

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
        let initial = match first_word.text.chars().next() {
            Some(c) => c,
            None => return,
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
        let cap_baseline = self.y - (LINES as f32 - 1.0) * BODY_LEADING;
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
        let lines = wrap_lines(&self.layout, &words, BODY_SIZE, line_w);
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
        let width = self.layout.text_w() - 2.0 * EPIGRAPH_INSET;
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

    /// The running head + folio for page `i` (the screen profile centers the folio;
    /// print places it at the outer corner and adds a running head off opener pages).
    fn decorations(&self, i: usize) -> String {
        let m = &self.meta[i];
        if m.blank {
            return String::new();
        }
        let mut s = String::new();
        let recto = i.is_multiple_of(2);
        let left = self.layout.left(i);
        let col = self.layout.text_w();
        // Folio under the roman-front/Arabic-body scheme (#651). The title page and
        // blank/cover pages carry no folio (`folio_label` returns `None`).
        if let Some(label) = self.folio_label(i) {
            let size = if self.layout.running_heads {
                HEAD_SIZE
            } else {
                BODY_SIZE
            };
            let w = self.layout.measure(&label, size, FontStyle::REGULAR, false);
            let x = if self.layout.running_heads {
                if recto {
                    left + col - w
                } else {
                    left
                }
            } else {
                left + (col - w) / 2.0
            };
            s.push_str(&self.text_op(x, self.layout.folio_baseline(), &label, size));
        }
        // Running head, print only, off the title and section-opening pages.
        if self.layout.running_heads && i > 0 && !m.opener {
            let head = if recto { &m.recto } else { &m.verso };
            if !head.is_empty() {
                let w = self
                    .layout
                    .measure(head, HEAD_SIZE, FontStyle::REGULAR, false);
                let x = left + (col - w) / 2.0;
                let y = self.layout.head_baseline();
                s.push_str(&self.text_op(x, y, head, HEAD_SIZE));
            }
        }
        s
    }

    /// A `BT…ET` text run showing one string at `(x, y)` in the regular face — used
    /// for running heads and folios, resolved to glyph ids like body text.
    fn text_op(&self, x: f32, y: f32, text: &str, size: f32) -> String {
        self.glyph_block(
            x,
            y,
            size,
            std::iter::once((text, FontStyle::REGULAR, false)),
        )
    }

    /// Serialize the pages and outline into the bytes of a PDF file.
    fn finish(mut self) -> Vec<u8> {
        if self.pages.is_empty() {
            self.start_page();
        }
        let n_pages = self.pages.len();
        // Build the page decorations (running heads + folios) first: they resolve
        // glyphs too, and every used glyph must be recorded before the fonts are
        // subset below.
        let decorations: Vec<String> = (0..n_pages).map(|i| self.decorations(i)).collect();
        let mut objects: ObjMap = BTreeMap::new();
        // 1 catalog, 2 pages, 3 outlines, then fonts, pages+contents, outline items.
        let mut next = 4u32;
        let font_names = self.layout.fonts.objects(&mut next, &mut objects);
        let image_names = emit_images(&self.images, &mut next, &mut objects);
        let mut page_obj = Vec::with_capacity(n_pages);
        let mut content_obj = Vec::with_capacity(n_pages);
        for _ in 0..n_pages {
            page_obj.push(next);
            next += 1;
            content_obj.push(next);
            next += 1;
        }
        let outline = std::mem::take(&mut self.outline);
        let mut item_count = 0usize;
        let (first, last) = flatten_outline(
            &outline,
            3,
            &mut next,
            &page_obj,
            self.layout.media_h(),
            &mut objects,
            &mut item_count,
        );

        // Page tree (MediaBox + font resources inherited by every page).
        let kids: String = page_obj.iter().map(|id| format!("{id} 0 R ")).collect();
        let xobjects = if image_names.is_empty() {
            String::new()
        } else {
            let entries: String = image_names
                .iter()
                .map(|(name, id)| format!("/{name} {id} 0 R "))
                .collect();
            format!(" /XObject << {entries}>>")
        };
        let fonts: String = font_names
            .iter()
            .map(|(name, id, _)| format!("/{name} {id} 0 R "))
            .collect();
        let resources = format!("/Resources << /Font << {fonts}>>{xobjects} >>");
        // Subsetting renumbered glyphs, so remap the content-stream glyph ids.
        let maps: Vec<(&str, &GidMap)> = font_names.iter().map(|(name, _, m)| (*name, m)).collect();
        objects.insert(
            2,
            format!(
                "<< /Type /Pages /Kids [ {kids}] /Count {n_pages} /MediaBox [0 0 {} {}] {resources} >>",
                num(self.layout.media_w()),
                num(self.layout.media_h())
            )
            .into_bytes(),
        );
        for i in 0..n_pages {
            objects.insert(
                page_obj[i],
                format!(
                    "<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>",
                    content_obj[i]
                )
                .into_bytes(),
            );
            let mut content = std::mem::take(&mut self.pages[i]);
            content.push_str(&decorations[i]);
            let content = remap_glyph_ids(&content, &maps);
            // FlateDecode-compress the page content stream (#662); the helper adds
            // `/Filter /FlateDecode` and the compressed `/Length`.
            objects.insert(content_obj[i], flate_stream(content.as_bytes(), ""));
        }
        let outlines = if item_count == 0 {
            "<< /Type /Outlines /Count 0 >>".to_string()
        } else {
            format!(
                "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {item_count} >>",
                first.unwrap(),
                last.unwrap()
            )
        };
        objects.insert(3, outlines.into_bytes());
        objects.insert(
            1,
            b"<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R /PageMode /UseOutlines >>".to_vec(),
        );
        serialize(&objects)
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
