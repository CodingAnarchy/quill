//! The [`Pdf`] builder's low-level text and drawing primitives: line flow and
//! pagination ([`Pdf::flow`]), positioned glyph runs ([`Pdf::draw_line`],
//! [`Pdf::draw_mono_line`], [`Pdf::glyph_block`]), centered single lines, and the
//! free helpers ([`mono_wrap`], [`num`]). The document-assembly methods (title page,
//! chapter, paragraph, …) and the public entry points stay in the parent module.

use super::font::{glyph_run, glyph_run_mono};
use super::layout::Layout;
use super::wrap::{segments, words_of, wrap_lines, Word};
use super::Run;
use super::{Align, Pdf, BODY_LEADING, BODY_SIZE, INDENT, MONO_SIZE};

impl Pdf {
    /// A normal body paragraph: first-line indented, left-aligned, with `*`/`**`
    /// emphasis as italic/bold runs.
    pub(super) fn paragraph(&mut self, runs: &[Run]) {
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

    /// A single centered line (titles, headings, scene breaks) followed by a gap.
    pub(super) fn centered(
        &mut self,
        text: &str,
        bold: bool,
        italic: bool,
        size: f32,
        leading: f32,
    ) {
        let words = words_of(&[Run::new(text, bold, italic)]);
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
    /// Each segment's text is resolved to glyph ids (body face, or the fallback for
    /// glyphs it lacks) and shown as Identity-H hex runs.
    pub(super) fn draw_line(&mut self, x: f32, y: f32, segs: &[Run], size: f32) {
        let block = self.glyph_block(
            x,
            y,
            size,
            segs.iter().map(|r| (r.text.as_str(), r.bold, r.italic)),
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

    /// A `BT…ET` text block at `(x, y)`: each `(text, bold, italic)` segment resolved
    /// to glyph-id hex runs (`/Fn` font switches between body faces and the fallback).
    pub(super) fn glyph_block<'a>(
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
