//! Vim-style motions and text objects (#202, #203). These all scan the document
//! by character class and return byte ranges; extracted from `buffer/mod.rs` to
//! keep the rope/parse/undo core legible (#316). They run on the edit path, so
//! the `keystroke_is_decoupled_from_document_size` / `reparse_is_fast_at_window_size`
//! perf guards back them.

use unicode_segmentation::UnicodeSegmentation;

use super::{col_to_byte, Buffer};

/// Direction and stop rule for the `f`/`t`/`F`/`T` find-char motions (#202):
/// land *on* the target (`Find`) or just *before* it (`Till`), forward or back.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FindKind {
    FindFwd,
    FindBack,
    TillFwd,
    TillBack,
}

impl FindKind {
    /// The opposite direction — for `,`, which repeats a find the other way.
    pub const fn reversed(self) -> Self {
        match self {
            Self::FindFwd => Self::FindBack,
            Self::FindBack => Self::FindFwd,
            Self::TillFwd => Self::TillBack,
            Self::TillBack => Self::TillFwd,
        }
    }
}

/// A Vim text object (#203) the operators `d`/`c` act on — `iw`/`aw`,
/// `i"`/`a"`, `i(`/`a(`, etc. The `around` flag (inner vs. a-) is passed
/// separately to [`Buffer::text_object`].
#[derive(Clone, Copy)]
pub enum TextObject {
    /// `w`: the word (or whitespace run) under the cursor.
    Word,
    /// `"` / `'` / `` ` ``: text between a pair of that quote on the line.
    Quote(char),
    /// `(` `)` `[` `]` `{` `}` `<` `>`: the enclosing matched pair.
    Pair(char, char),
    /// `p`: the paragraph — the run of non-blank lines around the cursor (or the
    /// run of blank lines, if the cursor is on one). `around` also takes the
    /// trailing blank lines (or the leading ones, when there's nothing trailing).
    Paragraph,
    /// `s`: the sentence — prose from one sentence terminator (`.`/`!`/`?` + space
    /// or line end) to the next. `around` also takes the trailing whitespace.
    Sentence,
}

/// Vim's character classes for word motions: a word is a run of word-characters
/// or a run of punctuation, separated by whitespace. Shared with the prompt line
/// editor (#706), which reuses this classification for word-delete.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CharClass {
    Word,
    Punct,
    Space,
}

impl CharClass {
    pub(crate) fn of(grapheme: &str) -> Self {
        let c = grapheme.chars().next().unwrap_or(' ');
        if c.is_whitespace() {
            Self::Space
        } else if c.is_alphanumeric() || c == '_' {
            Self::Word
        } else {
            Self::Punct
        }
    }
}

/// Initial half-width (bytes) of the [`ClassWindow`] a word motion scans around
/// the cursor before falling back to a wider window (#660). A word/whitespace run
/// almost never exceeds this, so one window covers the walk without copying the
/// whole rope; the doubling retry keeps a pathological run correct.
const CLASS_WINDOW: usize = 4096;

/// A bounded slice of `(absolute byte offset, character class)` graphemes around
/// the cursor — the scan the word motions and `iw`/`aw` walk, without
/// materializing the whole document (#660). `at_doc_start`/`at_doc_end` say
/// whether the window's edges are the real document boundaries, so a walk that
/// reaches an edge can tell "end of document" from "end of window".
struct ClassWindow {
    items: Vec<(usize, CharClass)>,
    /// Grapheme index of the cursor within `items` (its length when the cursor
    /// sits at or past the window's end).
    cursor: usize,
    doc_len: usize,
    at_doc_start: bool,
    at_doc_end: bool,
}

impl ClassWindow {
    /// The class at grapheme index `i`, or `None` past the window's end.
    fn class(&self, i: usize) -> Option<CharClass> {
        self.items.get(i).map(|&(_, c)| c)
    }

    /// The byte offset at grapheme index `i`; the document length when `i` is
    /// past the last grapheme (only ever called at a genuine `at_doc_end` edge).
    fn byte_at(&self, i: usize) -> usize {
        self.items.get(i).map_or(self.doc_len, |&(b, _)| b)
    }
}

impl Buffer {
    /// Move to the start of the next word — Vim's `w`. A word is a run of
    /// word-characters or a run of punctuation; whitespace separates them. Spans
    /// line boundaries like Vim.
    pub fn move_word_forward(&mut self) {
        let target = self.with_class_window(|w| {
            let mut i = w.cursor;
            if let Some(start) = w.class(i).filter(|&c| c != CharClass::Space) {
                while w.class(i) == Some(start) {
                    i += 1;
                }
            }
            while w.class(i) == Some(CharClass::Space) {
                i += 1;
            }
            // Reached the window's forward edge with document still beyond it —
            // widen and rescan so the walk isn't cut short.
            if i >= w.items.len() && !w.at_doc_end {
                return None;
            }
            Some(w.byte_at(i))
        });
        self.cursor = self.cursor_at_byte(target);
        self.goal_x = None;
    }

    /// Move to the start of the current/previous word — Vim's `b`.
    pub fn move_word_backward(&mut self) {
        let Some(target) = self.with_class_window(|w| {
            if w.cursor == 0 {
                return if w.at_doc_start { Some(None) } else { None };
            }
            let mut i = w.cursor - 1;
            while i > 0 && w.items[i].1 == CharClass::Space {
                i -= 1;
            }
            if i == 0 && !w.at_doc_start {
                return None; // the run (space or word) may extend before the window
            }
            if w.items[i].1 == CharClass::Space {
                return Some(Some(w.items[0].0)); // i == 0 at genuine document start
            }
            let class = w.items[i].1;
            while i > 0 && w.items[i - 1].1 == class {
                i -= 1;
            }
            if i == 0 && !w.at_doc_start {
                return None; // the word run may continue before the window
            }
            Some(Some(w.items[i].0))
        }) else {
            return; // already at the document start
        };
        self.cursor = self.cursor_at_byte(target);
        self.goal_x = None;
    }

    /// Move to the end of the next word — Vim's `e`.
    pub fn move_word_end(&mut self) {
        let Some(target) = self.with_class_window(|w| {
            let mut i = w.cursor + 1;
            while w.class(i) == Some(CharClass::Space) {
                i += 1;
            }
            let Some(class) = w.class(i) else {
                // Nothing but whitespace ahead: if the document really ends here
                // the cursor stays put, otherwise widen to look further.
                return if w.at_doc_end { Some(None) } else { None };
            };
            while w.class(i + 1) == Some(class) {
                i += 1;
            }
            if i + 1 >= w.items.len() && !w.at_doc_end {
                return None; // the run may extend past the window
            }
            Some(Some(w.items[i].0))
        }) else {
            return; // nothing ahead — cursor unchanged
        };
        self.cursor = self.cursor_at_byte(target);
        self.goal_x = None;
    }

    /// Find the `count`-th occurrence of `target` on the current line and move to
    /// it (or just shy of it, for `t`/`T`) — Vim's `f`/`t`/`F`/`T`. No move when
    /// there is no such occurrence.
    pub fn find_char(&mut self, target: char, kind: FindKind, count: usize) {
        let line = self.line(self.cursor.line);
        let hits: Vec<usize> = line
            .graphemes(true)
            .enumerate()
            .filter(|(_, g)| g.starts_with(target))
            .map(|(i, _)| i)
            .collect();
        let cur = self.cursor.col;
        let nth = count.saturating_sub(1);
        let found = match kind {
            FindKind::FindFwd | FindKind::TillFwd => {
                hits.iter().filter(|&&c| c > cur).nth(nth).copied()
            }
            FindKind::FindBack | FindKind::TillBack => {
                hits.iter().rev().filter(|&&c| c < cur).nth(nth).copied()
            }
        };
        if let Some(col) = found {
            self.cursor.col = match kind {
                FindKind::TillFwd => col - 1,
                FindKind::TillBack => col + 1,
                _ => col,
            };
            self.goal_x = None;
        }
    }

    /// Run `f` against a [`ClassWindow`] centered on the cursor, widening the
    /// window and retrying whenever `f` returns `None` (it reached a window edge
    /// that isn't the real document boundary). Word motions only ever walk a
    /// bounded distance, so the first window almost always suffices — this avoids
    /// materializing the whole rope (#660) while staying byte-identical to a
    /// full-document scan. The window doubles each retry; once it reaches the
    /// buffer size [`class_window`](Self::class_window) clamps it to the whole
    /// document, so a genuine boundary is always reached.
    fn with_class_window<T>(&self, mut f: impl FnMut(&ClassWindow) -> Option<T>) -> T {
        let mut radius = CLASS_WINDOW;
        loop {
            let window = self.class_window(radius);
            let covers_all = window.at_doc_start && window.at_doc_end;
            if let Some(out) = f(&window) {
                return out;
            }
            // `f` asked for more but the window already spans the whole document
            // — its edge *is* the boundary; force one final pass that accepts it.
            debug_assert!(!covers_all, "window scan failed to terminate");
            radius = radius.saturating_mul(2);
        }
    }

    /// A `(byte offset, character class)` slice of the rope around the cursor:
    /// `radius` bytes each side, snapped to grapheme boundaries. Byte offsets are
    /// absolute so results map straight back to document positions.
    fn class_window(&self, radius: usize) -> ClassWindow {
        let byte = self.cursor_byte();
        let len = self.rope.len_bytes();
        let lo = self.rope.byte_to_char(byte.saturating_sub(radius));
        let lo = self.rope.char_to_byte(lo);
        let hi = self.rope.byte_to_char((byte + radius).min(len));
        let hi = self.rope.char_to_byte(hi);
        let text = self.rope.byte_slice(lo..hi).to_string();
        let items: Vec<(usize, CharClass)> = text
            .grapheme_indices(true)
            .map(|(b, g)| (lo + b, CharClass::of(g)))
            .collect();
        let cursor = items
            .iter()
            .position(|&(b, _)| b >= byte)
            .unwrap_or(items.len());
        ClassWindow {
            items,
            cursor,
            doc_len: len,
            at_doc_start: lo == 0,
            at_doc_end: hi == len,
        }
    }

    /// The byte range of a text object around the cursor (#203) — `inner` is the
    /// content, the `around` form includes the delimiters / trailing whitespace.
    /// `None` when there's no such object here.
    pub fn text_object(&self, object: TextObject, around: bool) -> Option<(usize, usize)> {
        match object {
            TextObject::Word => self.word_object(around),
            TextObject::Quote(q) => self.quote_object(q, around),
            TextObject::Pair(open, close) => self.pair_object(open, close, around),
            TextObject::Paragraph => self.paragraph_object(around),
            TextObject::Sentence => self.sentence_object(around),
        }
    }

    /// `iw` / `aw`: the run of same-class graphemes under the cursor; `aw` also
    /// takes the trailing whitespace (or leading, when there's none trailing).
    fn word_object(&self, around: bool) -> Option<(usize, usize)> {
        self.with_class_window(|w| {
            if w.items.is_empty() {
                return Some(None);
            }
            let cur = w.cursor.min(w.items.len() - 1);
            let class = w.items[cur].1;
            let mut start = cur;
            while start > 0 && w.items[start - 1].1 == class {
                start -= 1;
            }
            if start == 0 && !w.at_doc_start {
                return None; // the run may reach before the window
            }
            let mut end = cur; // last grapheme index of the run
            while end + 1 < w.items.len() && w.items[end + 1].1 == class {
                end += 1;
            }
            if end + 1 >= w.items.len() && !w.at_doc_end {
                return None; // the run may reach past the window
            }
            let mut start_b = w.items[start].0;
            let mut end_b = w.byte_at(end + 1);
            if around {
                let mut e = end + 1;
                if w.class(e) == Some(CharClass::Space) {
                    while w.class(e) == Some(CharClass::Space) {
                        e += 1;
                    }
                    if e >= w.items.len() && !w.at_doc_end {
                        return None; // trailing whitespace may extend past the window
                    }
                    end_b = w.byte_at(e);
                } else {
                    while start > 0 && w.items[start - 1].1 == CharClass::Space {
                        start -= 1;
                    }
                    if start == 0 && !w.at_doc_start {
                        return None; // leading whitespace may extend before the window
                    }
                    start_b = w.items[start].0;
                }
            }
            Some(Some((start_b, end_b)))
        })
    }

    /// `i"` / `a"` (and `'`, `` ` ``): the text between the pair of quotes on the
    /// current line that the cursor is at or before.
    fn quote_object(&self, quote: char, around: bool) -> Option<(usize, usize)> {
        let line = self.line(self.cursor.line);
        let base = self.line_start_byte(self.cursor.line);
        let marks: Vec<usize> = line
            .grapheme_indices(true)
            .filter(|(_, g)| g.starts_with(quote))
            .map(|(b, _)| b)
            .collect();
        let cur = col_to_byte(&line, self.cursor.col);
        for pair in marks.chunks(2) {
            let [open, close] = *pair else { break };
            if cur <= close {
                return Some(if around {
                    (base + open, base + close + quote.len_utf8())
                } else {
                    (base + open + quote.len_utf8(), base + close)
                });
            }
        }
        None
    }

    /// `i(` / `a(` (and the other bracket pairs): the enclosing matched pair,
    /// honoring nesting. Scans outward from the cursor by streaming rope
    /// characters — never copying the whole buffer (#660) — stopping as soon as
    /// the enclosing pair is found.
    fn pair_object(&self, open: char, close: char, around: bool) -> Option<(usize, usize)> {
        let len = self.rope.len_bytes();
        let cur = self.cursor_byte().min(len);
        // Walk back to the unmatched open bracket enclosing the cursor. Position
        // the iterator just past the cursor's own char so it is the first one
        // `prev()` yields (mirroring the old inclusive `text[..=cur]` scan); a
        // close bracket sitting exactly at the cursor doesn't count as nesting.
        let start_char = if cur < len {
            self.rope.byte_to_char(cur) + 1
        } else {
            self.rope.len_chars()
        };
        let mut chars = self.rope.chars_at(start_char);
        let mut b = self.rope.char_to_byte(start_char);
        let mut depth = 0usize;
        let mut open_b = None;
        while let Some(ch) = chars.prev() {
            b -= ch.len_utf8();
            if ch == close && b != cur {
                depth += 1;
            } else if ch == open {
                if depth == 0 {
                    open_b = Some(b);
                    break;
                }
                depth -= 1;
            }
        }
        let open_b = open_b?;
        // Walk forward to its matching close.
        let mut b = open_b + open.len_utf8();
        let chars = self.rope.chars_at(self.rope.byte_to_char(b));
        depth = 0;
        let mut close_b = None;
        for ch in chars {
            if ch == open {
                depth += 1;
            } else if ch == close {
                if depth == 0 {
                    close_b = Some(b);
                    break;
                }
                depth -= 1;
            }
            b += ch.len_utf8();
        }
        let close_b = close_b?;
        Some(if around {
            (open_b, close_b + close.len_utf8())
        } else {
            (open_b + open.len_utf8(), close_b)
        })
    }

    /// Whether line `idx` is a paragraph separator — empty or whitespace-only.
    /// Reads one line, never the whole rope; the paragraph motions/objects only
    /// walk outward line by line until they hit one of these boundaries, so they
    /// stay windowed like the word motions (#660).
    fn line_is_blank(&self, idx: usize) -> bool {
        self.line(idx).trim().is_empty()
    }

    /// Move to the start of the next paragraph — Vim's `}`. Scans down: past any
    /// blank run the cursor sits in, then past the paragraph's non-blank lines, to
    /// land on the following blank line (or the last line at document end). Lands
    /// at column 0, as Vim does.
    pub fn move_paragraph_forward(&mut self) {
        let last = self.line_count().saturating_sub(1);
        let mut i = self.cursor.line;
        // Advance at least one line, then walk to the next boundary.
        while i < last && self.line_is_blank(i) {
            i += 1;
        }
        while i < last && !self.line_is_blank(i) {
            i += 1;
        }
        self.cursor = self.cursor_at_byte(self.line_start_byte(i));
        self.goal_x = None;
    }

    /// Move to the start of the previous paragraph — Vim's `{`. The mirror of
    /// [`move_paragraph_forward`](Self::move_paragraph_forward), walking up.
    pub fn move_paragraph_backward(&mut self) {
        let mut i = self.cursor.line;
        while i > 0 && self.line_is_blank(i) {
            i -= 1;
        }
        while i > 0 && !self.line_is_blank(i) {
            i -= 1;
        }
        self.cursor = self.cursor_at_byte(self.line_start_byte(i));
        self.goal_x = None;
    }

    /// `ip` / `ap`: the paragraph around the cursor. `ip` is the run of like lines
    /// (all non-blank, or all blank) the cursor is in; `ap` also takes the trailing
    /// blank lines — or the leading blank lines when there are none trailing —
    /// matching Vim. The range is linewise (whole lines, ending at a line start).
    fn paragraph_object(&self, around: bool) -> Option<(usize, usize)> {
        let last = self.line_count().saturating_sub(1);
        let cur = self.cursor.line.min(last);
        let on_blank = self.line_is_blank(cur);
        // The run of same-kind lines under the cursor.
        let mut first = cur;
        while first > 0 && self.line_is_blank(first - 1) == on_blank {
            first -= 1;
        }
        let mut end = cur; // last line of the run (inclusive)
        while end < last && self.line_is_blank(end + 1) == on_blank {
            end += 1;
        }
        if around {
            // `ap` extends over the trailing blank lines; if there are none (the
            // paragraph runs to the document end), it takes the leading blanks.
            let trailing_end = {
                let mut e = end;
                while e < last && self.line_is_blank(e + 1) != on_blank {
                    e += 1;
                }
                e
            };
            if trailing_end > end {
                end = trailing_end;
            } else {
                while first > 0 && self.line_is_blank(first - 1) != on_blank {
                    first -= 1;
                }
            }
        }
        let (lo, hi) = (self.line_start_byte(first), self.next_line_byte(end));
        (lo < hi).then_some((lo, hi))
    }

    /// `is` / `as`: the sentence around the cursor. Sentences end at `.`/`!`/`?`
    /// followed by whitespace or the line/paragraph end (a standard prose
    /// heuristic). `is` is the sentence text; `as` also takes the trailing
    /// whitespace (or leading, when there is none trailing). The scan is bounded to
    /// the surrounding paragraph — a blank line always ends a sentence — so it
    /// stays windowed, never serializing the whole rope (#660).
    fn sentence_object(&self, around: bool) -> Option<(usize, usize)> {
        let cur = self.cursor_byte();
        // A sentence never crosses a paragraph boundary; bound the scan to the
        // paragraph's byte span so a long document only ever reads nearby lines.
        let (lo_bound, hi_bound) = self.paragraph_bounds();
        if lo_bound >= hi_bound {
            return None;
        }
        let text = self.byte_slice(lo_bound, hi_bound);
        let rel = cur.saturating_sub(lo_bound).min(text.len());
        // Split points carve the paragraph into sentences; each span owns its own
        // trailing whitespace, so `[start, end)` is the sentence-plus-trailing-space.
        let splits = sentence_splits(&text);
        let start = splits
            .iter()
            .rev()
            .find(|&&e| e <= rel)
            .copied()
            .unwrap_or(0);
        let end = splits
            .iter()
            .find(|&&e| e > rel)
            .copied()
            .unwrap_or(text.len());
        // The content is the span with its trailing whitespace trimmed off; the
        // span already begins at the first word (the prior sentence took the
        // leading space), except a paragraph that opens with whitespace.
        let content_start = start
            + text[start..end]
                .find(|c: char| !c.is_whitespace())
                .unwrap_or(0);
        let content_end = content_start + text[content_start..end].trim_end().len();
        let (lo, hi) = if around {
            if end > content_end {
                (content_start, end) // include this sentence's trailing whitespace
            } else {
                // No trailing whitespace (paragraph end): take the leading run,
                // i.e. the whitespace the previous sentence's span held.
                let lead = text[..content_start].trim_end().len();
                (lead, content_end)
            }
        } else {
            (content_start, content_end)
        };
        if lo >= hi {
            return None;
        }
        Some((lo_bound + lo, lo_bound + hi))
    }

    /// The byte span `[start, end)` of the paragraph (run of non-blank lines, or of
    /// blank lines) around the cursor — the window the sentence object scans within.
    fn paragraph_bounds(&self) -> (usize, usize) {
        let last = self.line_count().saturating_sub(1);
        let cur = self.cursor.line.min(last);
        let on_blank = self.line_is_blank(cur);
        let mut first = cur;
        while first > 0 && self.line_is_blank(first - 1) == on_blank {
            first -= 1;
        }
        let mut end = cur;
        while end < last && self.line_is_blank(end + 1) == on_blank {
            end += 1;
        }
        (self.line_start_byte(first), self.next_line_byte(end))
    }
}

/// The byte offsets that split `text` into sentences — the point *after* each
/// terminator's trailing whitespace, so the sentence before a split owns that
/// whitespace (Vim's `as`). A sentence ends at `.`/`!`/`?` (with any trailing
/// closing quotes/brackets) followed by whitespace or the text end — a dot inside
/// a token like "e.g." isn't a break. A newline splits too, so `is`/`as` don't
/// span a hard line break. Offsets `0` and `text.len()` are implicit.
fn sentence_splits(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut splits = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let terminator = if c == b'.' || c == b'!' || c == b'?' {
            // Consume runs of terminators (…?!) and trailing closers ("')]}.
            let mut j = i + 1;
            while j < bytes.len()
                && matches!(
                    bytes[j],
                    b'.' | b'!' | b'?' | b'"' | b'\'' | b')' | b']' | b'}'
                )
            {
                j += 1;
            }
            // A sentence break needs whitespace (or the text end) after it —
            // "e.g." mid-word doesn't split.
            (j >= bytes.len() || bytes[j].is_ascii_whitespace()).then_some(j)
        } else {
            None
        };
        if let Some(j) = terminator {
            // Fold the trailing whitespace into this sentence's span.
            let mut k = j;
            while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                k += 1;
            }
            if k < bytes.len() {
                splits.push(k);
            }
            i = k;
        } else if c == b'\n' {
            // A line break splits too; the newline stays with the line before it.
            splits.push(i + 1);
            i += 1;
        } else {
            i += 1;
        }
    }
    splits
}
