//! The [`Pdf`] builder's page bookkeeping: the per-page [`PageMeta`] and the
//! non-font [`StyleBits`], plus the methods that start/open pages, track the body
//! boundary, number folios ([`roman`] front matter / Arabic body, #651), place image
//! ornaments, and draw the per-page running heads + folios ([`Pdf::decorations`]).
//! Split from `mod.rs` to keep both under the size convention (#927); the document
//! assembly and the public entry points stay in the parent module.

use std::fmt::Write;

use super::draw::num;
use super::images::PdfImage;
use super::layout::Layout;
use super::{Align, ExportStyle, FontStyle, ImageRole, Pdf, BODY_SIZE, HEAD_SIZE};

/// What a page is, for decoration: a section opener and blank pages get no running
/// head; blank pages (inserted for recto starts) get no folio either.
pub(super) struct PageMeta {
    pub(super) verso: String,
    pub(super) recto: String,
    pub(super) opener: bool,
    pub(super) blank: bool,
}

/// The lowercase roman numeral for `n` (1 → "i", 4 → "iv", 2026 → "mmxxvi"). Used to
/// number front-matter folios by convention (#651); a zero yields the empty string.
pub(super) fn roman(mut n: usize) -> String {
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
pub(super) struct StyleBits {
    pub(super) scene_break: String,
    pub(super) heading_align: Align,
    pub(super) drop_cap: bool,
    pub(super) title_ornament: Option<String>,
    pub(super) part_ornament: Option<String>,
}

impl StyleBits {
    pub(super) fn from_style(style: &ExportStyle) -> Self {
        Self {
            scene_break: style.scene_break().to_string(),
            // Headings aren't justified; map justify/left to flush-left.
            heading_align: match style.heading_align() {
                Some("left" | "justify") => Align::Left,
                Some("right") => Align::RightInset(0.0),
                _ => Align::Center,
            },
            drop_cap: style.drop_cap(),
            title_ornament: style.title_ornament.clone(),
            part_ornament: style.part_ornament.clone(),
        }
    }
}

impl Pdf {
    /// Build a fresh builder over `layout`/`bits`/`images` with an empty page list.
    pub(super) const fn new(layout: Layout, bits: StyleBits, images: Vec<PdfImage>) -> Self {
        Self {
            layout,
            bits,
            images,
            pages: Vec::new(),
            meta: Vec::new(),
            y: 0.0,
            outline: Vec::new(),
            head_verso: String::new(),
            head_recto: String::new(),
            doc_title: String::new(),
            doc_author: String::new(),
            body_start: usize::MAX,
        }
    }

    /// Record the document title/author for the `/Info` dictionary (#692), from the
    /// manifest metadata. Empty fields are omitted from `/Info`.
    pub(super) fn set_info(&mut self, title: &str, author: &str) {
        self.doc_title = title.to_string();
        self.doc_author = author.to_string();
    }

    /// Mark the body as beginning at the next page to be started (#651): everything
    /// laid out so far (cover/title/copyright/Contents) is front matter, numbered in
    /// roman; from here folios run Arabic from 1. Called once, before the body.
    pub(super) const fn begin_body(&mut self) {
        self.body_start = self.pages.len();
    }

    /// The printed folio label for page `i` under the roman-front/Arabic-body scheme
    /// (#651), or `None` when the page carries no folio (blank/cover pages, and the
    /// unnumbered opening title page). Front-matter pages are counted in roman from the
    /// first numbered front page; body pages in Arabic from 1. Both count only
    /// non-blank pages, so inserted blank versos don't consume a folio.
    pub(super) fn folio_label(&self, i: usize) -> Option<String> {
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
    pub(super) fn image(&self, role: ImageRole) -> Option<&PdfImage> {
        self.images.iter().find(|i| i.role == role)
    }

    /// Place an image ornament for `role` centered in the text column, scaled to fit
    /// `max_w`×`max_h` (aspect preserved), and advance past it. No-op if absent.
    pub(super) fn place_image(&mut self, role: ImageRole, max_w: f32, max_h: f32, gap_after: f32) {
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
        let _ = writeln!(
            self.cur(),
            "q {} 0 0 {} {} {} cm /{name} Do Q",
            num(dw),
            num(dh),
            num(x),
            num(bottom)
        );
        self.y -= dh + gap_after;
    }

    pub(super) fn cur(&mut self) -> &mut String {
        // Drawing always follows a `start_page`, but guard the empty case by
        // starting one lazily (as `finish` does) rather than risk a panic. The
        // index it returns is always valid, so no unwrap on the access.
        let idx = if self.pages.is_empty() {
            self.start_page()
        } else {
            self.pages.len() - 1
        };
        &mut self.pages[idx]
    }

    pub(super) const fn page_idx(&self) -> usize {
        self.pages.len() - 1
    }

    pub(super) fn left(&self) -> f32 {
        self.layout.left(self.page_idx())
    }

    /// Set the running-head text used from here on (verso/recto). Recorded per page.
    pub(super) fn set_heads(&mut self, verso: &str, recto: &str) {
        self.head_verso = verso.to_string();
        self.head_recto = recto.to_string();
    }

    /// Begin a fresh page, returning its index; the baseline returns to the top of
    /// the text area. `pages` and `meta` grow in lockstep, so the returned index is
    /// valid in both for the caller to annotate its `PageMeta`.
    pub(super) fn start_page(&mut self) -> usize {
        self.pages.push(String::new());
        self.meta.push(PageMeta {
            verso: self.head_verso.clone(),
            recto: self.head_recto.clone(),
            opener: false,
            blank: false,
        });
        self.y = self.layout.text_top();
        self.pages.len() - 1
    }

    /// Start a section's first page, returning its index. In the print profile this
    /// lands on a recto, inserting a blank verso when the next page would be a verso.
    pub(super) fn open_section(&mut self) -> usize {
        if self.layout.recto_starts && self.pages.len() % 2 == 1 {
            let blank = self.start_page();
            self.meta[blank].blank = true;
        }
        let idx = self.start_page();
        self.meta[idx].opener = true;
        idx
    }

    /// Whether a line of the given `leading` still fits above the bottom margin from
    /// the current baseline `y` — the single break threshold. [`need`] drops to a new
    /// page when it doesn't, and [`lines_that_fit`] (the widow/orphan fit count)
    /// derives from this same rule so the two never disagree at a page boundary.
    ///
    /// [`need`]: Pdf::need
    /// [`lines_that_fit`]: Pdf::lines_that_fit
    pub(super) fn fits(y: f32, leading: f32, bottom: f32) -> bool {
        y - leading >= bottom
    }

    /// How many successive `leading`-tall lines fit above `bottom` starting from
    /// baseline `y`. The k-th line (0-based) lands at `y - k*leading` and counts only
    /// while [`fits`] holds, so this and [`need`] share one threshold — the widow/orphan
    /// guard's fit count can't be off by a line versus where the break actually lands.
    ///
    /// [`fits`]: Pdf::fits
    /// [`need`]: Pdf::need
    pub(super) fn lines_that_fit(y: f32, leading: f32, bottom: f32) -> usize {
        let mut fit = 0usize;
        while Self::fits((fit as f32).mul_add(-leading, y), leading, bottom) {
            fit += 1;
        }
        fit
    }

    /// Drop to a new page if the next `height` wouldn't fit above the bottom margin.
    pub(super) fn need(&mut self, height: f32) {
        if !Self::fits(self.y, height, self.layout.text_bottom()) {
            self.start_page();
        }
    }

    /// The running head + folio for page `i` (the screen profile centers the folio;
    /// print places it at the outer corner and adds a running head off opener pages).
    pub(super) fn decorations(&self, i: usize) -> String {
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
    pub(super) fn text_op(&self, x: f32, y: f32, text: &str, size: f32) -> String {
        self.glyph_block(
            x,
            y,
            size,
            std::iter::once((text, FontStyle::REGULAR, false)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Pdf;

    const LEADING: f32 = 12.0;
    const BOTTOM: f32 = 100.0;

    /// The widow/orphan fit count and the actual break rule (`fits`, which drives
    /// `need`) derive from ONE threshold, so they never disagree: for every baseline
    /// `y`, the number of lines `lines_that_fit` reports is exactly the run of lines
    /// that `fits`/`need` would actually place. This closes the #1118 off-by-one where
    /// the fit count used `room >= 0` while `need` placed at `room >= leading`.
    #[test]
    fn fit_count_agrees_with_break_rule() {
        // Sweep baselines from below the margin up past several lines' worth of room.
        let mut y = BOTTOM - LEADING;
        while y <= 5.0f32.mul_add(LEADING, BOTTOM) {
            let counted = Pdf::lines_that_fit(y, LEADING, BOTTOM);
            // Independently count how many lines `fits` (== the break rule) admits.
            let mut placed = 0usize;
            while Pdf::fits((placed as f32).mul_add(-LEADING, y), LEADING, BOTTOM) {
                placed += 1;
            }
            assert_eq!(
                counted, placed,
                "fit count and break rule must agree at y={y}"
            );
            y += LEADING / 3.0; // step in thirds to probe the boundary from both sides
        }
    }

    /// The boundary the old code got wrong: at exactly one leading of room
    /// (`y == bottom + leading`) precisely one line fits — `need` would place it
    /// (`y - leading == bottom`, and `>=` admits it) and no more. The former
    /// `room >= 0` fit test claimed two.
    #[test]
    fn exact_boundary_fits_one_line() {
        let y = BOTTOM + LEADING;
        assert!(
            Pdf::fits(y, LEADING, BOTTOM),
            "the line at the margin is placed"
        );
        assert!(
            !Pdf::fits(y - LEADING, LEADING, BOTTOM),
            "the next line would fall below the margin"
        );
        assert_eq!(Pdf::lines_that_fit(y, LEADING, BOTTOM), 1);
    }

    /// A baseline that leaves no room for even one line reports zero (no underflow).
    #[test]
    fn no_room_fits_zero() {
        assert_eq!(Pdf::lines_that_fit(BOTTOM, LEADING, BOTTOM), 0);
        assert_eq!(Pdf::lines_that_fit(BOTTOM - LEADING, LEADING, BOTTOM), 0);
    }
}
