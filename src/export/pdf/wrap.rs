//! Line-wrapping for the PDF writer: split styled runs into words and pack them into
//! lines for a given per-line width, measuring with the layout's fonts. Pure of any
//! PDF emission — the layout module positions and draws the returned lines.

use super::{FontStyle, Layout, Run};

/// One word carrying the emphasis it inherits from its run and whether it is
/// heading text (drawn in the heading family), the unit line-wrapping works in.
pub(super) struct Word {
    pub(super) text: String,
    pub(super) bold: bool,
    pub(super) italic: bool,
    /// Heading text (chapter/part titles, sub-headings, the book title) — resolved
    /// through the heading font, never body-bold (#638).
    pub(super) heading: bool,
}

impl Word {
    /// The `(bold, italic)` style this word draws in.
    pub(super) fn style(&self) -> FontStyle {
        FontStyle {
            bold: self.bold,
            italic: self.italic,
        }
    }
}

/// Split runs into wrap-able words, preserving each run's emphasis. `heading` marks
/// the words as heading text so they resolve through the heading font (#638).
pub(super) fn words_of(runs: &[Run], heading: bool) -> Vec<Word> {
    let mut words = Vec::new();
    for Run { text, bold, italic } in runs {
        for w in text.split(' ').filter(|w| !w.is_empty()) {
            words.push(Word {
                text: w.to_string(),
                bold: *bold,
                italic: *italic,
                heading,
            });
        }
    }
    words
}

/// Greedy line-wrap: pack words into `width_for_line(line_index)` for each line.
/// Each line is returned with its content width (words plus inter-word spaces) so
/// callers align it without re-measuring. The per-line width callback lets one
/// wrapper serve both flush paragraphs (first line narrowed for the indent) and drop
/// caps (first lines narrowed for the initial).
pub(super) fn wrap_lines<'w>(
    layout: &Layout,
    words: &'w [Word],
    size: f32,
    width_for_line: impl Fn(usize) -> f32,
) -> Vec<(Vec<&'w Word>, f32)> {
    let space = layout.measure(" ", size, FontStyle::REGULAR, false);
    let mut lines: Vec<(Vec<&Word>, f32)> = Vec::new();
    let mut line: Vec<&Word> = Vec::new();
    let mut content = 0.0; // width of the current line's words + inter-word spaces
    for w in words {
        let ww = layout.measure(&w.text, size, w.style(), w.heading);
        let advance = if line.is_empty() { ww } else { space + ww };
        if !line.is_empty() && content + advance > width_for_line(lines.len()) {
            lines.push((std::mem::take(&mut line), content));
            content = ww;
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

/// One drawable segment of a wrapped line: its text (with any trailing space), the
/// `(bold, italic)` style, and whether it draws in the heading family (#638). The
/// draw layer resolves each to glyph runs, obliquing synthetic italics.
pub(super) struct Seg {
    pub(super) text: String,
    pub(super) style: FontStyle,
    pub(super) heading: bool,
}

/// A wrapped line as drawable segments: each word carries a trailing space except
/// the last, so `Tj` advances spell the gaps in each word's own font.
pub(super) fn segments(line: &[&Word]) -> Vec<Seg> {
    let n = line.len();
    line.iter()
        .enumerate()
        .map(|(i, w)| {
            let text = if i + 1 < n {
                format!("{} ", w.text)
            } else {
                w.text.clone()
            };
            Seg {
                text,
                style: w.style(),
                heading: w.heading,
            }
        })
        .collect()
}
