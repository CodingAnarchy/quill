//! PDF export (#463). A PDF is hand-written here the same way the `.docx` OOXML
//! and `.epub` XHTML are — no PDF library, just the file format: a header, a body
//! of numbered objects (catalog, page tree, fonts, per-page content streams, and a
//! bookmark outline), an xref table, and a trailer. We lay the book out ourselves —
//! US-Letter pages, 1" margins, each chapter on a fresh page, a title page, page
//! numbers, and a navigable outline — using the standard-14 Times fonts every
//! viewer ships, so nothing needs embedding. Consumes the shared [`Document`].
//!
//! Scope (per #463): a clean, readable, paginated PDF. The standard fonts cover
//! WinAnsi (Latin-1 plus the common typographic punctuation — em/en dashes, smart
//! quotes, ellipsis); characters outside that set render as `?`. Designed
//! typography (font choice, ornaments, art) is the styling epic, not this writer;
//! print-shop trim/gutters are the sibling print-interior ticket.

use std::collections::BTreeMap;

use super::{classify_block, inline_runs, Document, Prose, SCENE_BREAK};
use crate::frontmatter::Epigraph;

// US-Letter page, 1-inch margins, in PDF points (1/72").
const PAGE_W: f32 = 612.0;
const PAGE_H: f32 = 792.0;
const MARGIN: f32 = 72.0;
const TEXT_W: f32 = PAGE_W - 2.0 * MARGIN; // 468
const TOP: f32 = PAGE_H - MARGIN; // 720
const BOTTOM: f32 = MARGIN; // 72

// Body type and the heading/title sizes layered on top.
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

// The PDF object ids of the four fonts (assigned right after the fixed catalog /
// pages / outlines objects), mapped to the `/F1`…`/F4` resource names.
const FONT_BASES: [&str; 4] = [
    "Times-Roman",
    "Times-Bold",
    "Times-Italic",
    "Times-BoldItalic",
];

/// Compile a [`Document`] to the bytes of a `.pdf`: a title page, then each
/// front/part/chapter/back section in reading order, with a bookmark outline.
pub fn to_pdf(doc: &Document) -> std::io::Result<Vec<u8>> {
    let mut pdf = Pdf::new();
    pdf.title_page(
        doc.series.as_deref(),
        &doc.title,
        doc.subtitle.as_deref(),
        doc.author.as_deref(),
        doc.epigraph.as_ref(),
    );
    let outline = book_outline(&mut pdf, doc);
    pdf.outline = outline;
    Ok(pdf.finish())
}

/// Compile a whole series (one book per [`Document`], in order) to one `.pdf`
/// (#332): a series title page, then each book behind a divider page with its
/// sections nested under it in the outline.
pub fn to_pdf_series(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
) -> std::io::Result<Vec<u8>> {
    let mut pdf = Pdf::new();
    let title = series.unwrap_or("Series");
    pdf.title_page(None, title, None, author, None);
    let mut outline = Vec::new();
    for book in books {
        let page = pdf.divider_page(&book.title, PART_SIZE);
        let mut item = OutlineItem::new(book.title.clone(), page);
        item.children = book_outline(&mut pdf, book);
        outline.push(item);
    }
    pdf.outline = outline;
    Ok(pdf.finish())
}

/// Lay out one book's sections (front, parts, back) onto `pdf` and return the
/// outline subtree for them: front/back chapters and an unnamed part's chapters
/// sit at this level; a named part nests its chapters beneath it.
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

/// Accumulates per-page content streams as the book is laid out, tracking the
/// current baseline, then [`Pdf::finish`] serializes the objects, xref, and
/// trailer.
struct Pdf {
    pages: Vec<String>,
    y: f32,
    outline: Vec<OutlineItem>,
}

impl Pdf {
    fn new() -> Self {
        Pdf {
            pages: Vec::new(),
            y: TOP,
            outline: Vec::new(),
        }
    }

    fn cur(&mut self) -> &mut String {
        self.pages.last_mut().expect("a page was started")
    }

    fn page_idx(&self) -> usize {
        self.pages.len() - 1
    }

    /// Begin a fresh page; the baseline returns to the top margin.
    fn start_page(&mut self) {
        self.pages.push(String::new());
        self.y = TOP;
    }

    /// Drop to a new page if the next `height` wouldn't fit above the bottom margin.
    fn need(&mut self, height: f32) {
        if self.y - height < BOTTOM {
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
        self.start_page();
        self.y = PAGE_H * 0.62;
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
        self.start_page();
        let page = self.page_idx();
        self.y = PAGE_H * 0.5;
        self.centered(title, true, false, size, size * 1.4);
        page
    }

    /// One chapter on a fresh page: a centered title, an optional epigraph, then the
    /// body. Returns the page it starts on (for its bookmark).
    fn chapter(&mut self, ch: &super::Chapter) -> usize {
        self.start_page();
        let page = self.page_idx();
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
                self.flow(&words, TEXT_W, 0.0, BODY_SIZE, BODY_LEADING, Align::Center);
            }
            Some(Prose::Paragraph(text)) => {
                let words = words_of(&inline_runs(&text));
                self.flow(&words, TEXT_W, INDENT, BODY_SIZE, BODY_LEADING, Align::Left);
            }
        }
    }

    /// An epigraph: the lines set apart (inset, italic, single-spaced) with the
    /// attribution on its own line.
    fn epigraph(&mut self, e: &Epigraph) {
        let width = TEXT_W - 2.0 * EPIGRAPH_INSET;
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
        self.flow(&words, TEXT_W, 0.0, size, leading, Align::Center);
        // `flow` already advanced by `leading`; nothing more to do.
    }

    /// Wrap `words` into `max_width` and emit each line at `size`/`leading`,
    /// paginating as needed. `first_indent` indents only the first line (left
    /// alignment); `align` places each line within the column.
    fn flow(
        &mut self,
        words: &[Word],
        max_width: f32,
        first_indent: f32,
        size: f32,
        leading: f32,
        align: Align,
    ) {
        for (i, (line, line_w)) in wrap(words, max_width, first_indent, size)
            .into_iter()
            .enumerate()
        {
            self.need(leading);
            let indent = if i == 0 { first_indent } else { 0.0 };
            let x = match align {
                Align::Left => MARGIN + indent,
                Align::Center => MARGIN + (TEXT_W - line_w) / 2.0,
                Align::LeftInset(pad) => MARGIN + pad,
                Align::RightInset(pad) => MARGIN + TEXT_W - pad - line_w,
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

    /// Serialize the accumulated pages and outline into the bytes of a PDF file.
    fn finish(mut self) -> Vec<u8> {
        if self.pages.is_empty() {
            self.start_page();
        }
        let n_pages = self.pages.len();
        // Object ids: 1 catalog, 2 pages, 3 outlines, 4–7 fonts, then per page a
        // page dict + a content stream, then the flattened outline items.
        let font_ids: [u32; 4] = [4, 5, 6, 7];
        let mut next = 8u32;
        let mut page_obj = Vec::with_capacity(n_pages);
        let mut content_obj = Vec::with_capacity(n_pages);
        for _ in 0..n_pages {
            page_obj.push(next);
            next += 1;
            content_obj.push(next);
            next += 1;
        }
        let outline = std::mem::take(&mut self.outline);
        let mut objects: BTreeMap<u32, String> = BTreeMap::new();

        // Outline items (need page object ids to resolve destinations).
        let mut item_count = 0usize;
        let (first, last) = flatten_outline(
            &outline,
            3,
            &mut next,
            &page_obj,
            &mut objects,
            &mut item_count,
        );

        // Fonts.
        for (i, base) in FONT_BASES.iter().enumerate() {
            objects.insert(
                font_ids[i],
                format!("<< /Type /Font /Subtype /Type1 /BaseFont /{base} /Encoding /WinAnsiEncoding >>"),
            );
        }
        // Page tree (MediaBox + font resources inherited by every page).
        let kids: String = page_obj.iter().map(|id| format!("{id} 0 R ")).collect();
        let resources = format!(
            "/Resources << /Font << /F1 {} 0 R /F2 {} 0 R /F3 {} 0 R /F4 {} 0 R >> >>",
            font_ids[0], font_ids[1], font_ids[2], font_ids[3]
        );
        objects.insert(
            2,
            format!("<< /Type /Pages /Kids [ {kids}] /Count {n_pages} /MediaBox [0 0 {} {}] {resources} >>", num(PAGE_W), num(PAGE_H)),
        );
        // Pages and their content streams (page numbers drawn on content pages).
        for i in 0..n_pages {
            objects.insert(
                page_obj[i],
                format!(
                    "<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>",
                    content_obj[i]
                ),
            );
            let mut content = std::mem::take(&mut self.pages[i]);
            if i > 0 {
                content.push_str(&page_number(i));
            }
            objects.insert(
                content_obj[i],
                format!(
                    "<< /Length {} >>\nstream\n{content}\nendstream",
                    content.len()
                ),
            );
        }
        // Outline root and catalog.
        let outlines = if item_count == 0 {
            "<< /Type /Outlines /Count 0 >>".to_string()
        } else {
            format!(
                "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {item_count} >>",
                first.unwrap(),
                last.unwrap()
            )
        };
        objects.insert(3, outlines);
        objects.insert(
            1,
            "<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R /PageMode /UseOutlines >>".to_string(),
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

/// The page-number line: the 1-based content-page number, centered at the bottom
/// margin. The title page (index 0) is unnumbered.
fn page_number(page_idx: usize) -> String {
    let label = page_idx.to_string();
    let w = text_width(&label, BODY_SIZE);
    let x = MARGIN + (TEXT_W - w) / 2.0;
    format!(
        "BT\n1 0 0 1 {} {} Tm\n/F1 {} Tf\n({label}) Tj\nET\n",
        num(x),
        num(BOTTOM * 0.5),
        num(BODY_SIZE)
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

/// Greedy line-wrap: pack words up to `max_width`, indenting only the first line.
/// Each line is returned with its rendered content width (excluding the indent), so
/// callers can align it without re-measuring.
fn wrap(words: &[Word], max_width: f32, first_indent: f32, size: f32) -> Vec<(Vec<&Word>, f32)> {
    let space = text_width(" ", size);
    let mut lines: Vec<(Vec<&Word>, f32)> = Vec::new();
    let mut line: Vec<&Word> = Vec::new();
    let mut content = 0.0; // width of the current line's words + inter-word spaces
    let mut indent = first_indent; // applied to the first line only
    for w in words {
        let ww = text_width(&w.text, size);
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

/// The `/F1`…`/F4` resource name for an emphasis combination.
fn font_name(bold: bool, italic: bool) -> &'static str {
    match (bold, italic) {
        (false, false) => "F1",
        (true, false) => "F2",
        (false, true) => "F3",
        (true, true) => "F4",
    }
}

/// Format a coordinate/size compactly (drop a trailing `.00`).
fn num(x: f32) -> String {
    if (x - x.round()).abs() < 0.005 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.2}")
    }
}

/// Map a char to its WinAnsi byte, or `None` if the standard fonts can't show it.
/// ASCII and Latin-1 map directly; the common typographic punctuation maps into the
/// `0x80–0x9F` WinAnsi slots.
fn winansi(c: char) -> Option<u8> {
    let cp = c as u32;
    Some(match cp {
        0x20..=0x7E | 0xA0..=0xFF => cp as u8,
        0x2018 => 0x91,
        0x2019 => 0x92,
        0x201A => 0x82,
        0x201C => 0x93,
        0x201D => 0x94,
        0x201E => 0x84,
        0x2013 => 0x96,
        0x2014 => 0x97,
        0x2026 => 0x85,
        0x2022 => 0x95,
        0x2020 => 0x86,
        0x2021 => 0x87,
        0x2039 => 0x8B,
        0x203A => 0x9B,
        0x0152 => 0x8C,
        0x0153 => 0x9C,
        0x0160 => 0x8A,
        0x0161 => 0x9A,
        0x0178 => 0x9F,
        0x017D => 0x8E,
        0x017E => 0x9E,
        0x0192 => 0x83,
        0x02C6 => 0x88,
        0x02DC => 0x98,
        0x2122 => 0x99,
        0x2030 => 0x89,
        _ => return None,
    })
}

/// Encode text as a PDF literal string body (between the parens): WinAnsi bytes,
/// with `(`, `)`, `\` escaped and non-ASCII bytes written as `\ddd` octal so the
/// stream stays ASCII. Unsupported characters become `?`.
fn pdf_string(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match winansi(c) {
            None => out.push('?'),
            Some(b'(') => out.push_str("\\("),
            Some(b')') => out.push_str("\\)"),
            Some(b'\\') => out.push_str("\\\\"),
            Some(b @ 0x20..=0x7E) => out.push(b as char),
            Some(b) => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out
}

/// The width of `text` at `size`, in points, using the Times-Roman metrics for
/// every face. The body is Roman (so wrapping is exact there); bold/italic spans
/// measure a hair narrow, which only nudges the occasional emphasis word — well
/// within the right margin.
fn text_width(text: &str, size: f32) -> f32 {
    text.chars().map(char_width).sum::<f32>() / 1000.0 * size
}

/// One character's Times-Roman advance width in 1000-em units.
fn char_width(c: char) -> f32 {
    let b = winansi(c).unwrap_or(b'?');
    let w = if (0x20..=0x7E).contains(&b) {
        TIMES_ASCII[(b - 0x20) as usize]
    } else {
        match b {
            0x97 => 1000, // em dash
            0x96 => 500,  // en dash
            0x85 => 1000, // ellipsis
            0x91 | 0x92 => 333,
            0x93 | 0x94 => 444,
            0x95 => 350, // bullet
            0x82 => 333,
            0x84 => 444,
            0x86 | 0x87 => 500,
            _ => 500, // accented Latin & misc — close enough for wrapping
        }
    };
    w as f32
}

/// Times-Roman advance widths (1000-em units) for ASCII 0x20–0x7E, from the Adobe
/// Core-14 metrics.
const TIMES_ASCII: [u16; 95] = [
    250, 333, 408, 500, 500, 833, 778, 180, 333, 333, 500, 564, 250, 333, 250,
    278, // 0x20–0x2F
    500, 500, 500, 500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564,
    444, // 0x30–0x3F
    921, 722, 667, 667, 722, 611, 556, 722, 722, 333, 389, 722, 611, 889, 722,
    722, // 0x40–0x4F
    556, 722, 667, 556, 611, 722, 722, 944, 722, 722, 611, 333, 278, 333, 469,
    500, // 0x50–0x5F
    333, 444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500, 278, 778, 500,
    500, // 0x60–0x6F
    500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444, 480, 200, 480, 541, // 0x70–0x7E
];

/// Flatten an outline subtree into PDF objects under `parent_id`, returning the
/// (first, last) child object ids. `count` accumulates the total number of items so
/// the root's `/Count` is right.
fn flatten_outline(
    items: &[OutlineItem],
    parent_id: u32,
    next: &mut u32,
    page_obj: &[u32],
    objects: &mut BTreeMap<u32, String>,
    count: &mut usize,
) -> (Option<u32>, Option<u32>) {
    if items.is_empty() {
        return (None, None);
    }
    // Assign this level's ids first so siblings can reference each other.
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
        let (first, last) = flatten_outline(&item.children, id, next, page_obj, objects, count);
        let dest = format!(
            "[{} 0 R /XYZ null {} null]",
            page_obj[item.page],
            num(PAGE_H)
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
        objects.insert(id, body);
    }
    (ids.first().copied(), ids.last().copied())
}

/// Write the objects (in id order), the xref table, and the trailer.
fn serialize(objects: &BTreeMap<u32, String>) -> Vec<u8> {
    let max_id = *objects.keys().max().expect("at least the catalog");
    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = vec![0usize; (max_id + 1) as usize];
    for id in 1..=max_id {
        let body = objects.get(&id).expect("contiguous object ids");
        offsets[id as usize] = out.len();
        out.push_str(&format!("{id} 0 obj\n{body}\nendobj\n"));
    }
    let xref = out.len();
    out.push_str(&format!("xref\n0 {}\n", max_id + 1));
    out.push_str("0000000000 65535 f \n");
    for id in 1..=max_id {
        out.push_str(&format!("{:010} {:05} n \n", offsets[id as usize], 0));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        max_id + 1
    ));
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Chapter, Part};

    fn ep(text: &str, by: Option<&str>) -> Epigraph {
        Epigraph {
            text: text.into(),
            attribution: by.map(str::to_string),
        }
    }

    fn ch(title: &str, epigraph: Option<Epigraph>, body: &str) -> Chapter {
        Chapter {
            title: title.into(),
            epigraph,
            body: body.into(),
        }
    }

    /// A book exercising every section kind: front matter, a named part with one
    /// chapter (epigraph + emphasis + a scene break + a sub-heading), and back
    /// matter.
    fn doc() -> Document {
        Document {
            title: "Red State Earth".into(),
            subtitle: Some("a novel".into()),
            author: Some("A. Writer".into()),
            series: Some("The Cycle".into()),
            epigraph: Some(ep("In the beginning.", Some("Anon"))),
            front: vec![ch("Prologue", None, "Before it all.")],
            parts: vec![Part {
                name: Some("Part One".into()),
                chapters: vec![ch(
                    "The Find",
                    Some(ep("A single line.", None)),
                    "The locket *gleamed* and felt **heavy**.\n\n## A turn\n\nScene one.\n\n---\n\nScene two.",
                )],
            }],
            back: vec![ch("Afterword", None, "After it all.")],
        }
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// Reconstruct the visible text by concatenating every `(...) Tj` show
    /// operator's body (per-word, with the trailing spaces they carry).
    fn shown(bytes: &[u8]) -> String {
        let s = text(bytes);
        let b = s.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] != b'(' {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            let mut body = String::new();
            while j < b.len() && b[j] != b')' {
                if b[j] == b'\\' {
                    j += 2; // skip the escape (octal/`\(`/`\)`/`\\`)
                    continue;
                }
                body.push(b[j] as char);
                j += 1;
            }
            if s[j..].starts_with(") Tj") {
                out.push_str(&body);
            }
            i = j + 1;
        }
        out
    }

    fn count(s: &str, needle: &str) -> usize {
        s.matches(needle).count()
    }

    #[test]
    fn is_a_well_formed_pdf_with_a_locatable_xref() {
        let bytes = to_pdf(&doc()).unwrap();
        let s = text(&bytes);
        assert!(s.starts_with("%PDF-1.7"), "header");
        assert!(s.trim_end().ends_with("%%EOF"), "trailer marker");
        assert!(s.contains("/Type /Catalog") && s.contains("/Root 1 0 R"));
        assert!(s.contains("/Type /Pages") && s.contains("trailer"));
        // startxref points exactly at the xref table.
        let sx = s.rfind("startxref").expect("startxref");
        let off: usize = s[sx + "startxref".len()..]
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(&s[off..off + 4], "xref", "startxref locates the table");
    }

    #[test]
    fn embeds_the_four_times_faces_with_winansi() {
        let s = text(&to_pdf(&doc()).unwrap());
        for base in FONT_BASES {
            assert!(s.contains(&format!("/BaseFont /{base}")), "{base} declared");
        }
        assert_eq!(
            count(&s, "/Encoding /WinAnsiEncoding"),
            4,
            "every face WinAnsi-encoded"
        );
    }

    #[test]
    fn carries_metadata_structure_and_emphasis() {
        let bytes = to_pdf(&doc()).unwrap();
        let visible = shown(&bytes);
        // Title page + every section title reach the page text.
        for piece in [
            "Red State Earth",
            "a novel",
            "by A. Writer",
            "Prologue",
            "Part One",
            "The Find",
            "Afterword",
        ] {
            assert!(visible.contains(piece), "{piece:?} missing from: {visible}");
        }
        // Emphasis became real font switches, not literal markers.
        let raw = text(&bytes);
        assert!(
            raw.contains("/F2 ") && raw.contains("/F3 "),
            "bold + italic faces used"
        );
        assert!(!visible.contains('*'), "no literal asterisks: {visible}");
        // A scene break renders the divider glyph, not the rule.
        assert!(
            visible.contains(SCENE_BREAK) && !visible.contains("---"),
            "scene divider: {visible}"
        );
    }

    #[test]
    fn each_section_starts_on_its_own_page() {
        let s = text(&to_pdf(&doc()).unwrap());
        // Page dicts (not the `/Type /Pages` tree): title + prologue + part page +
        // chapter + afterword.
        assert_eq!(
            count(&s, "/Type /Page /Parent"),
            5,
            "one page per section + title"
        );
    }

    #[test]
    fn builds_a_nested_bookmark_outline() {
        let s = text(&to_pdf(&doc()).unwrap());
        assert!(
            s.contains("/PageMode /UseOutlines"),
            "opens with the outline"
        );
        for title in [
            "/Title (Prologue)",
            "/Title (Part One)",
            "/Title (The Find)",
            "/Title (Afterword)",
        ] {
            assert!(s.contains(title), "{title} bookmarked");
        }
        // "The Find" nests under "Part One": the part item has a child.
        let at = s.find("/Title (Part One)").unwrap();
        let item = &s[at..at + 200.min(s.len() - at)];
        assert!(
            item.contains("/First ") && item.contains("/Count 1"),
            "part nests its chapter: {item}"
        );
    }

    #[test]
    fn winansi_punctuation_encodes_and_unsupported_glyphs_degrade() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![ch(
                    "C",
                    None,
                    "She said \u{201C}go\u{201D}\u{2014}\u{03A9}.",
                )],
            }],
            back: vec![],
        };
        let bytes = to_pdf(&d).unwrap();
        let s = text(&bytes);
        // Smart quotes and the em dash become their WinAnsi octal escapes.
        assert!(
            s.contains("\\223") && s.contains("\\224") && s.contains("\\227"),
            "typographic punctuation as WinAnsi octal"
        );
        // The Greek capital omega isn't in the standard fonts → placeholder.
        assert!(
            shown(&bytes).contains('?'),
            "unsupported glyph degrades to ?"
        );
    }

    #[test]
    fn a_series_carries_every_book_in_order_under_the_series_title() {
        let book = |title: &str| Document {
            title: title.into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![ch("Ch", None, "Prose.")],
            }],
            back: vec![],
        };
        let bytes = to_pdf_series(
            &[book("Book One"), book("Book Two")],
            Some("The Saga"),
            Some("A. Writer"),
        )
        .unwrap();
        let visible = shown(&bytes);
        assert!(
            visible.contains("The Saga") && visible.contains("by A. Writer"),
            "series title page"
        );
        let one = visible.find("Book One").expect("book one");
        let two = visible.find("Book Two").expect("book two");
        assert!(one < two, "books in reading order");
        // Each book is a top-level bookmark.
        let s = text(&bytes);
        assert!(
            s.contains("/Title (Book One)") && s.contains("/Title (Book Two)"),
            "books bookmarked"
        );
    }
}
