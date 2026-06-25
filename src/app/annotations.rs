//! Inline author annotations (#302): create a note anchored to a selection (or
//! the word at the cursor), keep it on its phrase as the prose is edited, and
//! review / jump-to / delete the chapter's notes.
//!
//! Annotations are a single-chapter-view feature. They're held as live buffer
//! ranges in [`App::annotations`] for the active chapter, re-anchored on the idle
//! tick from the buffer's edit log ([`Buffer::drain_anchor_edits`]), and
//! persisted per chapter to `annotations.toml` (see [`crate::project`]) so the
//! chapter `.md` stays clean and exports never carry them.

use crossterm::event::{KeyCode, KeyEvent};

use super::App;
use crate::annotation::{remap, Live};
use crate::mode::{Mode, Pending};

impl App {
    /// Reconcile annotations with the buffer on the idle tick: switch the live set
    /// when the active chapter changes, otherwise shift each note through the
    /// edits since the last tick. The continuous view doesn't anchor annotations
    /// (its buffer spans several chapters), so the live set is empty there.
    ///
    /// Re-anchoring follows the keystroke-path edit log only; an undo/redo
    /// (whole-rope snapshot swap) isn't replayed here, so a note may drift after
    /// one — a documented limitation, like the continuous view.
    pub(crate) fn sync_annotations(&mut self) {
        // Always drain — discarding the deltas on a chapter switch / in continuous
        // view, so stale edits never apply to a later chapter's notes.
        let edits = self.edit_buffer_mut().drain_anchor_edits();

        if self.manuscript.is_some() {
            self.annotations.clear();
            self.annotations_chapter.clear();
            return;
        }
        let file = self.project.active_file().to_string();
        if file != self.annotations_chapter {
            self.annotations = self.project.annotations_for(&file);
            self.annotations_chapter = file;
            return;
        }
        if edits.is_empty() {
            return;
        }
        // Re-anchor every note through each edit, dropping any whose text is gone.
        for edit in edits {
            self.annotations
                .retain_mut(|a| match remap((a.start, a.end), edit) {
                    Some((s, e)) => {
                        a.start = s;
                        a.end = e;
                        true
                    }
                    None => false,
                });
        }
        self.annotations.sort_by_key(|a| (a.start, a.end));
    }

    /// Annotated byte ranges to mark in the editor, clamped to the buffer so a
    /// stale offset (e.g. after an undo) never indexes past the end. Single view
    /// only — empty in the continuous view.
    pub(crate) fn annotation_ranges(&self) -> Vec<(usize, usize)> {
        let len = self.edit_buffer().len_bytes();
        self.annotations
            .iter()
            .filter_map(|a| {
                let (s, e) = (a.start.min(len), a.end.min(len));
                (s < e).then_some((s, e))
            })
            .collect()
    }

    /// Begin annotating the selection (Visual mode) or the word at the cursor.
    /// Opens the note prompt; the annotation is created on Enter.
    pub(crate) fn start_annotation(&mut self) {
        let range = match self.mode {
            Mode::Visual { anchor, linewise } => Some(self.selection_bytes(anchor, linewise)),
            _ => self.word_at_cursor(),
        };
        match range {
            Some((start, end)) => self.start_annotation_range(start, end),
            None => self.message = Some("Select text or place the cursor on a word to annotate.".into()),
        }
    }

    /// Open the note prompt for the byte range `[start, end)`. Refuses in the
    /// continuous view, where annotations aren't anchored.
    pub(crate) fn start_annotation_range(&mut self, start: usize, end: usize) {
        if self.manuscript.is_some() {
            self.message = Some("Annotations are available in the single-chapter view.".into());
            return;
        }
        if start >= end {
            self.message = Some("Select text or place the cursor on a word to annotate.".into());
            return;
        }
        let snippet = self.edit_buffer().byte_slice(start, end);
        let label = format!("Note on \"{}\": ", crate::text::ellipsize(&snippet, 30));
        self.open_prompt(Pending::Annotate { start, end }, label, "", None);
    }

    /// Create the annotation for a confirmed note (#302). An empty note cancels.
    pub(crate) fn create_annotation(&mut self, start: usize, end: usize, note: String) {
        let note = note.trim().to_string();
        if note.is_empty() {
            return;
        }
        self.annotations.push(Live { start, end, note });
        self.annotations.sort_by_key(|a| (a.start, a.end));
        self.persist_annotations();
        self.message = Some("Annotation added.".into());
    }

    /// Open the annotation review list for the active chapter, or report when the
    /// chapter has none.
    pub(crate) fn open_annotation_review(&mut self) {
        if self.manuscript.is_some() {
            self.message = Some("Annotations are available in the single-chapter view.".into());
            return;
        }
        if self.annotations.is_empty() {
            self.message = Some("No annotations in this chapter.".into());
            return;
        }
        self.mode = Mode::Annotations { selected: 0 };
    }

    /// Key handling for the annotation review list: move, jump (Enter), delete
    /// (`d`), close (Esc).
    pub(crate) fn on_key_annotations(&mut self, key: KeyEvent) {
        let Mode::Annotations { selected } = self.mode else {
            return;
        };
        if let Some(s) = super::assist::list_move(selected, self.annotations.len(), key.code) {
            self.mode = Mode::Annotations { selected: s };
            return;
        }
        match key.code {
            KeyCode::Enter => {
                let start = self.annotations.get(selected).map(|a| a.start);
                self.mode = self.editing_mode();
                if let Some(start) = start {
                    let buf = self.edit_buffer_mut();
                    let at = start.min(buf.len_bytes());
                    buf.cursor = buf.cursor_at_byte(at);
                }
            }
            KeyCode::Char('d') => {
                if selected < self.annotations.len() {
                    self.annotations.remove(selected);
                    self.persist_annotations();
                    if self.annotations.is_empty() {
                        self.mode = self.editing_mode();
                    } else {
                        self.mode = Mode::Annotations { selected: selected.min(self.annotations.len() - 1) };
                    }
                }
            }
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }

    /// Write the active chapter's live annotations to the side file (#302).
    pub(crate) fn persist_annotations(&mut self) {
        let file = self.project.active_file().to_string();
        if let Err(e) = self.project.set_chapter_annotations(&file, &self.annotations) {
            self.message = Some(format!("Couldn't save annotations: {e}"));
        }
        self.annotations_chapter = file;
    }

    /// The byte range of the word under the cursor (letters/digits/apostrophes),
    /// or `None` when the cursor isn't on a word. Used when annotating without a
    /// selection.
    fn word_at_cursor(&self) -> Option<(usize, usize)> {
        let buf = self.edit_buffer();
        let text = buf.text();
        let cur = buf.cursor_byte().min(text.len());
        let is_word = |c: char| c.is_alphanumeric() || c == '\'' || c == '\u{2019}';

        let mut start = cur;
        for (i, c) in text[..cur].char_indices().rev() {
            if is_word(c) {
                start = i;
            } else {
                break;
            }
        }
        let mut end = cur;
        for (i, c) in text[cur..].char_indices() {
            if is_word(c) {
                end = cur + i + c.len_utf8();
            } else {
                break;
            }
        }
        (start < end).then_some((start, end))
    }
}
