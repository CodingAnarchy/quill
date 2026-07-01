//! The [`Pdf`] builder's low-level text and drawing primitives: line flow and
//! pagination ([`Pdf::flow`]), positioned glyph runs ([`Pdf::draw_line`],
//! [`Pdf::draw_mono_line`], [`Pdf::glyph_block`]), centered single lines, and the
//! free helpers ([`mono_wrap`], [`num`]). The document-assembly methods (title page,
//! chapter, paragraph, …) and the public entry points stay in the parent module.

use super::font::{glyph_run, glyph_run_mono, OBLIQUE_SHEAR};
use super::layout::Layout;
use super::wrap::{segments, words_of, wrap_lines, Hyphenate, Seg, Word};
use super::{Align, FontStyle, Pdf, Run, BODY_LEADING, BODY_SIZE, INDENT, MONO_SIZE};

impl Pdf {
    /// A normal body paragraph: first-line indented, justified to both margins
    /// (#650), with `*`/`**` emphasis as italic/bold runs.
    pub(super) fn paragraph(&mut self, runs: &[Run]) {
        let words = words_of(runs, false);
        self.flow(
            &words,
            self.layout.text_w(),
            INDENT,
            BODY_SIZE,
            BODY_LEADING,
            Align::Justify,
        );
    }

    /// A single centered line (titles, headings, scene breaks) followed by a gap.
    /// `heading` routes the text through the heading font (chapter/book titles),
    /// leaving body-family centered lines (epigraphs, ornaments) unchanged (#638).
    pub(super) fn centered(
        &mut self,
        text: &str,
        bold: bool,
        italic: bool,
        heading: bool,
        size: f32,
        leading: f32,
    ) {
        let words = words_of(&[Run::new(text, bold, italic)], heading);
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
    pub(super) fn flow(
        &mut self,
        words: &[Word],
        max_width: f32,
        first_indent: f32,
        size: f32,
        leading: f32,
        align: Align,
    ) {
        let col = self.layout.text_w();
        // Hyphenate only justified body paragraphs (#678); centered/inset flows
        // (headings, epigraphs, ornaments) never break words.
        let hyphenate = if align == Align::Justify {
            Hyphenate::On
        } else {
            Hyphenate::Off
        };
        // The first line is narrowed by the indent; the rest run the full width.
        let lines = wrap_lines(&self.layout, words, size, hyphenate, |i| {
            if i == 0 {
                max_width - first_indent
            } else {
                max_width
            }
        });
        let last = lines.len().saturating_sub(1);
        let n = lines.len();
        // Widow/orphan control (#650): decide up front the line index (if any) at which
        // to force a page break so no single paragraph line is stranded at a page top
        // (widow) or bottom (orphan). `None` leaves natural `need` pagination.
        let force_break = self.widow_orphan_break(n, leading);
        for (i, (line, line_w)) in lines.into_iter().enumerate() {
            if force_break == Some(i) {
                // Pull this line (and the rest) onto a fresh page so neither the current
                // page's tail nor the next page's head is a lone paragraph line.
                self.start_page();
            } else {
                self.need(leading);
            }
            let left = self.left();
            let indent = if i == 0 { first_indent } else { 0.0 };
            // Justify every line but the last, and never a one-word line (nothing to
            // stretch) — the target width fills the column from the line's left edge
            // (#650). Everything else draws flush at its computed x.
            let target = max_width - indent;
            if align == Align::Justify && i != last && line.len() > 1 {
                self.draw_justified_line(left + indent, self.y, &line, size, target);
            } else {
                let x = match align {
                    Align::Left | Align::Justify => left + indent,
                    Align::Center => left + (col - line_w) / 2.0,
                    Align::LeftInset(pad) => left + pad,
                    Align::RightInset(pad) => left + col - pad - line_w,
                };
                let segs = segments(&line);
                self.draw_line(x, self.y, &segs, size);
            }
            self.y -= leading;
        }
    }

    /// Plan a paragraph's page break for widow/orphan control (#650). Given the line
    /// count `n` and the `leading`, return the line index at which to force a fresh page
    /// so no lone line is left behind — or `None` when the natural break is already safe
    /// (the whole paragraph fits, or it splits leaving ≥2 lines on each side).
    ///
    /// - **Orphan:** only one line fits on the current page but the paragraph has more —
    ///   move the whole paragraph (break at line 0) so its first line isn't stranded at
    ///   the page bottom.
    /// - **Widow:** the natural break would leave exactly the last line alone on the next
    ///   page — break one line earlier so two lines carry over together.
    pub(super) fn widow_orphan_break(&self, n: usize, leading: f32) -> Option<usize> {
        if n < 2 {
            return None; // a single line is never a widow/orphan
        }
        // How many of this paragraph's lines fit on the current page from `self.y`.
        let room = self.y - self.layout.text_bottom();
        let fit = if room < 0.0 {
            0
        } else {
            (room / leading).floor() as usize + 1
        };
        if fit >= n {
            return None; // the whole paragraph fits — no break needed
        }
        if fit <= 1 {
            // Only the first line would fit here (orphan): push the paragraph down —
            // unless we're already at the top of a fresh page (nothing to gain, and it
            // would leave a blank page for an over-tall paragraph).
            if self.y < self.layout.text_top() {
                return Some(0);
            }
            return None;
        }
        // A widow forms when the tail left for the next page is exactly one line; break
        // one line early so the last two lines travel together.
        if n - fit == 1 {
            return Some(fit - 1);
        }
        None
    }

    /// Draw one justified line: place each word so the row fills `target` points from
    /// `x`, spreading the slack evenly across the inter-word gaps (#650). The pen walks
    /// word to word — each word's measured width plus a stretched gap — so the last word
    /// lands flush at `x + target` within measurement tolerance. Word emphasis/heading
    /// faces are preserved (each word is placed as its own glyph run).
    pub(super) fn draw_justified_line(
        &mut self,
        x: f32,
        y: f32,
        line: &[Word],
        size: f32,
        target: f32,
    ) {
        let words_w: f32 = line
            .iter()
            .map(|w| self.layout.measure(&w.text, size, w.style(), w.heading))
            .sum();
        let gaps = (line.len() - 1) as f32;
        // The slack to spread over the gaps; guard against a word row already wider than
        // the target (falls back to a single normal-ish space) rather than overlapping.
        let gap = ((target - words_w) / gaps).max(0.0);
        let mut pen = x;
        let mut block = String::new();
        for w in line {
            block.push_str(&self.glyph_block(
                pen,
                y,
                size,
                std::iter::once((w.text.as_str(), w.style(), w.heading)),
            ));
            pen += self.layout.measure(&w.text, size, w.style(), w.heading) + gap;
        }
        self.cur().push_str(&block);
    }

    /// Emit one positioned line of styled segments into the current page's content.
    /// Each segment's text is resolved to glyph ids (body/heading face, or the
    /// fallback for glyphs it lacks) and shown as Identity-H hex runs.
    pub(super) fn draw_line(&mut self, x: f32, y: f32, segs: &[Seg], size: f32) {
        let block = self.glyph_block(
            x,
            y,
            size,
            segs.iter().map(|s| (s.text.as_str(), s.style, s.heading)),
        );
        self.cur().push_str(&block);
    }

    /// Emit one positioned line of monospace text into the current page's content,
    /// resolved through the fixed-width face (mono→fallback) and shown as Identity-H
    /// hex runs (#524).
    pub(super) fn draw_mono_line(&mut self, x: f32, y: f32, text: &str) {
        let mut s = format!("BT\n1 0 0 1 {} {} Tm\n", num(x), num(y));
        let mut cur_font = "";
        for (name, hex) in glyph_run_mono(&self.layout.fonts, text) {
            if name != cur_font {
                s.push_str(&format!("/{name} {} Tf\n", num(MONO_SIZE)));
                cur_font = name;
            }
            s.push_str(&format!("<{hex}> Tj\n"));
        }
        s.push_str("ET\n");
        self.cur().push_str(&s);
    }

    /// A `BT…ET` text block at `(x, y)`: each `(text, style, heading)` segment
    /// resolved to glyph-id hex runs (`/Fn` font switches between the body/heading
    /// faces and the fallback). A segment whose face is synthetically obliqued — an
    /// italic style with no true-italic file — is drawn with a sheared text matrix,
    /// the last-resort slope (#638); real italics and everything else stay upright
    /// in the matrix. The pen advances segment to segment via each run's measured
    /// width, so a re-set matrix (needed to change the shear) lands in the right
    /// place.
    pub(super) fn glyph_block<'a>(
        &self,
        x: f32,
        y: f32,
        size: f32,
        segments: impl Iterator<Item = (&'a str, FontStyle, bool)>,
    ) -> String {
        let fonts = &self.layout.fonts;
        let mut s = String::from("BT\n");
        let mut cur_font = "";
        let mut pen = x;
        let mut cur_shear = f32::NAN; // force the first Tm emission
        for (text, style, heading) in segments {
            let shear = if fonts.is_synthetic_oblique(style, heading) {
                OBLIQUE_SHEAR
            } else {
                0.0
            };
            // (Re)set the text matrix when the shear changes, positioning the pen at
            // the segment's start x. `Tm` is absolute, so this preserves placement.
            if shear != cur_shear {
                s.push_str(&format!(
                    "1 0 {} 1 {} {} Tm\n",
                    num(shear),
                    num(pen),
                    num(y)
                ));
                cur_shear = shear;
            }
            for (name, hex) in glyph_run(fonts, text, style, heading) {
                if name != cur_font {
                    s.push_str(&format!("/{name} {} Tf\n", num(size)));
                    cur_font = name;
                }
                s.push_str(&format!("<{hex}> Tj\n"));
            }
            pen += self.layout.measure(text, size, style, heading);
        }
        s.push_str("ET\n");
        s
    }
}

/// Wrap one preformatted line to the text column at character boundaries (#524).
/// The face is fixed-width, so breaking on whole characters keeps columns aligned;
/// a line that fits returns unchanged. Leading whitespace (indentation) is preserved
/// on the first piece, and a continuation that itself overflows is broken again, so
/// nothing runs off the page edge. An empty line yields one empty piece.
pub(super) fn mono_wrap(layout: &Layout, line: &str, width: f32) -> Vec<String> {
    if layout.measure_mono(line, MONO_SIZE) <= width {
        return vec![line.to_string()];
    }
    let mut pieces = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0.0;
    for c in line.chars() {
        let cw = layout.measure_mono(&c.to_string(), MONO_SIZE);
        if !cur.is_empty() && cur_w + cw > width {
            pieces.push(std::mem::take(&mut cur));
            cur_w = 0.0;
        }
        cur.push(c);
        cur_w += cw;
    }
    if !cur.is_empty() {
        pieces.push(cur);
    }
    pieces
}

/// Format a coordinate/size compactly (drop a trailing `.00`).
pub(super) fn num(x: f32) -> String {
    if (x - x.round()).abs() < 0.005 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.2}")
    }
}
