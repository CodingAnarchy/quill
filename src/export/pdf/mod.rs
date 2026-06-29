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

mod font;
mod images;
mod wrap;
mod write;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use self::font::{glyph_run, Fonts, GidMap, ObjMap};
use self::images::{decode_images, emit_images, PdfImage};
use self::wrap::{segments, words_of, wrap_lines, Word};
use self::write::{flatten_outline, remap_glyph_ids, serialize, OutlineItem};
use super::{
    body_blocks, classify_block, inline_runs, Chapter, Document, ExportStyle, ImageRole, Prose,
    SCENE_BREAK,
};
use crate::frontmatter::Epigraph;

// Type sizes and vertical rhythm, shared by both profiles (in points).
const BODY_SIZE: f32 = 12.0;
const BODY_LEADING: f32 = 15.0;
const INDENT: f32 = 18.0; // first-line paragraph indent (~1.5em)
const HEADING_SIZE: f32 = 17.0;
const HEADING_LEADING: f32 = 21.0;
const TITLE_SIZE: f32 = 26.0;
const PART_SIZE: f32 = 20.0;
const EPIGRAPH_SIZE: f32 = 11.0;
const EPIGRAPH_LEADING: f32 = 14.0;
const EPIGRAPH_INSET: f32 = 36.0; // left/right inset for the set-apart block
const HEAD_SIZE: f32 = 10.0; // running heads + folios in the print profile

/// The default print trim, used when a project sets no `[print] trim`.
pub const DEFAULT_TRIM: &str = "6x9";

/// Opt-in print-interior settings (#464), read from the manifest `[print]` table.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct PrintConfig {
    /// The book trim size, e.g. `"6x9"` (see [`trim_points`]). Absent → 6×9.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<String>,
}

/// A named trim size in PDF points (1/72"), or `None` if unrecognized.
pub fn trim_points(name: &str) -> Option<(f32, f32)> {
    let (w, h) = match name {
        "6x9" => (6.0, 9.0),
        "5x8" => (5.0, 8.0),
        "5.5x8.5" => (5.5, 8.5),
        _ => return None,
    };
    Some((w * 72.0, h * 72.0))
}

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
/// (points): mirrored margins + gutter, running heads, recto starts, embedded fonts,
/// honoring the designed `[style]` (#489).
pub fn to_pdf_print(
    doc: &Document,
    trim_w: f32,
    trim_h: f32,
    style: &ExportStyle,
) -> std::io::Result<Vec<u8>> {
    Ok(render_book(
        doc,
        style,
        Layout::print(trim_w, trim_h, style),
    ))
}

/// Compile a whole series to one print-ready interior PDF (#464).
pub fn to_pdf_series_print(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
    trim_w: f32,
    trim_h: f32,
    style: &ExportStyle,
) -> std::io::Result<Vec<u8>> {
    Ok(render_series(
        books,
        series,
        author,
        style,
        Layout::print(trim_w, trim_h, style),
    ))
}

/// Lay out one standalone book onto a fresh `Pdf` with `layout` and serialize it.
fn render_book(doc: &Document, style: &ExportStyle, layout: Layout) -> Vec<u8> {
    let mut pdf = Pdf::new(layout, StyleBits::from_style(style), decode_images(style));
    pdf.set_heads(&doc.title, doc.author.as_deref().unwrap_or(&doc.title));
    pdf.title_page(
        doc.series.as_deref(),
        &doc.title,
        doc.subtitle.as_deref(),
        doc.author.as_deref(),
        doc.epigraph.as_ref(),
    );
    pdf.outline = book_outline(&mut pdf, doc);
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
    pdf.title_page(None, title, None, author, None);
    let mut outline = Vec::new();
    for book in books {
        pdf.set_heads(&book.title, author.unwrap_or(&book.title));
        let page = pdf.divider_page(&book.title, PART_SIZE, None, Align::Center);
        let mut item = OutlineItem::new(book.title.clone(), page);
        item.children = book_outline(&mut pdf, book);
        outline.push(item);
    }
    pdf.outline = outline;
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

/// The page geometry and font plan of one render — the only difference between the
/// screen and print profiles.
struct Layout {
    page_w: f32,
    page_h: f32,
    margin_top: f32,
    margin_bottom: f32,
    /// The binding-side and outer margins. Equal for the screen profile; mirrored
    /// by page parity for print (inside = the gutter side).
    margin_inside: f32,
    margin_outside: f32,
    /// Flip inside/outside by page parity (the binding gutter) — print only.
    mirrored: bool,
    /// Draw verso/recto running heads — print only.
    running_heads: bool,
    /// Begin each section on a recto (odd) page, inserting a blank verso if needed.
    recto_starts: bool,
    /// The embedded Type0 fonts (body faces + broad fallback) — both profiles embed
    /// (#492), so the PDF renders the full Unicode its fonts cover.
    fonts: Fonts,
}

impl Layout {
    /// The screen proof (#463): US-Letter, uniform 1" margins; the body serif (or a
    /// configured font) and the fallback embedded (#492).
    fn screen(style: &ExportStyle) -> Self {
        Layout {
            page_w: 612.0,
            page_h: 792.0,
            margin_top: 72.0,
            margin_bottom: 72.0,
            margin_inside: 72.0,
            margin_outside: 72.0,
            mirrored: false,
            running_heads: false,
            recto_starts: false,
            fonts: Fonts::new(style),
        }
    }

    /// The print interior (#464) at `trim_w`×`trim_h` points: generous top/bottom
    /// margins, a wider binding (inside) margin than outside for the gutter, with
    /// running heads, recto starts, and embedded fonts. Margins clear KDP's minimums
    /// for a typical book.
    fn print(trim_w: f32, trim_h: f32, style: &ExportStyle) -> Self {
        Layout {
            page_w: trim_w,
            page_h: trim_h,
            margin_top: 54.0,     // 0.75"
            margin_bottom: 54.0,  // 0.75"
            margin_inside: 63.0,  // 0.875" (0.5" outside + 0.375" gutter)
            margin_outside: 36.0, // 0.5"
            mirrored: true,
            running_heads: true,
            recto_starts: true,
            fonts: Fonts::new(style),
        }
    }

    fn text_top(&self) -> f32 {
        self.page_h - self.margin_top
    }

    fn text_bottom(&self) -> f32 {
        self.margin_bottom
    }

    fn text_w(&self) -> f32 {
        self.page_w - self.margin_inside - self.margin_outside
    }

    /// The left edge of the text column on page `idx`. Recto pages are even (page 1
    /// is the first, a recto); on a mirrored layout the binding (inside) margin sits
    /// on the left of a recto and the right of a verso.
    fn left(&self, idx: usize) -> f32 {
        if self.mirrored && idx % 2 == 1 {
            self.margin_outside
        } else {
            self.margin_inside
        }
    }

    /// The rendered width of `text` at `size` for the given face, in points.
    fn measure(&self, text: &str, size: f32, bold: bool, italic: bool) -> f32 {
        text.chars()
            .map(|c| self.fonts.advance_em(c, bold, italic))
            .sum::<f32>()
            / 1000.0
            * size
    }

    /// Whether the fonts can render every character of `glyph`.
    fn renders(&self, glyph: &str) -> bool {
        glyph.chars().all(|c| self.fonts.covers(c))
    }
}

/// What a page is, for decoration: a section opener and blank pages get no running
/// head; blank pages (inserted for recto starts) get no folio either.
struct PageMeta {
    verso: String,
    recto: String,
    opener: bool,
    blank: bool,
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
        }
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
            self.y = self.layout.page_h * 0.62;
        }
        if let Some(s) = series {
            self.centered(s, false, false, BODY_SIZE, BODY_LEADING * 2.0);
        }
        self.centered(title, true, false, TITLE_SIZE, TITLE_SIZE * 1.4);
        if let Some(s) = subtitle {
            self.centered(s, false, true, EPIGRAPH_SIZE + 3.0, BODY_LEADING * 1.6);
        }
        if let Some(a) = author {
            self.centered(&format!("by {a}"), false, false, BODY_SIZE, BODY_LEADING);
        }
        if let Some(o) = self
            .bits
            .title_ornament
            .clone()
            .filter(|o| self.layout.renders(o))
        {
            self.centered(&o, false, false, HEADING_SIZE, BODY_LEADING);
        }
        if let Some(e) = epigraph {
            self.y -= BODY_LEADING * 3.0;
            self.epigraph(e);
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
        self.y = self.layout.page_h * 0.5;
        self.heading_line(title, size, size * 1.4, align);
        if let Some(o) = ornament.filter(|o| self.layout.renders(o)) {
            self.centered(o, false, false, HEADING_SIZE, BODY_LEADING);
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

    /// A preformatted (fenced) block: each line drawn verbatim, left-aligned, its
    /// breaks and indentation preserved and no emphasis applied, set apart by space
    /// above and below (#522). Falls back to the body serif — a bundled monospace
    /// face is the follow-up, so long lines aren't yet wrapped or fixed-width.
    fn preformatted(&mut self, code: &str) {
        self.need(BODY_LEADING * 1.5);
        self.y -= BODY_LEADING * 0.5;
        for line in code.lines() {
            self.need(BODY_LEADING);
            if !line.is_empty() {
                let left = self.left();
                self.draw_line(left, self.y, &[(line.to_string(), false, false)], BODY_SIZE);
            }
            self.y -= BODY_LEADING;
        }
        self.y -= BODY_LEADING * 0.5;
    }

    /// A normal body paragraph: first-line indented, left-aligned, with `*`/`**`
    /// emphasis as italic/bold runs.
    fn paragraph(&mut self, runs: &[(String, bool, bool)]) {
        let words = words_of(runs);
        self.flow(
            &words,
            self.layout.text_w(),
            INDENT,
            BODY_SIZE,
            BODY_LEADING,
            Align::Left,
        );
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
        self.centered(&glyph, false, false, BODY_SIZE, BODY_LEADING * 1.5);
    }

    /// A body sub-heading (`##` …), bold, honoring the heading alignment.
    fn sub_heading(&mut self, h: &str) {
        self.need(BODY_LEADING * 2.0);
        self.y -= BODY_LEADING * 0.5;
        let align = self.bits.heading_align;
        self.heading_line(h, BODY_SIZE, BODY_LEADING, align);
    }

    /// A bold heading line at `align` (chapter/part titles, sub-headings).
    fn heading_line(&mut self, text: &str, size: f32, leading: f32, align: Align) {
        let words = words_of(&[(text.to_string(), true, false)]);
        self.flow(&words, self.layout.text_w(), 0.0, size, leading, align);
    }

    /// The chapter's opening paragraph with a three-line drop-cap initial: the first
    /// character is set large at the left margin and the first lines are indented to
    /// clear it (#489 / mirrors the EPUB `.opening` first-letter treatment).
    fn dropped_paragraph(&mut self, runs: &[(String, bool, bool)]) {
        const LINES: usize = 3;
        let mut words = words_of(runs);
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
            .measure(&initial.to_string(), cap_size, false, false)
            + 2.0;
        // Reserve the cap block so the initial and its first lines stay together.
        self.need(BODY_LEADING * LINES as f32);
        let left = self.left();
        let cap_baseline = self.y - (LINES as f32 - 1.0) * BODY_LEADING;
        self.draw_line(
            left,
            cap_baseline,
            &[(initial.to_string(), false, false)],
            cap_size,
        );
        // The first LINES lines are inset by the cap width; the rest run full width.
        let full = self.layout.text_w();
        let lines = wrap_lines(&self.layout, &words, BODY_SIZE, |i| {
            if i < LINES {
                full - cap_w
            } else {
                full
            }
        });
        for (i, (line, _w)) in lines.into_iter().enumerate() {
            self.need(BODY_LEADING);
            let x = self.left() + if i < LINES { cap_w } else { 0.0 };
            let segs = segments(&line);
            self.draw_line(x, self.y, &segs, BODY_SIZE);
            self.y -= BODY_LEADING;
        }
    }

    /// An epigraph: the lines set apart (inset, italic, single-spaced) with the
    /// attribution on its own line.
    fn epigraph(&mut self, e: &Epigraph) {
        let width = self.layout.text_w() - 2.0 * EPIGRAPH_INSET;
        for line in e.text.lines() {
            let words = words_of(&[(line.to_string(), false, true)]);
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
            let words = words_of(&[(format!("— {by}"), false, true)]);
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

    /// A single centered line (titles, headings, scene breaks) followed by a gap.
    fn centered(&mut self, text: &str, bold: bool, italic: bool, size: f32, leading: f32) {
        let words = words_of(&[(text.to_string(), bold, italic)]);
        self.flow(
            &words,
            self.layout.text_w(),
            0.0,
            size,
            leading,
            Align::Center,
        );
    }

    /// Wrap `words` into `max_width` and emit each line at `size`/`leading`,
    /// paginating as needed. `first_indent` indents only the first line (left
    /// alignment); `align` places each line within the text column.
    fn flow(
        &mut self,
        words: &[Word],
        max_width: f32,
        first_indent: f32,
        size: f32,
        leading: f32,
        align: Align,
    ) {
        let col = self.layout.text_w();
        // The first line is narrowed by the indent; the rest run the full width.
        let lines = wrap_lines(&self.layout, words, size, |i| {
            if i == 0 {
                max_width - first_indent
            } else {
                max_width
            }
        });
        for (i, (line, line_w)) in lines.into_iter().enumerate() {
            self.need(leading);
            let left = self.left();
            let indent = if i == 0 { first_indent } else { 0.0 };
            let x = match align {
                Align::Left => left + indent,
                Align::Center => left + (col - line_w) / 2.0,
                Align::LeftInset(pad) => left + pad,
                Align::RightInset(pad) => left + col - pad - line_w,
            };
            let segs = segments(&line);
            self.draw_line(x, self.y, &segs, size);
            self.y -= leading;
        }
    }

    /// Emit one positioned line of styled segments into the current page's content.
    /// Each segment's text is resolved to glyph ids (body face, or the fallback for
    /// glyphs it lacks) and shown as Identity-H hex runs.
    fn draw_line(&mut self, x: f32, y: f32, segs: &[(String, bool, bool)], size: f32) {
        let block = self.glyph_block(
            x,
            y,
            size,
            segs.iter().map(|(t, b, i)| (t.as_str(), *b, *i)),
        );
        self.cur().push_str(&block);
    }

    /// A `BT…ET` text block at `(x, y)`: each `(text, bold, italic)` segment resolved
    /// to glyph-id hex runs (`/Fn` font switches between body faces and the fallback).
    fn glyph_block<'a>(
        &self,
        x: f32,
        y: f32,
        size: f32,
        segments: impl Iterator<Item = (&'a str, bool, bool)>,
    ) -> String {
        let mut s = format!("BT\n1 0 0 1 {} {} Tm\n", num(x), num(y));
        let mut cur_font = "";
        for (text, bold, italic) in segments {
            for (name, hex) in glyph_run(&self.layout.fonts, text, bold, italic) {
                if name != cur_font {
                    s.push_str(&format!("/{name} {} Tf\n", num(size)));
                    cur_font = name;
                }
                s.push_str(&format!("<{hex}> Tj\n"));
            }
        }
        s.push_str("ET\n");
        s
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
        // Folio (the title page, index 0, is unnumbered). Number = physical page.
        if i > 0 {
            let label = (i + 1).to_string();
            let size = if self.layout.running_heads {
                HEAD_SIZE
            } else {
                BODY_SIZE
            };
            let w = self.layout.measure(&label, size, false, false);
            let x = if self.layout.running_heads {
                if recto {
                    left + col - w
                } else {
                    left
                }
            } else {
                left + (col - w) / 2.0
            };
            s.push_str(&self.text_op(x, self.layout.text_bottom() * 0.5, &label, size));
        }
        // Running head, print only, off the title and section-opening pages.
        if self.layout.running_heads && i > 0 && !m.opener {
            let head = if recto { &m.recto } else { &m.verso };
            if !head.is_empty() {
                let w = self.layout.measure(head, HEAD_SIZE, false, false);
                let x = left + (col - w) / 2.0;
                let y = self.layout.page_h - self.layout.margin_top * 0.5;
                s.push_str(&self.text_op(x, y, head, HEAD_SIZE));
            }
        }
        s
    }

    /// A `BT…ET` text run showing one string at `(x, y)` in the regular face — used
    /// for running heads and folios, resolved to glyph ids like body text.
    fn text_op(&self, x: f32, y: f32, text: &str, size: f32) -> String {
        self.glyph_block(x, y, size, std::iter::once((text, false, false)))
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
            self.layout.page_h,
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
                num(self.layout.page_w),
                num(self.layout.page_h)
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
            objects.insert(
                content_obj[i],
                format!(
                    "<< /Length {} >>\nstream\n{content}\nendstream",
                    content.len()
                )
                .into_bytes(),
            );
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
    /// Flush to a left edge inset by this many points (epigraph body).
    LeftInset(f32),
    /// Flush to a right edge inset by this many points (epigraph attribution).
    RightInset(f32),
}

/// Format a coordinate/size compactly (drop a trailing `.00`).
pub(super) fn num(x: f32) -> String {
    if (x - x.round()).abs() < 0.005 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.2}")
    }
}

#[cfg(test)]
mod tests;
