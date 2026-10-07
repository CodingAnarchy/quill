//! Continuous-manuscript view and edit dispatch — split out of `app/mod.rs` to keep it under the ~1,000-line
//! convention (#404). Pure move; `App`'s methods live across modules.

use super::*;

use unicode_width::UnicodeWidthStr;

use crate::export::is_thematic_break;

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
        // The continuous view and a split can't share the editor (#163); stay in the
        // single-chapter view if the other pane wouldn't save (#1188).
        if self.close_split_if_any().is_err() {
            return;
        }
        match Manuscript::open(&self.project) {
            Ok(m) => {
                self.manuscript = Some(m);
                // Folds are keyed on the single-chapter buffer's byte offsets (#975);
                // the continuous window is a different buffer, so clear them.
                self.folds.clear();
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
        // Fold the burst's pending edits into the live annotation offsets before we
        // persist them below (#302/#370): leaving the view is a keystroke-path save,
        // and the deltas since the last idle tick aren't in `self.annotations` yet —
        // without this the flush would write the pre-edit offsets, drifting each
        // note by the burst's net delta (#15). Re-anchoring while the manuscript is
        // still loaded keeps the offsets buffer-absolute against the same window the
        // bases below are taken from.
        self.reanchor_annotations();
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
                // The reopened single chapter is a fresh buffer; drop any folds keyed
                // on the continuous window's byte offsets (#975).
                self.folds.clear();
                self.message = Some("Chapter view".into());
            }
            Err(e) => self.message = Some(format!("Open failed: {e}")),
        }
    }

    // --- Edit dispatch (single chapter or continuous) ----------------------

    pub(super) fn edit_insert_char(&mut self, c: char) {
        // Resolve the character actually going into the buffer, applying smart
        // typography (#296) first so auto-pair composes on the oriented glyph.
        // Smart typography substitutes curly quotes / em-dash / ellipsis for the
        // punctuation just typed, keyed off the text before the cursor. Only the
        // prose-edit path runs this; prompts and titles insert verbatim.
        let mut delete = 0usize;
        let mut effective = c;
        if self.flags.smart_typography {
            // The substitution only inspects the last ≤2 chars before the cursor.
            // Smart typography stays on inside fenced blocks too — they hold
            // "log line"-style prose (transmissions, letters) that still wants
            // curly quotes and em-dashes, not literal source code (#522).
            let before = match &self.manuscript {
                Some(m) => m.buffer().text_before_cursor(2),
                None => self.buffer.text_before_cursor(2),
            };
            if let Some((d, replacement)) = typography::substitute(c, &before) {
                delete = d;
                effective = replacement;
            }
        }

        // Auto-pair (#1007): typing a closer that's already at the cursor skips
        // past it instead of duplicating. A substitution that consumes preceding
        // characters (`--` → em-dash, `...` → ellipsis) is never a closer, so this
        // only fires for the single-char quote/bracket glyphs. O(1): it inspects
        // only the one character immediately at the cursor, never scanning.
        if self.flags.auto_pair && delete == 0 && is_auto_pair_closer(effective) {
            let at_cursor = match &self.manuscript {
                Some(m) => m.buffer().char_after_cursor(),
                None => self.buffer.char_after_cursor(),
            };
            if at_cursor == Some(effective) {
                self.edit_move(Buffer::move_right);
                return;
            }
        }

        // Apply the (possibly substituted) insertion.
        for _ in 0..delete {
            self.edit_backspace();
        }
        self.edit_insert_raw(effective);

        // Auto-pair (#1007): typing an opener drops in its matching closer and puts
        // the cursor back between the pair. O(1) — insert one char and move left.
        // The closer goes straight onto the buffer (not through `edit_insert_raw`):
        // the opener already recorded the insertion, and the cursor ends up *before*
        // the closer, so recording the closer's end byte would leave a stale
        // edit-ring spot past the cursor that a later insert would invalidate.
        if self.flags.auto_pair && delete == 0 {
            if let Some(closer) = auto_pair_closer(effective) {
                if let Some(m) = &mut self.manuscript {
                    m.insert_char(closer);
                    m.buffer_mut().move_left();
                } else {
                    self.buffer.insert_char(closer);
                    self.buffer.move_left();
                }
            }
        }
    }

    /// Insert `c` verbatim into the active buffer (single or continuous), with no
    /// typography substitution — the raw path `edit_insert_char` falls back to.
    pub(super) fn edit_insert_raw(&mut self, c: char) {
        match &mut self.manuscript {
            Some(m) => m.insert_char(c),
            None => self.buffer.insert_char(c),
        }
        // Record the insertion onto the recent-edit ring and as the `gi` target
        // (#1010); coalesces per line, so a typed sentence is one spot.
        self.record_insertion();
    }

    pub(super) fn edit_backspace(&mut self) {
        match &mut self.manuscript {
            Some(m) => m.backspace(),
            None => self.buffer.backspace(),
        }
        self.record_edit_location(); // a deletion is an edit spot too (#1010)
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
        self.record_edit_location(); // (#1010)
    }

    /// Delete the previous word (#641): Ctrl/Alt-Backspace.
    pub(super) fn edit_delete_word_back(&mut self) {
        match &mut self.manuscript {
            Some(m) => m.delete_word_back(),
            None => {
                self.buffer.delete_word_back();
            }
        }
        self.record_edit_location(); // (#1010)
    }

    /// Replace the active buffer's byte range `[lo, hi)` with `text` (#700) — the
    /// range-edit path shared by the Vim operators and find/replace. In the
    /// continuous view it routes through [`Manuscript::replace_byte_range`], which
    /// clamps the range to the owning chapter and keeps its seam bookkeeping
    /// correct; in the single-chapter view it's the buffer's own range edit.
    /// Returns whether any text actually changed (a fully-clamped seam edit is a
    /// no-op).
    pub(super) fn edit_replace_range(&mut self, lo: usize, hi: usize, text: &str) -> bool {
        let changed = if let Some(m) = &mut self.manuscript {
            m.replace_byte_range(lo, hi, text)
        } else {
            self.buffer.replace_byte_range(lo, hi, text);
            true
        };
        // A range edit (an operator's change/delete, find-and-replace) is an edit
        // spot too (#1010); a replacement that inserted text also updates the `gi`
        // target. A fully-clamped seam no-op recorded nothing, so skip it.
        if changed {
            if text.is_empty() {
                self.record_edit_location();
            } else {
                self.record_insertion();
            }
        }
        changed
    }

    // --- Join and reflow (#1005) -------------------------------------------

    /// Vim `J`: merge the current line with the next, normalizing the space
    /// between them (#1005). With a count, joins that many lines (a count under 2
    /// still joins one, as in Vim). Only whitespace changes — the current line's
    /// trailing whitespace and the next line's leading whitespace collapse to a
    /// single space (none when the current line ends blank), never adding,
    /// removing, or reordering a word. One undoable edit through the range-edit
    /// path, so it inherits the seam bookkeeping (#700) and is `.`-repeatable
    /// (#1004). Respects prose boundaries: it won't merge a blank line or a scene
    /// break (a thematic break, #908) into the line, so paragraphs and scenes stay
    /// separate.
    pub(super) fn join_lines(&mut self, count: usize) {
        // Vim joins `count` lines, meaning `count - 1` newlines removed; a bare
        // `J` (count 1) and `2J` both fold one following line into the current.
        let joins = count.saturating_sub(1).max(1);
        let start_line = self.edit_buffer().cursor.line;
        // Build the merged text by folding each following line up in turn, stopping
        // at a boundary. `end_line` is the last line consumed; the replaced range
        // runs from the current line's start to just past `end_line`.
        // In the continuous view, clamp the forward scan to the current chapter's
        // segment (#1005 follow-up): chapters are concatenated with a single `\n`
        // seam and no blank line, so without this the join would fold the next
        // chapter's first line up across the seam — and since the range edit is
        // clamped to the chapter, that text would be duplicated.
        let scan_last = self.paragraph_scan_last(start_line);
        let (merged, end_line) = {
            let buf = self.edit_buffer();
            let mut merged = buf.line(start_line);
            let mut end_line = start_line;
            for _ in 0..joins {
                let next = end_line + 1;
                if next > scan_last {
                    break; // no line below to join — chapter/document end
                }
                let below = buf.line(next);
                let below_t = below.trim();
                // A blank line is a paragraph boundary, a thematic break is a scene
                // divider (#908), and an ATX heading is a section boundary: never
                // fold any of them into the prose above it.
                if below_t.is_empty() || is_thematic_break(below_t) || is_atx_heading(below_t) {
                    break;
                }
                // Likewise never join out of a scene break or heading itself.
                let merged_t = merged.trim();
                if is_thematic_break(merged_t) || is_atx_heading(merged_t) {
                    break;
                }
                merged = join_two(&merged, &below);
                end_line = next;
            }
            (merged, end_line)
        };
        if end_line == start_line {
            return; // nothing below to join — leave the buffer untouched
        }
        let lo = self.edit_buffer().line_start_byte(start_line);
        let (hi, append_newline) = self.paragraph_replace_tail(end_line);
        let replacement = if append_newline {
            format!("{merged}\n")
        } else {
            merged
        };
        self.edit_replace_range(lo, hi, &replacement);
        self.edit_move(|b| b.move_to_line(start_line));
        self.cache_focus_words();
    }

    /// Vim `gq`-style reflow: re-wrap the paragraph around the cursor to the
    /// editor's wrap column (#1005), collapsing the paragraph's internal hard line
    /// breaks into flowed text and re-breaking at word boundaries. Only whitespace
    /// and line breaks change — every word is preserved byte-for-byte in order. One
    /// undoable edit through the range-edit path (#700), so it's `.`-repeatable
    /// (#1004). A paragraph is a run of non-blank lines bounded by blank lines and
    /// scene breaks (thematic breaks, #908), so reflow never pulls text across a
    /// paragraph or scene boundary.
    pub(super) fn reflow_paragraph(&mut self) {
        let cur = self.edit_buffer().cursor.line;
        let Some((first, last)) = self.paragraph_line_span(cur) else {
            return; // the cursor is on a blank line or a scene break — nothing to reflow
        };
        // Gather the paragraph's words in order, flattening its hard breaks.
        let words: Vec<String> = {
            let buf = self.edit_buffer();
            (first..=last)
                .flat_map(|i| {
                    buf.line(i)
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        if words.is_empty() {
            return;
        }
        let wrapped = wrap_words(&words, self.wrap_w());
        let lo = self.edit_buffer().line_start_byte(first);
        let (hi, append_newline) = self.paragraph_replace_tail(last);
        let replacement = if append_newline {
            format!("{wrapped}\n")
        } else {
            wrapped
        };
        self.edit_replace_range(lo, hi, &replacement);
        self.edit_move(|b| b.move_to_line(first));
        self.cache_focus_words();
    }

    /// The inclusive line span `[first, last]` of the prose paragraph containing
    /// `line` — the run of non-blank, non-scene-break lines around it (#1005).
    /// Returns `None` when `line` is itself blank or a scene break, so join/reflow
    /// stay off the boundaries. Walks outward line by line (windowed), stopping at a
    /// blank line or a thematic break (#908) in either direction.
    fn paragraph_line_span(&self, line: usize) -> Option<(usize, usize)> {
        let buf = self.edit_buffer();
        let last = buf.line_count().saturating_sub(1);
        let cur = line.min(last);
        // In the continuous view the paragraph stops at the chapter seam in *both*
        // directions (#1005 follow-up), so reflow never gathers an adjacent
        // chapter's line across it — the concatenation seam is a single `\n` with no
        // blank line, which the blank/thematic-break check alone would miss. The
        // forward walk clamps to `scan_last` (the chapter's last line) and the
        // backward walk to `scan_first` (its first line); without the latter, a
        // reflow in the *first* paragraph of a non-first chapter would walk back
        // across the seam into the previous chapter — the mirror of the forward bug.
        let scan_last = self.paragraph_scan_last(cur);
        let is_boundary = |i: usize| {
            let t = buf.line(i);
            let t = t.trim();
            // Blank line, scene divider (#908), or ATX heading (#1005 follow-up) —
            // each bounds the paragraph, so a heading directly above prose is never
            // folded in.
            t.is_empty() || is_thematic_break(t) || is_atx_heading(t)
        };
        if is_boundary(cur) {
            return None;
        }
        let scan_first = self.paragraph_scan_first(cur);
        let mut first = cur;
        while first > scan_first && !is_boundary(first - 1) {
            first -= 1;
        }
        let mut end = cur;
        while end < scan_last && !is_boundary(end + 1) {
            end += 1;
        }
        Some((first, end))
    }

    /// The last line a forward-scanning join/reflow may reach from `line` (#1005
    /// follow-up). In the single-chapter view that's the buffer's final line; in
    /// the continuous view it's clamped to the end of the chapter segment holding
    /// `line`, so the scan never crosses a chapter seam.
    fn paragraph_scan_last(&self, line: usize) -> usize {
        let buf_last = self.edit_buffer().line_count().saturating_sub(1);
        self.manuscript
            .as_ref()
            .map_or(buf_last, |m| m.segment_end_line(line).min(buf_last))
    }

    /// The first line a backward-scanning join/reflow may reach from `line`, the
    /// mirror of [`paragraph_scan_last`](Self::paragraph_scan_last). In the
    /// single-chapter view that's line 0; in the continuous view it's clamped to
    /// the start of the chapter segment holding `line`, so the scan never crosses a
    /// chapter seam backward into the previous chapter.
    ///
    /// `Manuscript::segment_start_line` is a private, segment-indexed helper, so
    /// the segment start is derived here from the public, line-indexed
    /// [`Manuscript::segment_end_line`]: it is constant across a segment, so
    /// stepping back while the line above still belongs to the same segment (same
    /// end line) lands on the segment's first line.
    fn paragraph_scan_first(&self, line: usize) -> usize {
        let Some(m) = self.manuscript.as_ref() else {
            return 0;
        };
        let seg_end = m.segment_end_line(line);
        let mut first = line.min(seg_end);
        while first > 0 && m.segment_end_line(first - 1) == seg_end {
            first -= 1;
        }
        first
    }

    /// The end byte `hi` of the replaced range for a join/reflow whose last line is
    /// `end_line`, and whether the replacement should re-append a trailing newline
    /// (#1005 follow-up). Both are derived structurally, never by slicing the byte
    /// before `hi` — that byte can be a UTF-8 continuation byte when the final
    /// grapheme is multibyte (em-dash, curly quote, accent, emoji), and ropey
    /// panics on a non-char-boundary slice.
    ///
    /// - In the single-chapter view (and for a non-final chapter line), the range
    ///   runs to just past `end_line` (including its `\n`) and the replacement
    ///   re-appends the newline — unless `end_line` is the buffer's final line with
    ///   no trailing newline, in which case there's nothing to re-append.
    /// - In the continuous view, when `end_line` is a chapter segment's last line
    ///   (and not the buffer's final line) its trailing `\n` is the chapter seam.
    ///   [`Manuscript::replace_byte_range`] preserves that seam newline by clamping
    ///   the range back off it, so the range must stop *before* the seam and the
    ///   replacement must not re-append a newline — otherwise the seam's `\n` and a
    ///   re-appended `\n` would stack into a spurious blank line.
    fn paragraph_replace_tail(&self, end_line: usize) -> (usize, bool) {
        let buf = self.edit_buffer();
        let buf_last = buf.line_count().saturating_sub(1);
        let next_line = buf.next_line_byte(end_line);
        if end_line >= buf_last {
            // Buffer's final line: it carries a trailing `\n` only if the whole
            // buffer does. `next_line_byte` already excludes any absent newline.
            return (next_line, buf.text().ends_with('\n'));
        }
        // A non-final line always ends in `\n`. In the continuous view, if this is a
        // chapter seam the clamp keeps that newline, so stop the range before it and
        // don't re-append.
        if self
            .manuscript
            .as_ref()
            .is_some_and(|m| m.segment_end_line(end_line) == end_line)
        {
            return (next_line.saturating_sub(1), false);
        }
        (next_line, true)
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
        // Remember where the cursor sat before the move: if the move crosses a
        // chapter boundary but the reslide then fails, the window can't page and
        // the cursor would be stranded across the seam, so we rewind to here
        // (#892).
        let before = self.edit_buffer().cursor;
        mv(self.edit_buffer_mut());
        // A reslide rebuilds the buffer, resetting every offset. Re-anchor the live
        // annotations to the buffer as it stands (folding in the burst's pending
        // edits, #893) and flush them to disk *before* the rebuild, so we never
        // persist a stale offset (#302/#370). `would_reslide` is checked once and
        // reused below so the after-rebuild reload runs on exactly the moves that
        // paged the window.
        let will_reslide = self
            .manuscript
            .as_ref()
            .is_some_and(|m| m.would_reslide(&self.project));
        if will_reslide {
            self.reanchor_annotations();
            self.persist_annotations();
        }
        if let Some(mut m) = self.manuscript.take() {
            // reslide flushes pending edits then pages the window; either the save
            // or a chapter read (#154) can fail. On a failed read the window is
            // left intact — but the cursor has already crossed into the chapter
            // that couldn't page in, so it now straddles unloaded/stale space.
            // Rewind it to the pre-move position (still valid against the intact
            // window) so the visible buffer stays consistent and the next
            // keystroke can't land in a stale chapter or blindly re-trigger the
            // failing read; once the file reads again the same move reslides
            // normally (#892).
            if let Err(e) = m.reslide(&mut self.project) {
                m.buffer_mut().cursor = before;
                self.message = Some(format!("Continuous view: {e}"));
                self.manuscript = Some(m);
            } else {
                self.manuscript = Some(m);
                // The window paged: re-anchor the notes to the new segment bases
                // now — the reconciliation otherwise deferred to the idle tick — so
                // a *second* reslide in the same input burst persists against the
                // current offsets rather than the previous window's (#302/#370).
                if will_reslide {
                    self.reload_annotations_for_view();
                }
            }
        }
        // A motion can carry the cursor into a collapsed section of the current
        // chapter (e.g. Down from a folded heading into its hidden body). Reveal the
        // fold the cursor entered so it never lands on a zero-row hidden line — the
        // renderer's cursor-follow would otherwise never place the terminal caret and
        // keystrokes would insert blind into the invisible body (#975). The
        // continuous view clears folds, so this is a cheap no-op there.
        self.reveal_fold_at_cursor();
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
            self.adopt_view_annotations_if_stale();
            self.persist_annotations();
        } else {
            self.buffer
                .save()
                .map_err(|e| format!("Save failed: {e}"))?;
            self.cache_focus_words();
            self.refresh_split_sibling();
            // A save in the same burst as a chapter switch must persist the new
            // chapter's notes, not the previous chapter's set (which has none).
            self.adopt_view_annotations_if_stale();
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

/// The opening curly double quote `“` — smart typography's oriented open-quote
/// glyph (#296), an auto-pair opener whose closer is the closing curly `”`.
const LDQUO: char = '\u{201C}';
/// The closing curly double quote `”` — smart typography's oriented close-quote
/// glyph, and the closer auto-pair inserts (and skips over) for an open curly.
const RDQUO: char = '\u{201D}';

/// The closer auto-pair (#1007) inserts for the opener `c`, or `None` when `c`
/// isn't an opener. Covers the straight quote and the two bracket pairs, plus the
/// oriented open curly quote (`“`) so it composes with smart typography (#296): a
/// `"` becomes `“…”` with the cursor between. O(1) — a small `match`, no scan.
const fn auto_pair_closer(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '"' => Some('"'),
        LDQUO => Some(RDQUO),
        _ => None,
    }
}

/// Whether `c` is a closer auto-pair (#1007) skips over when it's already at the
/// cursor — the mirror of [`auto_pair_closer`]'s targets, including the oriented
/// close curly quote (`”`) so over-typing a smart-typography closer skips too.
const fn is_auto_pair_closer(c: char) -> bool {
    matches!(c, ')' | ']' | '"' | RDQUO)
}

/// Join two lines with a single normalized separator (#1005): the current line's
/// trailing whitespace and the next line's leading whitespace collapse to one
/// space. An empty (or all-blank) current line takes the next line's content with
/// no leading space, so a run of joins doesn't accrete leading spaces. No word is
/// added, removed, or reordered — only the whitespace between them changes.
fn join_two(current: &str, next: &str) -> String {
    let head = current.trim_end();
    let tail = next.trim_start();
    if head.is_empty() {
        tail.to_owned()
    } else if tail.is_empty() {
        head.to_owned()
    } else {
        format!("{head} {tail}")
    }
}

/// Greedy word-wrap `words` to `width` columns (#1005), one space between words.
/// Words are placed verbatim in order — a word longer than `width` still gets its
/// own line rather than being split, so no word is ever broken. A zero width falls
/// back to one word per line (fully flowed onto separate lines is still only a
/// whitespace change). Every word is preserved byte-for-byte.
fn wrap_words(words: &[String], width: usize) -> String {
    let mut out = String::new();
    let mut col = 0usize; // display width of the current line so far
    for word in words {
        let w = UnicodeWidthStr::width(word.as_str());
        if col == 0 {
            out.push_str(word);
            col = w;
        } else if col + 1 + w <= width {
            out.push(' ');
            out.push_str(word);
            col += 1 + w;
        } else {
            out.push('\n');
            out.push_str(word);
            col = w;
        }
    }
    out
}

/// Whether `line` (already trimmed of surrounding whitespace) is a Markdown ATX
/// heading (#1005 follow-up): 1–6 leading `#` followed by a space or the end of
/// the line. Join/reflow treat a heading as a paragraph boundary so `## Scene`
/// directly above prose (no blank line) is never folded into the prose line.
fn is_atx_heading(trimmed: &str) -> bool {
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&hashes)
        && trimmed[hashes..]
            .chars()
            .next()
            .is_none_or(|c| c == ' ' || c == '\t')
}
