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

impl App {
    /// Open the find prompt (Ctrl-F). Drops to the single-chapter view so search
    /// operates on one chapter; from the query, `r` promotes it to a replace.
    pub(super) fn open_find(&mut self) {
        self.leave_continuous();
        let origin_byte = self.buffer.cursor_byte();
        self.mode = Mode::Search(Box::new(Search {
            stage: SearchStage::Query,
            replace: false,
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
            (SearchStage::Query, KeyCode::Char(c)) if !ctrl => self.search_query_push(c),
            (SearchStage::Query, KeyCode::Backspace) => self.search_query_pop(),
            (SearchStage::Query, KeyCode::Enter) => self.search_query_done(),

            // --- Navigating matches (find) ---------------------------------
            (SearchStage::Navigate, KeyCode::Char('n') | KeyCode::Enter | KeyCode::Down) => {
                self.search_advance(1)
            }
            (SearchStage::Navigate, KeyCode::Char('N') | KeyCode::Up) => self.search_advance(-1),
            // Promote a find into a replace using the same query.
            (SearchStage::Navigate, KeyCode::Char('r')) => self.search_begin_replacement(),

            // --- Typing the replacement ------------------------------------
            (SearchStage::Replacement, KeyCode::Char(c)) if !ctrl => self.search_replacement_push(c),
            (SearchStage::Replacement, KeyCode::Backspace) => self.search_replacement_pop(),
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

    // --- Query entry -------------------------------------------------------

    fn search_query_push(&mut self, c: char) {
        self.search_mut().query.push(c);
        self.recompute_matches();
        self.jump_to_current();
    }

    fn search_query_pop(&mut self) {
        self.search_mut().query.pop();
        self.recompute_matches();
        self.jump_to_current();
    }

    /// Enter on the query: a find moves to navigation; a replace moves on to the
    /// replacement text. An empty query just closes.
    fn search_query_done(&mut self) {
        let s = self.search_mut();
        if s.query.is_empty() {
            self.close_search();
            return;
        }
        s.stage = if s.replace { SearchStage::Replacement } else { SearchStage::Navigate };
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
        // In the replace flow, wrapping past the end means we're done stepping.
        self.jump_to_current();
    }

    // --- Replacement -------------------------------------------------------

    fn search_begin_replacement(&mut self) {
        let s = self.search_mut();
        s.replace = true;
        s.stage = SearchStage::Replacement;
    }

    fn search_replacement_push(&mut self, c: char) {
        self.search_mut().replacement.push(c);
    }

    fn search_replacement_pop(&mut self) {
        self.search_mut().replacement.pop();
    }

    /// Replacement text entered: start stepping through matches from the top.
    fn search_replacement_done(&mut self) {
        self.recompute_matches();
        let s = self.search_mut();
        s.current = 0;
        s.replaced = 0;
        s.stage = SearchStage::Step;
        if self.search_mut().matches.is_empty() {
            self.finish_replace();
        } else {
            self.jump_to_current();
        }
    }

    /// Replace the current match, then advance to the next one past it.
    fn replace_current_and_advance(&mut self) {
        let (range, replacement) = match &self.mode {
            Mode::Search(s) => (s.current_match(), s.replacement.clone()),
            _ => return,
        };
        let Some((start, end)) = range else {
            self.finish_replace();
            return;
        };
        self.buffer.replace_byte_range(start, end, &replacement);
        let resume = start + replacement.len();

        // The edit shifted later offsets — recompute and resume past the
        // replacement (so replacing "a" with "aa" doesn't re-match itself).
        let text = self.buffer.text();
        let done = {
            let s = self.search_mut();
            s.replaced += 1;
            s.matches = find_match_ranges(&text, &s.query);
            s.current = s.matches.iter().position(|&(st, _)| st >= resume).unwrap_or(s.matches.len());
            s.current >= s.matches.len()
        };
        if done {
            self.finish_replace();
        } else {
            self.jump_to_current();
        }
    }

    /// Replace every remaining match (current to end) in one undo group. Done
    /// right-to-left so earlier byte offsets stay valid as the text shifts.
    fn replace_all_remaining(&mut self) {
        let (ranges, replacement) = match &self.mode {
            Mode::Search(s) => (s.matches[s.current.min(s.matches.len())..].to_vec(), s.replacement.clone()),
            _ => return,
        };
        for &(start, end) in ranges.iter().rev() {
            self.buffer.replace_byte_range(start, end, &replacement);
        }
        self.search_mut().replaced += ranges.len();
        self.finish_replace();
    }

    /// End the replace flow, report the count, and return to editing.
    fn finish_replace(&mut self) {
        let replaced = match &self.mode {
            Mode::Search(s) => s.replaced,
            _ => 0,
        };
        self.project.set_active_words(self.buffer.word_count());
        self.mode = Mode::Insert;
        self.message = Some(match replaced {
            0 => "No matches replaced.".into(),
            1 => "Replaced 1 occurrence.".into(),
            n => format!("Replaced {n} occurrences."),
        });
    }

    fn close_search(&mut self) {
        self.mode = Mode::Insert;
    }

    // --- Shared helpers ----------------------------------------------------

    /// Recompute the match list for the current query against the active buffer,
    /// and point `current` at the match nearest the search origin.
    fn recompute_matches(&mut self) {
        let text = self.buffer.text();
        let origin = match &self.mode {
            Mode::Search(s) => s.origin_byte,
            _ => return,
        };
        let s = self.search_mut();
        s.matches = find_match_ranges(&text, &s.query);
        s.current = s
            .matches
            .iter()
            .position(|&(start, _)| start >= origin)
            .unwrap_or(0);
    }

    /// Move the cursor to the current match so the editor scrolls it into view.
    fn jump_to_current(&mut self) {
        if let Some((start, _)) = self.current_search_match() {
            self.buffer.cursor = self.buffer.cursor_at_byte(start);
        }
    }

    fn current_search_match(&self) -> Option<(usize, usize)> {
        match &self.mode {
            Mode::Search(s) => s.current_match(),
            _ => None,
        }
    }

    fn search_mut(&mut self) -> &mut Search {
        match &mut self.mode {
            Mode::Search(s) => s,
            _ => unreachable!("search helper called outside Mode::Search"),
        }
    }
}

/// Byte ranges of every case-insensitive, non-overlapping match of `query` in
/// `text`, in document order. Empty when the query is empty. Case-insensitivity
/// folds each character through its Unicode lowercase mapping, so "José" matches
/// "josé" and "The" matches "the".
pub(super) fn find_match_ranges(text: &str, query: &str) -> Vec<(usize, usize)> {
    let needle: Vec<char> = query.chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let hay: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if (0..needle.len()).all(|k| char_eq_ci(hay[i + k].1, needle[k])) {
            let start = hay[i].0;
            let last = &hay[i + needle.len() - 1];
            out.push((start, last.0 + last.1.len_utf8()));
            i += needle.len(); // non-overlapping
        } else {
            i += 1;
        }
    }
    out
}

/// Case-insensitive single-character comparison via Unicode lowercase folding.
fn char_eq_ci(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::find_match_ranges;

    #[test]
    fn finds_case_insensitive_non_overlapping_matches() {
        let text = "The cat sat on the mat. THE END.";
        let m = find_match_ranges(text, "the");
        assert_eq!(m.len(), 3, "The / the / THE all match");
        // First match is the leading "The" at byte 0.
        assert_eq!(m[0], (0, 3));
        // Each range spans exactly the query length.
        for (s, e) in m {
            assert_eq!(e - s, 3);
        }
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(find_match_ranges("anything", "").is_empty());
    }

    #[test]
    fn non_overlapping_for_repeated_runs() {
        // "aa" in "aaaa" yields two matches, not three.
        assert_eq!(find_match_ranges("aaaa", "aa"), vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn folds_accented_capitals() {
        let text = "José and JOSÉ";
        assert_eq!(find_match_ranges(text, "josé").len(), 2);
    }
}
