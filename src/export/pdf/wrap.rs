//! Line-wrapping for the PDF writer: split styled runs into words and pack them into
//! lines for a given per-line width, measuring with the layout's fonts. Pure of any
//! PDF emission — the layout module positions and draws the returned lines.
//!
//! Justified body paragraphs (#678) may hyphenate: when a word overruns the line, the
//! wrapper tries to break it at a valid Knuth-Liang point, drawing the head with a
//! trailing hyphen and carrying the tail to the next line. Hyphenation is opt-in
//! (`Hyphenate::On`) so headings, epigraphs, and preformatted/inset blocks — which
//! pass `Off` — are never broken. The en-US patterns are embedded and the hyphenator
//! is built once, off the keystroke path: it runs only during export.

use std::sync::OnceLock;

use hyphenation::{Hyphenator, Language, Load, Standard};

use super::{FontStyle, Layout, Run};

/// Don't hyphenate a word shorter than this (in `char`s): short words rarely have a
/// useful break and a broken three-letter word reads worse than a loose gap.
const MIN_HYPHENATED_WORD: usize = 6;
/// Keep at least this many `char`s on each side of a hyphenation break, so neither the
/// head fragment (before the hyphen) nor the tail is a stub like "a-" or "-y".
const MIN_FRAGMENT: usize = 3;

/// The embedded en-US Knuth-Liang hyphenation patterns, loaded once. Building the
/// `Standard` dictionary parses the embedded pattern table, so it's cached in a
/// `OnceLock` — export may hyphenate thousands of words, and the editor never touches
/// this path (it's reached only from the PDF writer).
fn en_us() -> &'static Standard {
    static DICT: OnceLock<Standard> = OnceLock::new();
    DICT.get_or_init(|| {
        Standard::from_embedded(Language::EnglishUS)
            .expect("embedded en-US hyphenation patterns load")
    })
}

/// Whether a flow's lines may be hyphenated. Only justified body paragraphs opt in
/// (#678); headings, epigraphs, and preformatted/inset blocks stay `Off`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Hyphenate {
    On,
    Off,
}

/// Valid byte offsets within `word` at which a hyphen may be inserted, honoring the
/// minimum-fragment rule so neither side is a stub. Returns nothing for short words,
/// words with non-letters (numbers, punctuation-joined tokens), or words with no
/// pattern break — the caller then leaves the word whole.
fn hyphen_points(word: &str) -> Vec<usize> {
    let len = word.chars().count();
    if len < MIN_HYPHENATED_WORD || !word.chars().all(char::is_alphabetic) {
        return Vec::new();
    }
    // Byte offset of each break must leave ≥ MIN_FRAGMENT chars on both sides.
    en_us()
        .hyphenate(word)
        .breaks
        .into_iter()
        .filter(|&b| {
            let head = word[..b].chars().count();
            head >= MIN_FRAGMENT && len - head >= MIN_FRAGMENT
        })
        .collect()
}

/// One word carrying the emphasis it inherits from its run and whether it is
/// heading text (drawn in the heading family), the unit line-wrapping works in.
#[derive(Clone)]
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
    pub(super) const fn style(&self) -> FontStyle {
        FontStyle {
            bold: self.bold,
            italic: self.italic,
        }
    }

    /// A fragment of this word (same emphasis/face) with `text` — the head (with a
    /// trailing hyphen) or the tail of a hyphenation split (#678).
    const fn fragment(&self, text: String) -> Self {
        Self {
            text,
            bold: self.bold,
            italic: self.italic,
            heading: self.heading,
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

/// Greedy line-wrap: pack words into `width_for_line(line_index)` for each line, with
/// optional hyphenation (#678). Each line is returned (owning its words, since a
/// hyphenation split makes fragments that don't exist in `words`) with its content
/// width (words plus inter-word spaces) so callers align it without re-measuring. The
/// per-line width callback lets one wrapper serve both flush paragraphs (first line
/// narrowed for the indent) and drop caps (first lines narrowed for the initial).
///
/// When `hyphenate` is `On` and a word would overflow the current (non-empty) line,
/// the wrapper tries to break it at a valid en-US point so the head (plus a hyphen)
/// fills the line and the tail carries over — tightening justified lines. It never
/// hyphenates onto what would be the paragraph's last line's overflow awkwardly: the
/// break is only taken when the head genuinely fits and both fragments clear the
/// minimum-fragment rule; otherwise the word moves whole to the next line as before.
pub(super) fn wrap_lines(
    layout: &Layout,
    words: &[Word],
    size: f32,
    hyphenate: Hyphenate,
    width_for_line: impl Fn(usize) -> f32,
) -> Vec<(Vec<Word>, f32)> {
    let space = layout.measure(" ", size, FontStyle::REGULAR, false);
    let hyphen_w = layout.measure("-", size, FontStyle::REGULAR, false);
    let mut lines: Vec<(Vec<Word>, f32)> = Vec::new();
    let mut line: Vec<Word> = Vec::new();
    let mut content = 0.0; // width of the current line's words + inter-word spaces
    for w in words {
        let ww = layout.measure(&w.text, size, w.style(), w.heading);
        let advance = if line.is_empty() { ww } else { space + ww };
        let width = width_for_line(lines.len());
        if !line.is_empty() && content + advance > width {
            // The word overflows. Before pushing it whole to the next line, try to
            // hyphenate: split it so the head + hyphen fills the remaining room here.
            let room = width - content - space;
            if hyphenate == Hyphenate::On {
                if let Some((head, tail)) = split_to_fit(layout, w, size, room, hyphen_w) {
                    let head_w = layout.measure(&head, size, w.style(), w.heading);
                    line.push(w.fragment(head));
                    lines.push((std::mem::take(&mut line), content + space + head_w));
                    content = layout.measure(&tail, size, w.style(), w.heading);
                    line.push(w.fragment(tail));
                    continue;
                }
            }
            lines.push((std::mem::take(&mut line), content));
            content = ww;
            line.push(w.clone());
        } else {
            content += advance;
            line.push(w.clone());
        }
    }
    if !line.is_empty() {
        lines.push((line, content));
    }
    lines
}

/// Try to split `w` at its last valid hyphenation point whose head (with a trailing
/// hyphen) fits within `room` points. Returns `(head_with_hyphen, tail)`, or `None`
/// when the word is unhyphenable or no head fits — the caller then wraps it whole.
fn split_to_fit(
    layout: &Layout,
    w: &Word,
    size: f32,
    room: f32,
    hyphen_w: f32,
) -> Option<(String, String)> {
    if room <= hyphen_w {
        return None;
    }
    let points = hyphen_points(&w.text);
    // Prefer the longest head that still fits (fills the line most tightly).
    for &b in points.iter().rev() {
        let head = &w.text[..b];
        let head_w = layout.measure(head, size, w.style(), w.heading);
        if head_w + hyphen_w <= room {
            return Some((format!("{head}-"), w.text[b..].to_string()));
        }
    }
    None
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
pub(super) fn segments(line: &[Word]) -> Vec<Seg> {
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
