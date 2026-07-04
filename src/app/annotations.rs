//! Inline author annotations (#302): create a note anchored to a selection (or
//! the word at the cursor), keep it on its phrase as the prose is edited, and
//! review / jump-to / delete the chapter's notes.
//!
//! Annotations are held as live buffer ranges in [`App::annotations`], each tagged
//! with its chapter — buffer-local in the single-chapter view, buffer-absolute
//! (offset by the owning segment) in the continuous view (#370). They're
//! re-anchored on the idle tick from the buffer's edit log
//! ([`Buffer::drain_anchor_edits`]), reloaded and re-translated when the view's
//! layout changes, restored alongside undo/redo, and persisted per chapter to
//! `annotations.toml` (see [`crate::project`]) so the chapter `.md` stays clean
//! and exports never carry them.

use crossterm::event::{KeyCode, KeyEvent};

use super::App;
use crate::annotation::{remap, Live};
use crate::mode::{Mode, Pending};

impl App {
    /// Reconcile annotations with the buffer on the idle tick (#302/#370): reload
    /// and re-translate when the view's layout changes (chapter switch, continuous
    /// window page, view toggle), otherwise shift each note through the edits since
    /// the last tick. Works in both views — single-chapter notes are buffer-local;
    /// continuous notes are buffer-absolute, offset by their segment.
    ///
    /// Undo/redo is handled separately: a rope swap carries no edit delta, so
    /// annotations are snapshotted alongside the buffer's undo stack and restored,
    /// rather than re-mapped here.
    pub(crate) fn sync_annotations(&mut self) {
        // Snapshot annotations for any edit group that opened, before the deltas
        // below move them — keeps the undo history aligned with the buffer's (#370).
        self.note_annotation_undo_point();

        let edits = self.edit_buffer_mut().drain_anchor_edits();

        // A layout change (different chapter / window / view) means the buffer was
        // rebuilt: reload from disk and re-translate, discarding the stale deltas.
        let context = self.annotation_context();
        if context != self.annotations_context {
            self.annotations = self.load_annotations_for_view();
            self.annotations_context = context;
            self.clear_annotation_history(); // a fresh buffer has no undo history
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

    /// A signature of the current view's chapter layout (#370): the active file in
    /// the single view, the loaded-chapter indices in the continuous view. Stable
    /// across prose edits; changes when the buffer is rebuilt around a new layout.
    fn annotation_context(&self) -> String {
        self.manuscript.as_ref().map_or_else(
            || format!("S:{}", self.project.active_file()),
            |m| {
                let ids: Vec<String> = m.loaded_chapters().iter().map(usize::to_string).collect();
                format!("C:{}", ids.join(","))
            },
        )
    }

    /// Load every loaded chapter's annotations for the current view (#370),
    /// translated to live buffer offsets — buffer-local in the single view, offset
    /// by the owning segment's start in the continuous view.
    fn load_annotations_for_view(&self) -> Vec<crate::annotation::Live> {
        let mut out = Vec::new();
        for (file, base) in self.chapter_bases() {
            for a in self.project.annotations_for(&file) {
                out.push(Live {
                    chapter: file.clone(),
                    start: base + a.start,
                    end: base + a.end,
                    note: a.note,
                });
            }
        }
        out.sort_by_key(|a| (a.start, a.end));
        out
    }

    /// Each loaded chapter's `(file, base_byte)` for the current view (#370): the
    /// active chapter at 0 in the single view; each segment's chapter and start
    /// byte in the continuous view. The seam for translating annotation offsets.
    fn chapter_bases(&self) -> Vec<(String, usize)> {
        self.manuscript.as_ref().map_or_else(
            || vec![(self.project.active_file().to_string(), 0)],
            |m| {
                m.segment_spans()
                    .into_iter()
                    .map(|(idx, start, _end)| (self.project.chapter_file(idx).to_string(), start))
                    .collect()
            },
        )
    }

    /// The chapter `(file, base_byte)` that owns a buffer byte (#370) — the active
    /// chapter in the single view, or the segment containing `byte` in continuous.
    fn owning_chapter(&self, byte: usize) -> (String, usize) {
        if let Some(m) = &self.manuscript {
            let spans = m.segment_spans();
            for &(idx, start, end) in &spans {
                if byte >= start && byte < end {
                    return (self.project.chapter_file(idx).to_string(), start);
                }
            }
            // Byte at the very end maps to the last segment.
            if let Some(&(idx, start, _)) = spans.last() {
                return (self.project.chapter_file(idx).to_string(), start);
            }
        }
        (self.project.active_file().to_string(), 0)
    }

    /// Snapshot the current annotations when a new undo group has opened (#370),
    /// so the app's annotation history stays one-entry-per-group aligned with the
    /// buffer's undo stack. Works in the continuous view too (#649): its notes are
    /// buffer-absolute [`Live`] ranges, so a snapshot restores them the same way an
    /// undo restores the rope, keeping them anchored across the windowed buffer.
    pub(crate) fn note_annotation_undo_point(&mut self) {
        let opened = self.edit_buffer_mut().take_checkpointed();
        if !opened {
            return;
        }
        self.anno_undo.push_back(self.annotations.clone());
        while self.anno_undo.len() > crate::buffer::Buffer::MAX_UNDO_GROUPS {
            self.anno_undo.pop_front();
        }
        self.anno_redo.clear(); // a new edit invalidates redo, mirroring the buffer
    }

    /// Swap the live annotations to match an undo/redo of the rope (#370): pop the
    /// pre-step snapshot off the matching stack and push the current onto the
    /// other, the exact mirror of the buffer's `step`.
    pub(crate) fn restore_annotation_history(&mut self, redo: bool) {
        let current = self.annotations.clone();
        let popped = if redo {
            self.anno_redo.pop_back()
        } else {
            self.anno_undo.pop_back()
        };
        if let Some(prev) = popped {
            if redo {
                self.anno_undo.push_back(current);
            } else {
                self.anno_redo.push_back(current);
            }
            self.annotations = prev;
        }
    }

    /// Drop the annotation undo/redo history — when the edit buffer is replaced
    /// (chapter switch, continuous view, crash recovery), its undo stack resets
    /// too (#370).
    pub(crate) fn clear_annotation_history(&mut self) {
        self.anno_undo.clear();
        self.anno_redo.clear();
    }

    /// Annotated byte ranges to mark in the editor, clamped to the buffer so a
    /// stale offset (e.g. after an undo) never indexes past the end. Buffer-
    /// absolute, so it marks the right spans in both the single and continuous
    /// views (#370).
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
            None => {
                self.message =
                    Some("Select text or place the cursor on a word to annotate.".into());
            }
        }
    }

    /// Open the note prompt for the byte range `[start, end)` (#370: in either
    /// view — the range is a buffer offset, anchored to its owning chapter).
    pub(crate) fn start_annotation_range(&mut self, start: usize, end: usize) {
        if start >= end {
            self.message = Some("Select text or place the cursor on a word to annotate.".into());
            return;
        }
        let snippet = self.edit_buffer().byte_slice(start, end);
        let label = format!("Note on \"{}\": ", crate::text::ellipsize(&snippet, 30));
        self.open_prompt(Pending::Annotate { start, end }, label, "", None);
    }

    /// Create the annotation for a confirmed note (#302). An empty note cancels.
    /// The range is buffer-absolute; it's tagged with the chapter that owns it
    /// (#370) so it persists to the right file in either view.
    #[allow(clippy::needless_pass_by_value)] // callers (prompts.rs, tests) pass owned notes
    pub(crate) fn create_annotation(&mut self, start: usize, end: usize, note: String) {
        let note = note.trim().to_string();
        if note.is_empty() {
            return;
        }
        let (chapter, _base) = self.owning_chapter(start);
        self.annotations.push(Live {
            chapter,
            start,
            end,
            note,
        });
        self.annotations.sort_by_key(|a| (a.start, a.end));
        self.persist_annotations();
        self.message = Some("Annotation added.".into());
    }

    /// Open the annotation review list for the loaded chapter(s) (#370: the active
    /// chapter, or the whole continuous window), or report when there are none.
    pub(crate) fn open_annotation_review(&mut self) {
        if self.annotations.is_empty() {
            self.message = Some("No annotations here.".into());
            return;
        }
        self.mode = Mode::Annotations {
            selected: 0,
            filter: String::new(),
            typing: false,
        };
    }

    /// Key handling for the annotation review list: move (skipping filtered-out
    /// notes), jump (Enter), delete (`d`), close (Esc). `/` starts a type-to-filter
    /// query (#822) that narrows the list to notes whose text or anchored snippet
    /// match; Esc clears the filter before it closes the list.
    pub(crate) fn on_key_annotations(&mut self, key: KeyEvent) {
        let Mode::Annotations {
            selected, typing, ..
        } = self.mode
        else {
            return;
        };
        // In the filter-input sub-mode, keystrokes build the query (#822); the
        // list's single-key commands are reachable only outside it.
        if typing {
            self.on_key_annotations_filter(key);
            return;
        }
        // Only act on the selection when it passes the active filter (#822).
        let acts_on_selected =
            selected < self.annotations.len() && self.annotation_query_matches(selected);
        match key.code {
            KeyCode::Char('/') => self.set_annotations_typing(true),
            KeyCode::Up | KeyCode::Char('k') => {
                let s = self.annotation_step(false);
                self.set_annotations_selected(s);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let s = self.annotation_step(true);
                self.set_annotations_selected(s);
            }
            KeyCode::Enter if acts_on_selected => {
                let start = self.annotations.get(selected).map(|a| a.start);
                self.mode = self.editing_mode();
                if let Some(start) = start {
                    let buf = self.edit_buffer_mut();
                    let at = start.min(buf.len_bytes());
                    buf.cursor = buf.cursor_at_byte(at);
                }
            }
            KeyCode::Char('d') if acts_on_selected => {
                self.annotations.remove(selected);
                self.persist_annotations();
                if self.annotations.is_empty() {
                    self.mode = self.editing_mode();
                } else {
                    let s = selected.min(self.annotations.len() - 1);
                    self.set_annotations_selected(s);
                    self.annotation_snap();
                }
            }
            // Esc clears an active filter first (restoring the full list), then a
            // second Esc closes the list.
            KeyCode::Esc => {
                if self.annotation_has_filter() {
                    self.clear_annotation_filter();
                } else {
                    self.mode = self.editing_mode();
                }
            }
            _ => {}
        }
    }

    /// Keys for the annotation list's filter-input sub-mode (#822): printable
    /// characters build the query (live), Backspace edits it, the arrows move
    /// within the matches, Enter/Esc leave input — keeping the filter so the
    /// list's commands act on the narrowed set (a second Esc, in command mode,
    /// clears it).
    fn on_key_annotations_filter(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.set_annotations_typing(false),
            KeyCode::Up => {
                let s = self.annotation_step(false);
                self.set_annotations_selected(s);
            }
            KeyCode::Down => {
                let s = self.annotation_step(true);
                self.set_annotations_selected(s);
            }
            KeyCode::Backspace => self.annotation_filter_pop(),
            KeyCode::Char(c) => self.annotation_filter_push(c),
            _ => {}
        }
    }

    /// The active annotation filter, lowercased (empty off the list/unfiltered).
    fn annotation_query(&self) -> String {
        match &self.mode {
            Mode::Annotations { filter, .. } => filter.to_lowercase(),
            _ => String::new(),
        }
    }

    const fn annotation_has_filter(&self) -> bool {
        matches!(&self.mode, Mode::Annotations { filter, .. } if !filter.is_empty())
    }

    /// Whether the annotation at `index` passes the active filter (#822): a
    /// case-insensitive substring match on its note text or the snippet of prose it
    /// anchors. Clamped to the buffer so a stale offset never indexes past the end.
    pub(crate) fn annotation_matches(&self, index: usize, q: &str) -> bool {
        if q.is_empty() {
            return true;
        }
        let Some(a) = self.annotations.get(index) else {
            return false;
        };
        if a.note.to_lowercase().contains(q) {
            return true;
        }
        let len = self.edit_buffer().len_bytes();
        let (s, e) = (a.start.min(len), a.end.min(len));
        s < e
            && self
                .edit_buffer()
                .byte_slice(s, e)
                .to_lowercase()
                .contains(q)
    }

    fn annotation_query_matches(&self, index: usize) -> bool {
        let q = self.annotation_query();
        self.annotation_matches(index, &q)
    }

    const fn set_annotations_typing(&mut self, on: bool) {
        if let Mode::Annotations { typing, .. } = &mut self.mode {
            *typing = on;
        }
    }

    const fn set_annotations_selected(&mut self, selected: usize) {
        if let Mode::Annotations { selected: s, .. } = &mut self.mode {
            *s = selected;
        }
    }

    fn clear_annotation_filter(&mut self) {
        if let Mode::Annotations { filter, typing, .. } = &mut self.mode {
            filter.clear();
            *typing = false;
        }
    }

    fn annotation_filter_push(&mut self, c: char) {
        if let Mode::Annotations { filter, .. } = &mut self.mode {
            filter.push(c);
        }
        self.annotation_snap();
    }

    fn annotation_filter_pop(&mut self) {
        if let Mode::Annotations { filter, .. } = &mut self.mode {
            filter.pop();
        }
        self.annotation_snap();
    }

    /// After the filter changes, keep the selection on a visible note: if the
    /// current one no longer matches, jump to the first that does (#822).
    fn annotation_snap(&mut self) {
        let q = self.annotation_query();
        let sel = match &self.mode {
            Mode::Annotations { selected, .. } => *selected,
            _ => return,
        };
        if !self.annotation_matches(sel, &q) {
            if let Some(first) =
                (0..self.annotations.len()).find(|&i| self.annotation_matches(i, &q))
            {
                self.set_annotations_selected(first);
            }
        }
    }

    /// The next (or previous, by `forward`) note that passes the filter, or the
    /// current selection unchanged when there's no further match that way (#822).
    fn annotation_step(&self, forward: bool) -> usize {
        let q = self.annotation_query();
        let from = match &self.mode {
            Mode::Annotations { selected, .. } => *selected,
            _ => return 0,
        };
        let n = self.annotations.len();
        let mut i = from;
        loop {
            if forward {
                if i + 1 >= n {
                    return from;
                }
                i += 1;
            } else {
                if i == 0 {
                    return from;
                }
                i -= 1;
            }
            if self.annotation_matches(i, &q) {
                return i;
            }
        }
    }

    /// Write the loaded chapters' live annotations to their side files (#302/#370):
    /// for each loaded chapter, the notes tagged to it, translated from
    /// buffer-absolute back to chapter-local offsets. Other chapters' notes are
    /// left untouched.
    pub(crate) fn persist_annotations(&mut self) {
        let bases = self.chapter_bases();
        self.persist_annotations_to(&bases);
    }

    /// Persist annotations for the chapters in `bases` (#370). Taken explicitly so
    /// the continuous-view seams can persist while the manuscript is moved out of
    /// `self` (save / reslide / leaving the view).
    pub(crate) fn persist_annotations_to(&mut self, bases: &[(String, usize)]) {
        for (file, base) in bases {
            let local: Vec<Live> = self
                .annotations
                .iter()
                .filter(|a| &a.chapter == file)
                .map(|a| Live {
                    chapter: file.clone(),
                    start: a.start.saturating_sub(*base),
                    end: a.end.saturating_sub(*base),
                    note: a.note.clone(),
                })
                .collect();
            if let Err(e) = self.project.set_chapter_annotations(file, &local) {
                self.message = Some(format!("Couldn't save annotations: {e}"));
            }
        }
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
