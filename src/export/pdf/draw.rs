//! The [`Pdf`] builder's low-level text and drawing primitives: line flow and
//! pagination ([`Pdf::flow`]), positioned glyph runs ([`Pdf::draw_line`],
//! [`Pdf::draw_mono_line`], [`Pdf::glyph_block`]), centered single lines, and the
//! free helpers ([`mono_wrap`], [`num`]). The document-assembly methods (title page,
//! chapter, paragraph, …) and the public entry points stay in the parent module.

use super::font::{glyph_run, glyph_run_mono, OBLIQUE_SHEAR};
use super::layout::Layout;
use super::wrap::{segments, words_of, wrap_lines, Seg, Word};
use super::{Align, FontStyle, Pdf, Run, BODY_LEADING, BODY_SIZE, INDENT, MONO_SIZE};

impl Pdf {
    /// A normal body paragraph: first-line indented, left-aligned, with `*`/`**`
    /// emphasis as italic/bold runs.
    pub(super) fn paragraph(&mut self, runs: &[Run]) {
        let words = words_of(runs, false);
        self.flow(
            &words,
            self.layout.text_w(),
            INDENT,
            BODY_SIZE,
            BODY_LEADING,
            Align::Left,
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
