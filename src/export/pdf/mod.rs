//! PDF export (#463) and the print-ready interior profile (#464). A PDF is
//! hand-written the same way the `.docx` OOXML and `.epub` XHTML are — a header, a
//! body of numbered objects (catalog, page tree, fonts, per-page content streams,
//! and a bookmark outline), an xref table, and a trailer. We lay the book out
//! ourselves; only glyph metrics are read from the bundled fonts (see [`font`]).
//!
//! Two [`Layout`]s share that machinery: the **screen** profile (#463) is US-Letter
//! with the standard-14 Times faces (nothing embedded) for a readable proof; the
//! **print** profile (#464) is a chosen trim size with mirrored inside/outside
//! margins and a binding gutter, running heads, recto chapter starts, and the
//! bundled fonts embedded whole — what print-on-demand services require. Text is
//! WinAnsi (English / Latin-1 plus the common typographic punctuation); characters
//! outside that set render as `?`. Designed typography (font choice, ornaments) is
//! the styling epic; cover/bleed are a separate artifact.

mod font;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use self::font::{font_name, pdf_string, FontPlan, ObjMap};
use super::{classify_block, inline_runs, Chapter, Document, Prose, SCENE_BREAK};
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

/// Compile a [`Document`] to a screen PDF (#463): US-Letter, standard-14 Times.
pub fn to_pdf(doc: &Document) -> std::io::Result<Vec<u8>> {
    Ok(render_book(doc, Layout::screen()))
}

/// Compile a whole series to one screen PDF (#332/#463).
pub fn to_pdf_series(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
) -> std::io::Result<Vec<u8>> {
    Ok(render_series(books, series, author, Layout::screen()))
}

/// Compile a [`Document`] to a print-ready interior PDF (#464) at the given trim
/// (points): mirrored margins + gutter, running heads, recto starts, embedded fonts.
pub fn to_pdf_print(doc: &Document, trim_w: f32, trim_h: f32) -> std::io::Result<Vec<u8>> {
    Ok(render_book(doc, Layout::print(trim_w, trim_h)))
}

/// Compile a whole series to one print-ready interior PDF (#464).
pub fn to_pdf_series_print(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
    trim_w: f32,
    trim_h: f32,
) -> std::io::Result<Vec<u8>> {
    Ok(render_series(
        books,
        series,
        author,
        Layout::print(trim_w, trim_h),
    ))
}

/// Lay out one standalone book onto a fresh `Pdf` with `layout` and serialize it.
fn render_book(doc: &Document, layout: Layout) -> Vec<u8> {
    let mut pdf = Pdf::new(layout);
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
    layout: Layout,
) -> Vec<u8> {
    let mut pdf = Pdf::new(layout);
    let title = series.unwrap_or("Series");
    pdf.set_heads(title, author.unwrap_or(title));
    pdf.title_page(None, title, None, author, None);
    let mut outline = Vec::new();
    for book in books {
        pdf.set_heads(&book.title, author.unwrap_or(&book.title));
        let page = pdf.divider_page(&book.title, PART_SIZE);
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
                let mut item = OutlineItem::new(name.clone(), pdf.divider_page(name, PART_SIZE));
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
    fonts: FontPlan,
}

impl Layout {
    /// The screen proof (#463): US-Letter, uniform 1" margins, standard-14 Times.
    fn screen() -> Self {
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
            fonts: FontPlan::Standard,
        }
    }

    /// The print interior (#464) at `trim_w`×`trim_h` points: generous top/bottom
    /// margins, a wider binding (inside) margin than outside for the gutter, with
    /// running heads, recto starts, and embedded fonts. Margins clear KDP's minimums
    /// for a typical book.
    fn print(trim_w: f32, trim_h: f32) -> Self {
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
            fonts: FontPlan::embedded(),
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
            .map(|c| self.fonts.em_width(c, bold, italic))
            .sum::<f32>()
            / 1000.0
            * size
    }
}

/// One word carrying the emphasis it inherits from its run, the unit line-wrapping
/// works in.
struct Word {
    text: String,
    bold: bool,
    italic: bool,
}

/// A bookmark: a title, the page it jumps to, and any nested entries.
struct OutlineItem {
    title: String,
    page: usize,
    children: Vec<OutlineItem>,
}

impl OutlineItem {
    /// A bookmark to `page`; nest entries by pushing onto `children` afterward.
    fn new(title: String, page: usize) -> Self {
        OutlineItem {
            title,
            page,
            children: Vec::new(),
        }
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

/// Accumulates per-page content as the book is laid out, then [`Pdf::finish`]
/// decorates the pages (running heads, folios) and serializes the file.
struct Pdf {
    layout: Layout,
    pages: Vec<String>,
    meta: Vec<PageMeta>,
    y: f32,
    outline: Vec<OutlineItem>,
    head_verso: String,
    head_recto: String,
}

impl Pdf {
    fn new(layout: Layout) -> Self {
        Pdf {
            layout,
            pages: Vec::new(),
            meta: Vec::new(),
            y: 0.0,
            outline: Vec::new(),
            head_verso: String::new(),
            head_recto: String::new(),
        }
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
        self.y = self.layout.page_h * 0.62;
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
        if let Some(e) = epigraph {
            self.y -= BODY_LEADING * 3.0;
            self.epigraph(e);
        }
    }

    /// A standalone divider page (a part or a series book): one centered line.
    fn divider_page(&mut self, title: &str, size: f32) -> usize {
        let page = self.open_section();
        self.y = self.layout.page_h * 0.5;
        self.centered(title, true, false, size, size * 1.4);
        page
    }

    /// One chapter on a fresh page: a centered title, an optional epigraph, then the
    /// body. Returns the page it starts on (for its bookmark).
    fn chapter(&mut self, ch: &Chapter) -> usize {
        let page = self.open_section();
        self.y -= HEADING_LEADING * 2.0;
        self.centered(&ch.title, true, false, HEADING_SIZE, HEADING_LEADING * 1.6);
        if let Some(e) = &ch.epigraph {
            self.epigraph(e);
            self.y -= BODY_LEADING;
        }
        for block in ch.body.split("\n\n") {
            self.prose_block(block);
        }
        page
    }

    /// Render one body block: a scene break (centered glyph), a sub-heading
    /// (centered, bold), or a normal indented paragraph with `*`/`**` emphasis.
    fn prose_block(&mut self, block: &str) {
        match classify_block(block) {
            None => {}
            Some(Prose::SceneBreak) => {
                self.need(BODY_LEADING * 2.0);
                self.y -= BODY_LEADING * 0.5;
                self.centered(SCENE_BREAK, false, false, BODY_SIZE, BODY_LEADING * 1.5);
            }
            Some(Prose::Heading(h)) => {
                self.need(BODY_LEADING * 2.0);
                self.y -= BODY_LEADING * 0.5;
                let words = words_of(&[(h, true, false)]);
                self.flow(
                    &words,
                    self.layout.text_w(),
                    0.0,
                    BODY_SIZE,
                    BODY_LEADING,
                    Align::Center,
                );
            }
            Some(Prose::Paragraph(text)) => {
                let words = words_of(&inline_runs(&text));
                self.flow(
                    &words,
                    self.layout.text_w(),
                    INDENT,
                    BODY_SIZE,
                    BODY_LEADING,
                    Align::Left,
                );
            }
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
        let lines = wrap(&self.layout, words, max_width, first_indent, size);
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
    fn draw_line(&mut self, x: f32, y: f32, segs: &[(String, bool, bool)], size: f32) {
        let mut s = String::from("BT\n");
        s.push_str(&format!("1 0 0 1 {} {} Tm\n", num(x), num(y)));
        let mut cur_font = "";
        for (text, bold, italic) in segs {
            let f = font_name(*bold, *italic);
            if f != cur_font {
                s.push_str(&format!("/{f} {} Tf\n", num(size)));
                cur_font = f;
            }
            s.push_str(&format!("({}) Tj\n", pdf_string(text)));
        }
        s.push_str("ET\n");
        self.cur().push_str(&s);
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
            s.push_str(&text_op(x, self.layout.text_bottom() * 0.5, &label, size));
        }
        // Running head, print only, off the title and section-opening pages.
        if self.layout.running_heads && i > 0 && !m.opener {
            let head = if recto { &m.recto } else { &m.verso };
            if !head.is_empty() {
                let w = self.layout.measure(head, HEAD_SIZE, false, false);
                let x = left + (col - w) / 2.0;
                let y = self.layout.page_h - self.layout.margin_top * 0.5;
                s.push_str(&text_op(x, y, head, HEAD_SIZE));
            }
        }
        s
    }

    /// Serialize the pages and outline into the bytes of a PDF file.
    fn finish(mut self) -> Vec<u8> {
        if self.pages.is_empty() {
            self.start_page();
        }
        let n_pages = self.pages.len();
        let mut objects: ObjMap = BTreeMap::new();
        // 1 catalog, 2 pages, 3 outlines, then fonts, pages+contents, outline items.
        let mut next = 4u32;
        let font_ids = self.layout.fonts.objects(&mut next, &mut objects);
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
        let resources = format!(
            "/Resources << /Font << /F1 {} 0 R /F2 {} 0 R /F3 {} 0 R /F4 {} 0 R >> >>",
            font_ids[0], font_ids[1], font_ids[2], font_ids[3]
        );
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
            content.push_str(&self.decorations(i));
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
enum Align {
    Left,
    Center,
    /// Flush to a left edge inset by this many points (epigraph body).
    LeftInset(f32),
    /// Flush to a right edge inset by this many points (epigraph attribution).
    RightInset(f32),
}

/// A `BT…ET` text run showing one string at `(x, y)` in the regular face — used for
/// running heads and folios.
fn text_op(x: f32, y: f32, text: &str, size: f32) -> String {
    format!(
        "BT\n1 0 0 1 {} {} Tm\n/F1 {} Tf\n({}) Tj\nET\n",
        num(x),
        num(y),
        num(size),
        pdf_string(text)
    )
}

/// Split runs into wrap-able words, preserving each run's emphasis.
fn words_of(runs: &[(String, bool, bool)]) -> Vec<Word> {
    let mut words = Vec::new();
    for (text, bold, italic) in runs {
        for w in text.split(' ').filter(|w| !w.is_empty()) {
            words.push(Word {
                text: w.to_string(),
                bold: *bold,
                italic: *italic,
            });
        }
    }
    words
}

/// Greedy line-wrap: pack words up to `max_width` for the layout's fonts, indenting
/// only the first line. Each line is returned with its content width (excluding the
/// indent), so callers can align it without re-measuring.
fn wrap<'w>(
    layout: &Layout,
    words: &'w [Word],
    max_width: f32,
    first_indent: f32,
    size: f32,
) -> Vec<(Vec<&'w Word>, f32)> {
    let space = layout.measure(" ", size, false, false);
    let mut lines: Vec<(Vec<&Word>, f32)> = Vec::new();
    let mut line: Vec<&Word> = Vec::new();
    let mut content = 0.0; // width of the current line's words + inter-word spaces
    let mut indent = first_indent; // applied to the first line only
    for w in words {
        let ww = layout.measure(&w.text, size, w.bold, w.italic);
        let advance = if line.is_empty() { ww } else { space + ww };
        if !line.is_empty() && indent + content + advance > max_width {
            lines.push((std::mem::take(&mut line), content));
            content = ww;
            indent = 0.0;
            line.push(w);
        } else {
            content += advance;
            line.push(w);
        }
    }
    if !line.is_empty() {
        lines.push((line, content));
    }
    lines
}

/// A wrapped line as drawable segments: each word carries a trailing space except
/// the last, so `Tj` advances spell the gaps in each word's own font.
fn segments(line: &[&Word]) -> Vec<(String, bool, bool)> {
    let n = line.len();
    line.iter()
        .enumerate()
        .map(|(i, w)| {
            let text = if i + 1 < n {
                format!("{} ", w.text)
            } else {
                w.text.clone()
            };
            (text, w.bold, w.italic)
        })
        .collect()
}

/// Format a coordinate/size compactly (drop a trailing `.00`).
fn num(x: f32) -> String {
    if (x - x.round()).abs() < 0.005 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.2}")
    }
}

/// Flatten an outline subtree into PDF objects under `parent_id`, returning the
/// (first, last) child object ids. `count` accumulates the total item count.
fn flatten_outline(
    items: &[OutlineItem],
    parent_id: u32,
    next: &mut u32,
    page_obj: &[u32],
    page_h: f32,
    objects: &mut ObjMap,
    count: &mut usize,
) -> (Option<u32>, Option<u32>) {
    if items.is_empty() {
        return (None, None);
    }
    let ids: Vec<u32> = items
        .iter()
        .map(|_| {
            let id = *next;
            *next += 1;
            id
        })
        .collect();
    *count += items.len();
    for (k, item) in items.iter().enumerate() {
        let id = ids[k];
        let (first, last) =
            flatten_outline(&item.children, id, next, page_obj, page_h, objects, count);
        // Jump to the top of the destination page (its height varies by trim).
        let dest = format!(
            "[{} 0 R /XYZ null {} null]",
            page_obj[item.page],
            num(page_h)
        );
        let mut body = format!(
            "<< /Title ({}) /Parent {parent_id} 0 R /Dest {dest}",
            pdf_string(&item.title)
        );
        if k > 0 {
            body.push_str(&format!(" /Prev {} 0 R", ids[k - 1]));
        }
        if k + 1 < ids.len() {
            body.push_str(&format!(" /Next {} 0 R", ids[k + 1]));
        }
        if let (Some(f), Some(l)) = (first, last) {
            body.push_str(&format!(
                " /First {f} 0 R /Last {l} 0 R /Count {}",
                item.children.len()
            ));
        }
        body.push_str(" >>");
        objects.insert(id, body.into_bytes());
    }
    (ids.first().copied(), ids.last().copied())
}

/// Write the objects (in id order), the xref table, and the trailer.
fn serialize(objects: &ObjMap) -> Vec<u8> {
    let max_id = *objects.keys().max().expect("at least the catalog");
    let mut out: Vec<u8> = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0usize; (max_id + 1) as usize];
    for id in 1..=max_id {
        let body = objects.get(&id).expect("contiguous object ids");
        offsets[id as usize] = out.len();
        out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", max_id + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for id in 1..=max_id {
        out.extend_from_slice(format!("{:010} {:05} n \n", offsets[id as usize], 0).as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            max_id + 1
        )
        .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests;
