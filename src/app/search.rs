//! Find and find-and-replace over the active chapter (#144).
//!
//! Search is scoped to the single-chapter view: opening it leaves the
//! continuous view first, so every operation works on one chapter's buffer.
//! Matches are stored as byte ranges into that buffer and recomputed whenever
//! the query or the document changes; the editor highlights them and the cursor
//! jumps to the current one. Replace goes through the buffer's normal edit path,
//! so it is captured by undo (#145).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::App;
use crate::mode::{Mode, Search, SearchStage};
use crate::text::match_ranges;

impl App {
    /// Open the find prompt (Ctrl-F). Drops to the single-chapter view so search
    /// operates on one chapter; from the query, `r` promotes it to a replace.
    pub(super) fn open_find(&mut self) {
        self.leave_continuous();
        let origin_byte = self.buffer.cursor_byte();
        self.mode = Mode::Search(Box::new(Search {
            stage: SearchStage::Query,
            query: String::new(),
            replacement: String::new(),
            matches: Vec::new(),
            current: 0,
            origin_byte,
            replaced: 0,
        }));
    }

    /// Key handling while in [`Mode::Search`].
    pub(super) fn on_key_search(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let stage = match &self.mode {
            Mode::Search(s) => s.stage,
            _ => return,
        };
        match (stage, key.code) {
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

            // --- Navigating matches (find) ---------------------------------
            (SearchStage::Navigate, KeyCode::Char('n') | KeyCode::Enter | KeyCode::Down) => {
                self.search_advance(1)
            }
            (SearchStage::Navigate, KeyCode::Char('N') | KeyCode::Up) => self.search_advance(-1),
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
            (SearchStage::Step, KeyCode::Char('n') | KeyCode::Char('s')) => self.search_advance(1),
            (SearchStage::Step, KeyCode::Char('a')) => self.replace_all_remaining(),
            // Done stepping: report what was replaced so far.
            (SearchStage::Step, KeyCode::Enter | KeyCode::Esc) => self.finish_replace(),

            // Esc backs out of the other stages without touching the text.
            (_, KeyCode::Esc) => self.close_search(),
            _ => {}
        }
    }

    /// Enter on the query: a non-empty query moves to navigation; an empty one
    /// just closes. (Replace is reached from navigation with `r`.)
    fn search_query_done(&mut self) {
        let s = self.search_mut();
        if s.query.is_empty() {
            self.close_search();
        } else {
            s.stage = SearchStage::Navigate;
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
        let replacement = self.search().replacement.clone();
        self.buffer.replace_byte_range(start, end, &replacement);
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

    /// Replace every remaining match (current to end) in one undo group. Done
    /// right-to-left so earlier byte offsets stay valid as the text shifts.
    fn replace_all_remaining(&mut self) {
        let s = self.search();
        let ranges = s.matches[s.current.min(s.matches.len())..].to_vec();
        let replacement = s.replacement.clone();
        for &(start, end) in ranges.iter().rev() {
            self.buffer.replace_byte_range(start, end, &replacement);
        }
        self.search_mut().replaced += ranges.len();
        self.finish_replace();
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
        self.mode = self.editing_mode();
    }

    // --- Shared helpers ----------------------------------------------------

    /// Re-run the search for a changed query and jump to the current match. The
    /// match nearest the search origin is selected; if none lie at/after it, the
    /// first match in the document is used so the user always sees one.
    fn requery(&mut self) {
        let origin = self.search().origin_byte;
        self.recompute_matches_from(origin);
        let s = self.search_mut();
        if s.current >= s.matches.len() && !s.matches.is_empty() {
            s.current = 0;
        }
        self.jump_to_current();
    }

    /// Recompute the match list for the current query and point `current` at the
    /// first match at/after `anchor` (or past the end — i.e. "no current match" —
    /// when none follow it).
    fn recompute_matches_from(&mut self, anchor: usize) {
        let text = self.buffer.text();
        let s = self.search_mut();
        s.matches = match_ranges(&text, &s.query);
        s.current = s
            .matches
            .iter()
            .position(|&(start, _)| start >= anchor)
            .unwrap_or(s.matches.len());
    }

    /// Move the cursor to the current match so the editor scrolls it into view.
    fn jump_to_current(&mut self) {
        if let Some((start, _)) = self.search().current_match() {
            self.buffer.cursor = self.buffer.cursor_at_byte(start);
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
