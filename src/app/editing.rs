//! Continuous-manuscript view and edit dispatch — split out of `app/mod.rs` to keep it under the ~1,000-line
//! convention (#404). Pure move; `App`'s methods live across modules.

use super::*;

impl App {
    // --- Continuous-manuscript view ---------------------------------------

    /// Toggle between the single-chapter view and the continuous view.
    pub(super) fn toggle_continuous(&mut self) {
        if self.manuscript.is_some() {
            self.leave_continuous();
        } else {
            self.enter_continuous();
        }
    }

    /// Enter the continuous view: flush the single chapter, then open a window
    /// of chapters around it.
    pub(super) fn enter_continuous(&mut self) {
        if self.flush_active().is_err() {
            return;
        }
        self.close_split_if_any(); // the continuous view and a split can't share the editor (#163)
        match Manuscript::open(&self.project) {
            Ok(m) => {
                self.manuscript = Some(m);
                // A Visual selection's anchor is a byte in the single-chapter
                // buffer; it's meaningless against the manuscript window, so drop
                // back to the resting mode (#205).
                if matches!(self.mode, Mode::Visual { .. }) {
                    self.mode = self.editing_mode();
                }
                self.message = Some("Continuous view".into());
            }
            // A chapter in the window couldn't be read (#154) — stay in the
            // single-chapter view rather than show it blank.
            Err(e) => self.message = Some(format!("Couldn't open continuous view: {e}")),
        }
    }

    /// Leave the continuous view: save its chapters, then reopen the single
    /// chapter the cursor was in. Stays in continuous view if the save fails, so
    /// no edits are lost.
    pub(super) fn leave_continuous(&mut self) {
        let Some(mut m) = self.manuscript.take() else {
            return;
        };
        if let Err(e) = m.save(&mut self.project) {
            self.message = Some(format!("Save failed: {e}"));
            self.manuscript = Some(m);
            return;
        }
        // Flush the window's annotation offsets before dropping it (#370), so they
        // match the just-saved prose; the single-chapter reload re-anchors them.
        let bases: Vec<(String, usize)> = m
            .segment_spans()
            .into_iter()
            .map(|(idx, start, _)| (self.project.chapter_file(idx).to_string(), start))
            .collect();
        self.persist_annotations_to(&bases);
        self.project.set_active(m.current_chapter());
        match Buffer::from_path(self.project.active_path()) {
            Ok(buffer) => {
                self.buffer = buffer;
                self.message = Some("Chapter view".into());
            }
            Err(e) => self.message = Some(format!("Open failed: {e}")),
        }
    }

    // --- Edit dispatch (single chapter or continuous) ----------------------

    pub(super) fn edit_insert_char(&mut self, c: char) {
        // Smart typography (#296): substitute curly quotes / em-dash / ellipsis
        // for the punctuation just typed, keyed off the text before the cursor.
        // Only the prose-edit path runs this; prompts and titles insert verbatim.
        if self.smart_typography {
            // The substitution only inspects the last ≤2 chars before the cursor.
            let before = match &self.manuscript {
                Some(m) => m.buffer().text_before_cursor(2),
                None => self.buffer.text_before_cursor(2),
            };
            if let Some((delete, replacement)) = typography::substitute(c, &before) {
                for _ in 0..delete {
                    self.edit_backspace();
                }
                self.edit_insert_raw(replacement);
                return;
            }
        }
        self.edit_insert_raw(c);
    }

    /// Insert `c` verbatim into the active buffer (single or continuous), with no
    /// typography substitution — the raw path `edit_insert_char` falls back to.
    pub(super) fn edit_insert_raw(&mut self, c: char) {
        match &mut self.manuscript {
            Some(m) => m.insert_char(c),
            None => self.buffer.insert_char(c),
        }
    }

    pub(super) fn edit_backspace(&mut self) {
        match &mut self.manuscript {
            Some(m) => m.backspace(),
            None => self.buffer.backspace(),
        }
    }

    /// Undo / redo one edit group (#145). Scoped to the single-chapter view: the
    /// continuous view's segment bookkeeping isn't snapshotted, so history there
    /// is guarded with a message rather than a silent desync (a follow-up).
    pub(super) fn history_step(&mut self, redo: bool) {
        let verb = if redo { "Redo" } else { "Undo" };
        if self.manuscript.is_some() {
            self.message = Some(format!("{verb} isn't available in the continuous view yet — Ctrl-l to return."));
            return;
        }
        // Snapshot any open edit group's annotations before stepping, so the
        // undo/redo stacks are aligned with the buffer's (#370).
        self.note_annotation_undo_point();
        let changed = if redo { self.buffer.redo() } else { self.buffer.undo() };
        if changed {
            self.restore_annotation_history(redo); // swap annotations with the rope
            self.cache_focus_words();
            self.message = None;
        } else {
            self.message = Some(format!("Nothing to {}.", verb.to_lowercase()));
        }
    }

    /// The editor's current wrap column (#143), in cells.
    pub(super) fn wrap_w(&self) -> usize {
        self.wrap_width.get() as usize
    }

    /// Apply a wrap-aware cursor movement (up/down/home/end) at the current wrap
    /// width, paging the continuous view if the cursor crossed a chapter.
    pub(super) fn edit_move_wrapped(&mut self, mv: impl FnOnce(&mut Buffer, usize)) {
        let w = self.wrap_w();
        self.edit_move(move |b| mv(b, w));
    }

    /// Apply a cursor movement to the active edit buffer, then page the window
    /// if the continuous view's cursor crossed into another chapter.
    pub(super) fn edit_move(&mut self, mv: impl FnOnce(&mut Buffer)) {
        mv(self.edit_buffer_mut());
        // A reslide rebuilds the buffer, resetting every offset, so flush the
        // current annotation offsets to disk first — the next idle reloads and
        // re-translates them for the paged window (#370).
        if self.manuscript.as_ref().is_some_and(|m| m.would_reslide(&self.project)) {
            self.persist_annotations();
        }
        if let Some(mut m) = self.manuscript.take() {
            // reslide flushes pending edits then pages the window; either the save
            // or a chapter read (#154) can fail. The window is left intact on a
            // failed read, so the view simply doesn't page.
            if let Err(e) = m.reslide(&mut self.project) {
                self.message = Some(format!("Continuous view: {e}"));
            }
            self.manuscript = Some(m);
        }
    }

    /// Save the active edit target (the continuous window, or the single chapter).
    pub(super) fn save_edit(&mut self) {
        if let Some(mut m) = self.manuscript.take() {
            let result = m.save(&mut self.project);
            self.manuscript = Some(m);
            if result.is_ok() {
                self.persist_annotations(); // keep notes in lockstep with the saved prose (#370)
            }
            self.message = Some(match result {
                Ok(()) => "Saved".into(),
                Err(e) => format!("Save failed: {e}"),
            });
        } else {
            match self.buffer.save() {
                Ok(()) => {
                    self.cache_focus_words();
                    self.refresh_split_sibling(); // a clean sibling on this chapter reflects the save (#163)
                    // Keep annotations.toml in lockstep with the saved prose (#302),
                    // so the anchors match the bytes on disk.
                    self.persist_annotations();
                    self.message = Some("Saved".into());
                }
                Err(e) => self.message = Some(format!("Save failed: {e}")),
            }
        }
        // A clean save means there's nothing left to recover (#263).
        if !self.edit_dirty() {
            self.clear_recovery();
        }
        // Checkpoint today's momentum to the history log.
        self.persist_snapshot();
        // Run the on-save workflow trigger (#228). This is the explicit-save
        // command (Ctrl-S / `:w`); implicit flushes (chapter switch, leaving the
        // continuous view) save the buffer directly and intentionally don't fire
        // it. A no-op when this save is itself a workflow's own step — no recursion.
        self.trigger_on_save();
    }

}
