//! Find and find-and-replace over the active buffer (#144).
//!
//! Search runs against whichever buffer the author is editing — the single
//! chapter, or the continuous window's concatenated chapters (#700), so a find
//! or `/`-search no longer has to drop back to single-chapter view. Matches are
//! stored as byte ranges into that buffer and recomputed whenever the query or
//! the document changes; the editor highlights them and the cursor jumps to the
//! current one. Replace goes through the range-edit path (`edit_replace_range`),
//! so it is captured by undo (#145) and keeps the continuous view's per-chapter
//! seam bookkeeping correct.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::App;
use crate::buffer::TextObject;
use crate::mode::{Mode, Search, SearchStage, VimSearch};
use crate::text::{match_ranges, match_ranges_opts};

impl App {
    /// Open the find prompt (Ctrl-F). Works in whichever view is active: the
    /// single chapter, or the continuous window (#700), searching across its
    /// loaded chapters. From the query, `r` promotes it to a replace.
    pub(super) fn open_find(&mut self) {
        let origin_byte = self.edit_buffer().cursor_byte();
        self.mode = Mode::Search(Box::new(Search::new(origin_byte, false, false)));
    }

    /// Open a Vim search (#206): `/` forward, `?` backward. Reuses the find query
    /// line and live highlight; Enter finalizes back to Normal and records the
    /// search for `n` / `N`. Works in the continuous view too (#700).
    pub(super) fn open_vim_search(&mut self, backward: bool) {
        self.vim_state.op = None; // a search isn't an operator target here
        let origin_byte = self.edit_buffer().cursor_byte();
        self.mode = Mode::Search(Box::new(Search::new(origin_byte, backward, true)));
    }

    /// `n` / `N` in Normal mode (#206): jump to the next match of the last search.
    /// `reverse` inverts the recorded direction (so `N` goes the other way).
    pub(super) fn vim_search_repeat(&mut self, reverse: bool) {
        self.vim_state.op = None;
        let Some(search) = self.vim_state.last_search.clone() else {
            self.message = Some("No previous search.".into());
            return;
        };
        self.vim_search_jump(&search.query, search.backward ^ reverse);
    }

    /// `*` in Normal mode (#206): search forward for the word under the cursor.
    pub(super) fn vim_search_word(&mut self) {
        self.vim_state.op = None;
        let buffer = self.edit_buffer();
        let Some((lo, hi)) = buffer.text_object(TextObject::Word, false) else {
            self.message = Some("No word under the cursor.".into());
            return;
        };
        let word = buffer.byte_slice(lo, hi);
        if word.trim().is_empty() {
            self.message = Some("No word under the cursor.".into());
            return;
        }
        self.vim_state.last_search = Some(VimSearch {
            query: word.clone(),
            backward: false,
        });
        self.vim_search_jump(&word, false);
    }

    /// Jump the cursor to the next/previous match of `query` from its current
    /// position, wrapping around the document ends with a notice (Vim's behavior).
    fn vim_search_jump(&mut self, query: &str, backward: bool) {
        let text = self.edit_buffer().text();
        let matches = match_ranges(&text, query);
        if matches.is_empty() {
            self.message = Some(format!("Pattern not found: {query}"));
            return;
        }
        let cur = self.edit_buffer().cursor_byte();
        // A search repeat (`n`/`N`/`*`) is a jump too (#825): leave the current
        // position on the jumplist so `` `` `` can return before we move.
        self.push_jump_from(cur);
        let (target, wrapped) = if backward {
            match matches.iter().rev().find(|&&(start, _)| start < cur) {
                Some(&(start, _)) => (start, false),
                // `matches` is non-empty here (an empty list returned early above),
                // so index the last entry — mirroring the `matches[0]` wrap below.
                None => (matches[matches.len() - 1].0, true),
            }
        } else {
            match matches.iter().find(|&&(start, _)| start > cur) {
                Some(&(start, _)) => (start, false),
                None => (matches[0].0, true),
            }
        };
        let buffer = self.edit_buffer_mut();
        buffer.cursor = buffer.cursor_at_byte(target);
        // Reveal a folded section the jump landed in (#975), so `n`/`N`/`*` still
        // finds and shows matches inside collapsed regions.
        self.reveal_fold_at_cursor();
        if wrapped {
            self.message = Some(
                if backward {
                    "search hit TOP, continuing at BOTTOM"
                } else {
                    "search hit BOTTOM, continuing at TOP"
                }
                .into(),
            );
        }
    }

    /// Key handling while in [`Mode::Search`].
    pub(super) fn on_key_search(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let stage = match &self.mode {
            Mode::Search(s) => s.stage,
            _ => return,
        };
        match (stage, key.code) {
            // --- Toggling the match modes (#570) ---------------------------
            // Ctrl-C / Ctrl-W flip case-sensitive / whole-word and re-run the
            // live query. Only on the Ctrl-F find (a Vim `/`/`?` keeps its plain
            // incremental search). Allowed at the query and navigate stages so
            // the modes can be changed before or while reviewing matches.
            (SearchStage::Query | SearchStage::Navigate, KeyCode::Char('c'))
                if ctrl && !self.search().from_vim =>
            {
                self.search_mut().opts.case_sensitive ^= true;
                self.requery();
            }
            (SearchStage::Query | SearchStage::Navigate, KeyCode::Char('w'))
                if ctrl && !self.search().from_vim =>
            {
                self.search_mut().opts.whole_word ^= true;
                self.requery();
            }
            // Ctrl-R flips regex mode (#891): the query is read as a regular
            // expression rather than a literal, composing with case/whole-word.
            // Guarded on `ctrl`, so a plain `r` at Navigate still promotes to
            // replace (below).
            (SearchStage::Query | SearchStage::Navigate, KeyCode::Char('r'))
                if ctrl && !self.search().from_vim =>
            {
                self.search_mut().opts.regex ^= true;
                self.requery();
            }

            // --- Typing the query ------------------------------------------
            (SearchStage::Query, KeyCode::Char(c)) if !ctrl => {
                self.search_mut().query.push(c);
                self.requery();
            }
            (SearchStage::Query, KeyCode::Backspace) => {
                self.search_mut().query.pop();
                self.requery();
            }
            (SearchStage::Query, KeyCode::Enter) => self.search_query_done(),

            // --- Navigating matches ----------------------------------------
            // `n`/Enter/Down step forward at the Navigate stage; Tab steps forward
            // from the query line too, so a Vim `/`/`?` — which commits on Enter and
            // never reaches Navigate — can still cycle matches before landing. `N`/
            // Up/Shift-Tab step back. Both wrap.
            (SearchStage::Query, KeyCode::Tab)
            | (
                SearchStage::Navigate,
                KeyCode::Char('n') | KeyCode::Enter | KeyCode::Down | KeyCode::Tab,
            ) => {
                self.search_advance(1);
            }
            (SearchStage::Query, KeyCode::BackTab)
            | (SearchStage::Navigate, KeyCode::Char('N') | KeyCode::Up | KeyCode::BackTab) => {
                self.search_advance(-1);
            }
            // Promote a find into a replace using the same query.
            (SearchStage::Navigate, KeyCode::Char('r')) => {
                self.search_mut().stage = SearchStage::Replacement;
            }

            // --- Typing the replacement ------------------------------------
            (SearchStage::Replacement, KeyCode::Char(c)) if !ctrl => {
                self.search_mut().replacement.push(c);
            }
            (SearchStage::Replacement, KeyCode::Backspace) => {
                self.search_mut().replacement.pop();
            }
            (SearchStage::Replacement, KeyCode::Enter) => self.search_replacement_done(),

            // --- Stepping through replacements -----------------------------
            (SearchStage::Step, KeyCode::Char('y')) => self.replace_current_and_advance(),
            (SearchStage::Step, KeyCode::Char('n' | 's')) => self.search_advance(1),
            (SearchStage::Step, KeyCode::Char('a')) => self.replace_all_remaining(),
            // Done stepping: report what was replaced so far.
            (SearchStage::Step, KeyCode::Enter | KeyCode::Esc) => self.finish_replace(),

            // Esc backs out of the other stages without touching the text.
            (_, KeyCode::Esc) => self.close_search(),
            _ => {}
        }
    }

    /// Enter on the query. An empty query just closes. A Vim search (#206)
    /// records itself for `n`/`N` and returns to Normal with the cursor on the
    /// match; the Ctrl-F find moves to navigation (where `r` promotes to replace).
    fn search_query_done(&mut self) {
        let s = self.search();
        if s.query.is_empty() {
            self.close_search();
        } else if s.from_vim {
            // Leave a jump-back target at where the search began (#825), so `` `` ``
            // returns there rather than to a match navigated past.
            let origin = s.origin_byte;
            self.vim_state.last_search = Some(VimSearch {
                query: s.query.clone(),
                backward: s.backward,
            });
            self.push_jump_from(origin);
            self.mode = self.editing_mode(); // cursor already on the current match
        } else {
            let origin = s.origin_byte;
            self.push_jump_from(origin);
            self.search_mut().stage = SearchStage::Navigate;
        }
    }

    // --- Navigation --------------------------------------------------------

    /// Move the current match by `dir` (+1 next, -1 previous), wrapping around.
    fn search_advance(&mut self, dir: i32) {
        {
            let s = self.search_mut();
            if s.matches.is_empty() {
                return;
            }
            let n = s.matches.len() as i32;
            s.current = (s.current as i32 + dir).rem_euclid(n) as usize;
        }
        self.jump_to_current();
    }

    // --- Replacement -------------------------------------------------------

    /// Replacement text entered: start stepping through matches from the top.
    fn search_replacement_done(&mut self) {
        self.recompute_matches_from(0);
        let s = self.search_mut();
        s.replaced = 0;
        s.stage = SearchStage::Step;
        if s.matches.is_empty() {
            self.finish_replace();
        } else {
            self.jump_to_current();
        }
    }

    /// Replace the current match, then advance to the next one past it.
    fn replace_current_and_advance(&mut self) {
        let Some((start, end)) = self.search().current_match() else {
            self.finish_replace();
            return;
        };
        // In the continuous view a match can straddle a fixed chapter seam (#700).
        // The range-edit path clamps such a range back to the starting chapter, so
        // applying the replacement would land it over only the match's head and
        // strand the tail in the next chapter (corruption, #6). Skip it cleanly:
        // step past it without counting, and tell the author it was passed over.
        if self.range_crosses_seam(start, end) {
            self.step_past_unapplied(end);
            self.message = Some(seam_skip_note(1));
            return;
        }
        let replacement = self.search().replacement.clone();
        // A fully-clamped seam no-op reports `false` (nothing changed): don't count
        // it, and step past it rather than re-anchoring on the same spot — otherwise
        // an unremovable seam match re-offers itself forever (#17).
        if !self.edit_replace_range(start, end, &replacement) {
            self.step_past_unapplied(end);
            return;
        }
        self.search_mut().replaced += 1;

        // The edit shifted later offsets — recompute and resume past the
        // replacement (so replacing "a" with "aa" doesn't re-match itself).
        self.recompute_matches_from(start + replacement.len());
        if self.search().current_match().is_none() {
            self.finish_replace();
        } else {
            self.jump_to_current();
        }
    }

    /// Step past a match at `[_, end)` that was *not* applied (a cross-seam skip or
    /// a fully-clamped no-op): re-anchor on the first match after it and jump there,
    /// finishing when none remain. Leaves `replaced` untouched, so an unapplied
    /// match is never counted and stepping can't loop on it.
    fn step_past_unapplied(&mut self, end: usize) {
        self.recompute_matches_from(end);
        if self.search().current_match().is_none() {
            self.finish_replace();
        } else {
            self.jump_to_current();
        }
    }

    /// Replace every remaining match (current to end) in one undo group. Done
    /// right-to-left so earlier byte offsets stay valid as the text shifts.
    fn replace_all_remaining(&mut self) {
        let s = self.search();
        let ranges = s.matches[s.current.min(s.matches.len())..].to_vec();
        let replacement = s.replacement.clone();
        let mut skipped = 0;
        for &(start, end) in ranges.iter().rev() {
            // Skip a match that reaches across a chapter seam (#700) — the edit
            // would clamp to the starting chapter and corrupt the match's tail
            // (#6). Count only replacements that genuinely changed text, so a
            // fully-clamped no-op never inflates the tally (#17).
            if self.range_crosses_seam(start, end) {
                skipped += 1;
                continue;
            }
            if self.edit_replace_range(start, end, &replacement) {
                self.search_mut().replaced += 1;
            }
        }
        self.finish_replace();
        if skipped > 0 {
            let note = seam_skip_note(skipped);
            self.message = Some(match self.message.take() {
                Some(m) if !m.is_empty() => format!("{m} {note}"),
                _ => note,
            });
        }
    }

    /// Whether the buffer byte range `[start, end)` reaches across a fixed chapter
    /// seam in the continuous view (#700): it begins in one loaded chapter but
    /// extends past that chapter's editable span — the seam newline that keeps the
    /// chapters apart is never editable, so [`Manuscript::replace_byte_range`]
    /// clamps such a range back to the starting chapter. Mirrors that clamp's
    /// bound so find/replace can detect the case up front and skip it rather than
    /// corrupt the match's tail. Always `false` in the single-chapter view (no
    /// seams there, so an ordinary replace is never a partial one).
    fn range_crosses_seam(&self, start: usize, end: usize) -> bool {
        let Some(m) = &self.manuscript else {
            return false;
        };
        let spans = m.segment_spans();
        let Some((i, &(_, _, seg_end))) = spans
            .iter()
            .enumerate()
            .find(|(_, &(_, seg_start, seg_end))| start >= seg_start && start < seg_end)
        else {
            return false;
        };
        // A middle chapter keeps its terminating seam newline (not editable); the
        // last loaded chapter's whole span is editable.
        let max_end = if i + 1 == spans.len() {
            seg_end
        } else {
            seg_end - 1
        };
        end > max_end
    }

    /// End the replace flow, report the count, and return to editing.
    fn finish_replace(&mut self) {
        let replaced = self.search().replaced;
        self.cache_focus_words();
        self.mode = self.editing_mode();
        self.message = Some(match replaced {
            0 => "No matches replaced.".into(),
            1 => "Replaced 1 occurrence.".into(),
            n => format!("Replaced {n} occurrences."),
        });
    }

    fn close_search(&mut self) {
        // Abandoning a Vim search restores the cursor to where it started, like
        // Vim's incremental search (#206).
        if self.search().from_vim {
            let origin = self.search().origin_byte;
            let buffer = self.edit_buffer_mut();
            buffer.cursor = buffer.cursor_at_byte(origin);
        }
        self.mode = self.editing_mode();
    }

    // --- Shared helpers ----------------------------------------------------

    /// Re-run the search for a changed query and jump to the current match. The
    /// match nearest the search origin is selected; if none lie at/after it, the
    /// first match in the document is used so the user always sees one.
    fn requery(&mut self) {
        let origin = self.search().origin_byte;
        let backward = self.search().backward;
        self.recompute_matches_from(origin);
        let s = self.search_mut();
        if !s.matches.is_empty() {
            if backward {
                // A `?` search lands on the nearest match *before* the origin,
                // wrapping to the last if none precede it.
                s.current = s
                    .matches
                    .iter()
                    .rposition(|&(start, _)| start < origin)
                    .unwrap_or(s.matches.len() - 1);
            } else if s.current >= s.matches.len() {
                s.current = 0;
            }
        }
        self.jump_to_current();
    }

    /// Recompute the match list for the current query and point `current` at the
    /// first match at/after `anchor` (or past the end — i.e. "no current match" —
    /// when none follow it).
    fn recompute_matches_from(&mut self, anchor: usize) {
        let text = self.edit_buffer().text();
        let s = self.search_mut();
        s.matches = match_ranges_opts(&text, &s.query, s.opts);
        s.current = s
            .matches
            .iter()
            .position(|&(start, _)| start >= anchor)
            .unwrap_or(s.matches.len());
    }

    /// Move the cursor to the current match so the editor scrolls it into view.
    fn jump_to_current(&mut self) {
        if let Some((start, _)) = self.search().current_match() {
            let buffer = self.edit_buffer_mut();
            buffer.cursor = buffer.cursor_at_byte(start);
            // Find still matches inside folded regions (#975): if the match landed in
            // a collapsed section, reveal it so the cursor isn't hidden away.
            self.reveal_fold_at_cursor();
        }
    }

    fn search(&self) -> &Search {
        match &self.mode {
            Mode::Search(s) => s,
            _ => unreachable!("search helper called outside Mode::Search"),
        }
    }

    fn search_mut(&mut self) -> &mut Search {
        match &mut self.mode {
            Mode::Search(s) => s,
            _ => unreachable!("search helper called outside Mode::Search"),
        }
    }
}

/// The notice shown when `n` cross-seam match(es) were passed over during a
/// replace (#6): they can't be replaced in the continuous view without corrupting
/// the text either side of the seam, so the author is told rather than surprised.
fn seam_skip_note(n: usize) -> String {
    if n == 1 {
        "1 match spanning a chapter break was skipped.".into()
    } else {
        format!("{n} matches spanning a chapter break were skipped.")
    }
}
