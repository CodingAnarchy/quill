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
            // Smart typography stays on inside fenced blocks too — they hold
            // "log line"-style prose (transmissions, letters) that still wants
            // curly quotes and em-dashes, not literal source code (#522).
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

    /// Forward Delete (#641): remove the grapheme at the cursor, or join the next
    /// line up at a line end. Like a keystroke, this only records the edit.
    pub(super) fn edit_delete_forward(&mut self) {
        match &mut self.manuscript {
            Some(m) => m.delete_forward(),
            None => {
                self.buffer.delete_forward();
            }
        }
    }

    /// Delete the previous word (#641): Ctrl/Alt-Backspace.
    pub(super) fn edit_delete_word_back(&mut self) {
        match &mut self.manuscript {
            Some(m) => m.delete_word_back(),
            None => {
                self.buffer.delete_word_back();
            }
        }
    }

    /// Replace the active buffer's byte range `[lo, hi)` with `text` (#700) — the
    /// range-edit path shared by the Vim operators and find/replace. In the
    /// continuous view it routes through [`Manuscript::replace_byte_range`], which
    /// clamps the range to the owning chapter and keeps its seam bookkeeping
    /// correct; in the single-chapter view it's the buffer's own range edit.
    /// Returns whether any text actually changed (a fully-clamped seam edit is a
    /// no-op).
    pub(super) fn edit_replace_range(&mut self, lo: usize, hi: usize, text: &str) -> bool {
        if let Some(m) = &mut self.manuscript {
            m.replace_byte_range(lo, hi, text)
        } else {
            self.buffer.replace_byte_range(lo, hi, text);
            true
        }
    }

    /// Move the cursor a screenful of visual rows up or down (#641) — PageUp /
    /// PageDown. Pages by the editor's interior height (keeping a line of
    /// context), reusing the wrap-aware per-row motions so it lands on a real
    /// column and pages the continuous window if it crosses a chapter.
    pub(super) fn edit_page(&mut self, down: bool) {
        let rows = self.viewport_height.saturating_sub(3).max(1) as usize;
        self.edit_move_wrapped(move |b, w| {
            for _ in 0..rows {
                if down {
                    b.move_down(w);
                } else {
                    b.move_up(w);
                }
            }
        });
    }

    /// Undo / redo one edit group (#145). Works in both views: the single-chapter
    /// buffer steps its own rope; the continuous view steps the windowed buffer and
    /// restores the per-chapter seam bookkeeping alongside it (#649), so an undo
    /// reverts the edit and writes back to the right chapter file.
    pub(super) fn history_step(&mut self, redo: bool) {
        let verb = if redo { "Redo" } else { "Undo" };
        // Snapshot any open edit group's annotations before stepping, so the
        // undo/redo stacks are aligned with the buffer's (#370).
        self.note_annotation_undo_point();
        let changed = match &mut self.manuscript {
            Some(m) if redo => m.redo(),
            Some(m) => m.undo(),
            None if redo => self.buffer.redo(),
            None => self.buffer.undo(),
        };
        if changed {
            self.restore_annotation_history(redo); // swap annotations with the rope
            self.cache_focus_words();
            self.message = None;
        } else {
            self.message = Some(format!("Nothing to {}.", verb.to_lowercase()));
        }
    }

    /// The editor's current wrap column (#143), in cells.
    pub(super) const fn wrap_w(&self) -> usize {
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
        if self
            .manuscript
            .as_ref()
            .is_some_and(|m| m.would_reslide(&self.project))
        {
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
        self.message = Some(match self.write_active() {
            Ok(()) => "Saved".into(),
            Err(e) => e,
        });
        self.finalize_save();
        // Run the on-save workflow trigger (#228). This is the explicit-save
        // command (Ctrl-S / `:w`); implicit flushes (chapter switch, leaving the
        // continuous view) save the buffer directly and intentionally don't fire
        // it. A no-op when this save is itself a workflow's own step — no recursion.
        self.trigger_on_save();
    }

    /// Write the active chapter (or the continuous window) to disk through the
    /// same atomic, front-matter-preserving path as Ctrl-S, with the persistence
    /// that travels with a save — annotations in lockstep (#302/#370), the
    /// word-count cache, and a clean sibling pane (#163). Returns the error
    /// message on failure. No status message and no on-save trigger, so it's
    /// shared by the explicit save (which adds those) and autosave (#409).
    fn write_active(&mut self) -> Result<(), String> {
        // The project is open read-only because another live instance holds it
        // (#871): refuse the write rather than race the owner's saves and clobber
        // the shared chapter files. The open-time warning already told the author.
        if self.project.is_read_only() {
            return Err(
                "Read-only: this project is open in another Quill instance; not saved.".to_string(),
            );
        }
        if let Some(mut m) = self.manuscript.take() {
            let result = m.save(&mut self.project);
            self.manuscript = Some(m);
            result.map_err(|e| format!("Save failed: {e}"))?;
            self.persist_annotations();
        } else {
            self.buffer
                .save()
                .map_err(|e| format!("Save failed: {e}"))?;
            self.cache_focus_words();
            self.refresh_split_sibling();
            self.persist_annotations();
        }
        Ok(())
    }

    /// Autosave on the idle timer (#409): when enabled and there are unsaved
    /// changes, save after the configured seconds of genuine idle. Driven from the
    /// event loop (like `tick_sprint`), so it catches an idle pause even with no
    /// keypress. Cheap and O(1) when off or already saved.
    pub(crate) fn tick_autosave(&mut self) {
        let secs = self.autosave_interval_secs;
        if secs == 0 || !self.edit_dirty() {
            return;
        }
        if self.last_activity.elapsed() >= std::time::Duration::from_secs(secs) {
            self.autosave();
            // Reset the idle window so a failed save doesn't retry every tick.
            self.last_activity = std::time::Instant::now();
        }
    }

    /// The autosave write: the same persistence as Ctrl-S minus the "Saved"
    /// message and the on-save trigger (so an autosave-driven workflow can't
    /// thrash, #228). On failure it stays dirty and surfaces the error, so a full
    /// disk or lost write permission can't fail silently every idle tick while the
    /// author keeps typing, trusting autosave (reliability audit).
    fn autosave(&mut self) {
        if let Err(e) = self.write_active() {
            self.message = Some(format!(
                "Autosave failed: {e} — save with Ctrl-S or check disk"
            ));
            return;
        }
        self.finalize_save();
    }

    /// The post-write tail shared by an explicit save and an autosave: a clean
    /// buffer means there's nothing left to recover (#263), and either way we
    /// checkpoint today's momentum to the history log.
    fn finalize_save(&mut self) {
        // A save is the one moment pure typing can change git status; flag it so
        // the next idle tick recomputes the count off-thread (#779), rather than
        // re-walking the tree on every idle burst.
        self.mark_vc_status_dirty();
        // A save reached disk: note that an automatic off-site backup is owed
        // (#887). Just sets a flag — the actual push is decided and spawned off the
        // save path by the cadence hook, so a save never touches the network.
        self.mark_backup_on_save();
        if !self.edit_dirty() {
            self.clear_recovery();
        }
        self.persist_snapshot();
        // Keep the resume point current on every save, so an unclean exit after a
        // save still reopens where the author was working (#588).
        self.persist_position();
    }
}
