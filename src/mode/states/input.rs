//! Metadata-edit and prompt mode state (#404/#706): the scratch metadata-field
//! editor and the status-bar prompt with its caret editing. Split out of
//! `states.rs` (pure move).

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::{Buffer, CharClass};

use crate::mode::{MetaField, Pending};

/// State for an open [`Mode::EditMeta`]. `label` is public for the editor's
/// banner; the rest is the handler's business. The scratch `buffer` is reached
/// through `App::edit_buffer`.
pub struct MetaEdit {
    /// Which field is being edited (a quotation field).
    pub(crate) target: MetaField,
    /// Scratch buffer holding the field's text.
    pub(crate) buffer: Buffer,
    /// Banner label, e.g. "chapter epigraph".
    pub label: String,
    /// Chapter to reselect when editing finishes.
    pub(crate) nav_return: Option<usize>,
}

/// State for an open [`Mode::Prompt`]. `label`/`input` are public for the status
/// bar to render; the rest is the handler's business.
pub struct PromptState {
    /// What confirming the prompt does.
    pub(crate) pending: Pending,
    /// The prompt label shown in the status bar.
    pub label: String,
    /// Text typed so far (empty for y/N confirmations).
    pub input: String,
    /// Caret position as a byte offset into `input` (#706). Insertions and
    /// deletions happen here; the arrow keys, Home/End, and word-delete move it.
    /// Always kept on a `char` boundary of `input`.
    pub caret: usize,
    /// Where to return when the prompt closes: `Some(selected)` back to the
    /// chapter browser, `None` to insert mode (e.g. the quit prompt).
    pub(crate) nav_return: Option<usize>,
}

impl PromptState {
    /// A prompt prefilled with `input`, caret at its end (the natural spot to
    /// keep typing or edit from).
    pub(crate) const fn new(pending: Pending, label: String, input: String) -> Self {
        let caret = input.len();
        Self {
            pending,
            label,
            input,
            caret,
            nav_return: None,
        }
    }

    /// Clear the input and reset the caret to the start — used by tests to
    /// replace a prefilled default before typing a fresh value, keeping the
    /// caret invariant (in production the author backspaces through it).
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.input.clear();
        self.caret = 0;
    }

    /// Insert a typed character at the caret and advance past it (#706).
    pub(crate) fn insert(&mut self, c: char) {
        self.input.insert(self.caret, c);
        self.caret += c.len_utf8();
    }

    /// Delete the character before the caret (Backspace); a no-op at the start.
    pub(crate) fn backspace(&mut self) {
        if let Some(prev) = self.prev_char_boundary(self.caret) {
            self.input.replace_range(prev..self.caret, "");
            self.caret = prev;
        }
    }

    /// Delete the character at the caret (forward Delete); a no-op at the end.
    pub(crate) fn delete_forward(&mut self) {
        if let Some(next) = self.next_char_boundary(self.caret) {
            self.input.replace_range(self.caret..next, "");
        }
    }

    /// Delete the word before the caret (Ctrl/Alt-Backspace), reusing the
    /// buffer's word classification (#706): skip any whitespace immediately
    /// before the caret, then the run of one character class before that.
    pub(crate) fn delete_word_back(&mut self) {
        let target = self.word_start_before(self.caret);
        if target < self.caret {
            self.input.replace_range(target..self.caret, "");
            self.caret = target;
        }
    }

    /// Move the caret one character left; a no-op at the start.
    pub(crate) fn move_left(&mut self) {
        if let Some(prev) = self.prev_char_boundary(self.caret) {
            self.caret = prev;
        }
    }

    /// Move the caret one character right; a no-op at the end.
    pub(crate) fn move_right(&mut self) {
        if let Some(next) = self.next_char_boundary(self.caret) {
            self.caret = next;
        }
    }

    /// Move the caret to the start of the input.
    pub(crate) const fn move_home(&mut self) {
        self.caret = 0;
    }

    /// Move the caret to the end of the input.
    pub(crate) const fn move_end(&mut self) {
        self.caret = self.input.len();
    }

    /// The byte offset of the character boundary before `at`, or `None` at the
    /// start of the string.
    fn prev_char_boundary(&self, at: usize) -> Option<usize> {
        self.input[..at].char_indices().next_back().map(|(i, _)| i)
    }

    /// The byte offset of the character boundary after `at`, or `None` at the
    /// end of the string.
    fn next_char_boundary(&self, at: usize) -> Option<usize> {
        self.input[at..].chars().next().map(|c| at + c.len_utf8())
    }

    /// The start of the word ending at `at`: skip trailing whitespace, then the
    /// run of the class the preceding character belongs to. Mirrors the editor's
    /// `delete_word_back` (#641) using the shared [`CharClass`].
    fn word_start_before(&self, at: usize) -> usize {
        let mut boundaries = self.input[..at].grapheme_indices(true).rev().peekable();
        // Skip a run of trailing whitespace first.
        let mut cut = at;
        while let Some(&(i, g)) = boundaries.peek() {
            if CharClass::of(g) == CharClass::Space {
                cut = i;
                boundaries.next();
            } else {
                break;
            }
        }
        // Then delete the contiguous run of the next class we hit.
        let Some(&(_, g)) = boundaries.peek() else {
            return cut;
        };
        let class = CharClass::of(g);
        for (i, g) in boundaries {
            if CharClass::of(g) == class {
                cut = i;
            } else {
                break;
            }
        }
        cut
    }
}
