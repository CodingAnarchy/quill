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
    pub fn reversed(self) -> Self {
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
}

/// Vim's character classes for word motions: a word is a run of word-characters
/// or a run of punctuation, separated by whitespace.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Punct,
    Space,
}

impl CharClass {
    fn of(grapheme: &str) -> Self {
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

impl Buffer {
    /// Move to the start of the next word — Vim's `w`. A word is a run of
    /// word-characters or a run of punctuation; whitespace separates them. Spans
    /// line boundaries like Vim.
    pub fn move_word_forward(&mut self) {
        let items = self.classes();
        let mut i = self.grapheme_index(&items);
        let Some(&(_, start)) = items.get(i) else { return };
        if start != CharClass::Space {
            while items.get(i).is_some_and(|&(_, c)| c == start) {
                i += 1;
            }
        }
        while items.get(i).is_some_and(|&(_, c)| c == CharClass::Space) {
            i += 1;
        }
        let target = items.get(i).map_or_else(|| self.rope.len_bytes(), |&(b, _)| b);
        self.cursor = self.cursor_at_byte(target);
        self.goal_x = None;
    }

    /// Move to the start of the current/previous word — Vim's `b`.
    pub fn move_word_backward(&mut self) {
        let items = self.classes();
        let cur = self.grapheme_index(&items);
        if cur == 0 {
            return;
        }
        let mut i = cur - 1;
        while i > 0 && items[i].1 == CharClass::Space {
            i -= 1;
        }
        if items[i].1 == CharClass::Space {
            self.cursor = self.cursor_at_byte(items[0].0);
            self.goal_x = None;
            return;
        }
        let class = items[i].1;
        while i > 0 && items[i - 1].1 == class {
            i -= 1;
        }
        self.cursor = self.cursor_at_byte(items[i].0);
        self.goal_x = None;
    }

    /// Move to the end of the next word — Vim's `e`.
    pub fn move_word_end(&mut self) {
        let items = self.classes();
        let mut i = self.grapheme_index(&items) + 1;
        while items.get(i).is_some_and(|&(_, c)| c == CharClass::Space) {
            i += 1;
        }
        let Some(&(_, class)) = items.get(i) else { return };
        while items.get(i + 1).is_some_and(|&(_, c)| c == class) {
            i += 1;
        }
        self.cursor = self.cursor_at_byte(items[i].0);
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
            FindKind::FindFwd | FindKind::TillFwd => hits.iter().filter(|&&c| c > cur).nth(nth).copied(),
            FindKind::FindBack | FindKind::TillBack => hits.iter().rev().filter(|&&c| c < cur).nth(nth).copied(),
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

    /// The whole buffer as `(byte offset, character class)` per grapheme — the
    /// scan the word motions walk.
    fn classes(&self) -> Vec<(usize, CharClass)> {
        let text = self.rope.to_string();
        text.grapheme_indices(true).map(|(b, g)| (b, CharClass::of(g))).collect()
    }

    /// Index into [`classes`](Self::classes) of the grapheme at the cursor (the
    /// length when the cursor is at the very end).
    fn grapheme_index(&self, items: &[(usize, CharClass)]) -> usize {
        let byte = self.cursor_byte();
        items.iter().position(|&(b, _)| b >= byte).unwrap_or(items.len())
    }

    /// The byte range of a text object around the cursor (#203) — `inner` is the
    /// content, the `around` form includes the delimiters / trailing whitespace.
    /// `None` when there's no such object here.
    pub fn text_object(&self, object: TextObject, around: bool) -> Option<(usize, usize)> {
        match object {
            TextObject::Word => self.word_object(around),
            TextObject::Quote(q) => self.quote_object(q, around),
            TextObject::Pair(open, close) => self.pair_object(open, close, around),
        }
    }

    /// `iw` / `aw`: the run of same-class graphemes under the cursor; `aw` also
    /// takes the trailing whitespace (or leading, when there's none trailing).
    fn word_object(&self, around: bool) -> Option<(usize, usize)> {
        let items = self.classes();
        if items.is_empty() {
            return None;
        }
        let cur = self.grapheme_index(&items).min(items.len() - 1);
        let class = items[cur].1;
        let mut start = cur;
        while start > 0 && items[start - 1].1 == class {
            start -= 1;
        }
        let mut end = cur; // last grapheme index of the run
        while end + 1 < items.len() && items[end + 1].1 == class {
            end += 1;
        }
        let mut start_b = items[start].0;
        let mut end_b = items.get(end + 1).map_or_else(|| self.len_bytes(), |&(b, _)| b);
        if around {
            let mut e = end + 1;
            if items.get(e).is_some_and(|&(_, c)| c == CharClass::Space) {
                while items.get(e).is_some_and(|&(_, c)| c == CharClass::Space) {
                    e += 1;
                }
                end_b = items.get(e).map_or_else(|| self.len_bytes(), |&(b, _)| b);
            } else {
                while start > 0 && items[start - 1].1 == CharClass::Space {
                    start -= 1;
                }
                start_b = items[start].0;
            }
        }
        Some((start_b, end_b))
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
    /// honoring nesting, searched across the whole buffer.
    fn pair_object(&self, open: char, close: char, around: bool) -> Option<(usize, usize)> {
        let text = self.rope.to_string();
        let cur = self.cursor_byte().min(text.len());
        // Walk back to the unmatched open bracket enclosing the cursor.
        let mut depth = 0usize;
        let mut open_b = None;
        for (b, ch) in text[..=cur.min(text.len().saturating_sub(1))].char_indices().rev() {
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
        depth = 0;
        let mut close_b = None;
        for (b, ch) in text[open_b + open.len_utf8()..].char_indices() {
            let b = open_b + open.len_utf8() + b;
            if ch == open {
                depth += 1;
            } else if ch == close {
                if depth == 0 {
                    close_b = Some(b);
                    break;
                }
                depth -= 1;
            }
        }
        let close_b = close_b?;
        Some(if around {
            (open_b, close_b + close.len_utf8())
        } else {
            (open_b + open.len_utf8(), close_b)
        })
    }
}
