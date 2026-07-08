//! Manuscript-wide find & replace (#295): search every chapter in the project,
//! list the matches grouped by chapter, jump to one, or replace across all of
//! them. Distinct from the single-chapter find (`search.rs`, Ctrl-F) — opened
//! from the command palette and rendered as a results overlay.
//!
//! Matches are byte ranges into each chapter's prose **body** (front matter is
//! split off and never searched or rewritten). Chapter files are read one at a
//! time, on demand — never all bodies held at once — so a series-sized project
//! stays within the existing open/scan budgets. A cross-file replace can't ride
//! the single buffer's undo, so the replace-all is confirmed before it runs; the
//! active chapter's edits go through its live buffer (so those are undoable).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::App;
use crate::mode::{Mode, ProjectMatch, ProjectSearch, ProjectSearchStage};
use crate::text::try_match_ranges;

impl App {
    /// Open manuscript-wide find (#295). Drops to the single-chapter view so a
    /// jump lands in a normal editing buffer, then opens the query line.
    pub(crate) fn open_project_find(&mut self) {
        self.leave_continuous();
        self.mode = Mode::ProjectSearch(Box::new(ProjectSearch::new()));
    }

    /// Key handling while in [`Mode::ProjectSearch`].
    // The Results and Step stages share n/N navigation bodies but stay in their own
    // documented sections ("Navigating results" vs "Stepping through replacements");
    // merging the patterns would fold two distinct stages of the flow together.
    #[allow(clippy::match_same_arms)]
    pub(super) fn on_key_project_search(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let stage = match &self.mode {
            Mode::ProjectSearch(s) => s.stage,
            _ => return,
        };
        match (stage, key.code) {
            // --- Toggling the match modes (#570) ---------------------------
            // Ctrl-C / Ctrl-W flip case-sensitive / whole-word. Available while
            // typing the query and while reviewing results (re-runs the search
            // so the list reflects the new modes).
            (ProjectSearchStage::Query | ProjectSearchStage::Results, KeyCode::Char('c'))
                if ctrl =>
            {
                self.project_search_mut().opts.case_sensitive ^= true;
                self.rerun_project_search_if_listed();
            }
            (ProjectSearchStage::Query | ProjectSearchStage::Results, KeyCode::Char('w'))
                if ctrl =>
            {
                self.project_search_mut().opts.whole_word ^= true;
                self.rerun_project_search_if_listed();
            }
            // Ctrl-R flips regex mode (#891): the query is read as a regular
            // expression rather than a literal, at parity with the chapter find.
            // Guarded on `ctrl`, so a plain `r` at Results still promotes to
            // replace (below).
            (ProjectSearchStage::Query | ProjectSearchStage::Results, KeyCode::Char('r'))
                if ctrl =>
            {
                self.project_search_mut().opts.regex ^= true;
                self.rerun_project_search_if_listed();
            }

            // --- Typing the query ------------------------------------------
            (ProjectSearchStage::Query, KeyCode::Char(c)) if !ctrl => {
                self.project_search_mut().query.push(c);
            }
            (ProjectSearchStage::Query, KeyCode::Backspace) => {
                self.project_search_mut().query.pop();
            }
            (ProjectSearchStage::Query, KeyCode::Enter) => self.run_project_search(),

            // --- Navigating results (find) ---------------------------------
            (ProjectSearchStage::Results, KeyCode::Char('n') | KeyCode::Down) => {
                self.project_search_advance(1);
            }
            (ProjectSearchStage::Results, KeyCode::Char('N') | KeyCode::Up) => {
                self.project_search_advance(-1);
            }
            (ProjectSearchStage::Results, KeyCode::Enter) => self.jump_to_project_match(),
            (ProjectSearchStage::Results, KeyCode::Char('r')) => {
                self.project_search_mut().stage = ProjectSearchStage::Replacement;
            }

            // --- Typing the replacement ------------------------------------
            (ProjectSearchStage::Replacement, KeyCode::Char(c)) if !ctrl => {
                self.project_search_mut().replacement.push(c);
            }
            (ProjectSearchStage::Replacement, KeyCode::Backspace) => {
                self.project_search_mut().replacement.pop();
            }
            (ProjectSearchStage::Replacement, KeyCode::Enter) => {
                let s = self.project_search_mut();
                s.selected = 0;
                s.stage = ProjectSearchStage::Step;
            }

            // --- Stepping through replacements -----------------------------
            (ProjectSearchStage::Step, KeyCode::Char('y')) => self.replace_project_selected(),
            (ProjectSearchStage::Step, KeyCode::Char('n') | KeyCode::Down) => {
                self.project_search_advance(1);
            }
            (ProjectSearchStage::Step, KeyCode::Char('N') | KeyCode::Up) => {
                self.project_search_advance(-1);
            }
            (ProjectSearchStage::Step, KeyCode::Char('a')) => {
                self.project_search_mut().stage = ProjectSearchStage::Confirm;
            }
            (ProjectSearchStage::Step, KeyCode::Enter) => self.finish_project_replace(),

            // --- Confirming the irreversible replace-all -------------------
            (ProjectSearchStage::Confirm, KeyCode::Char('y' | 'Y')) => {
                self.replace_project_all();
            }
            (ProjectSearchStage::Confirm, _) => {
                // Anything else backs out to stepping, nothing replaced.
                self.project_search_mut().stage = ProjectSearchStage::Step;
            }

            // Esc backs out of any other stage without touching the text.
            (_, KeyCode::Esc) => self.close_project_search(),
            _ => {}
        }
    }

    /// Run the query across every chapter, collecting match locations and a
    /// context snippet per match. Reads each chapter's body on demand (one file
    /// at a time); the active chapter's pending edits are flushed first so its
    /// matches are accurate.
    fn run_project_search(&mut self) {
        let query = self.project_search().query.clone();
        if query.is_empty() {
            self.close_project_search();
            return;
        }
        let matches = match self.collect_project_matches(&query) {
            Ok(matches) => matches,
            // A bad regex keeps the overlay open on the query so the author can
            // fix it, rather than a search that silently finds nothing (#891).
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        if matches.is_empty() {
            self.close_project_search();
            self.message = Some(format!("No matches for '{query}' across the manuscript."));
            return;
        }
        let s = self.project_search_mut();
        s.matches = matches;
        s.selected = 0;
        s.stage = ProjectSearchStage::Results;
    }

    /// Re-run the search in place after a mode toggle (#570) while results are
    /// listed, so the list tracks the new modes. Unlike [`run_project_search`],
    /// it keeps the overlay open even if the new modes leave nothing — the author
    /// can toggle back rather than being kicked out — and never resets the stage.
    fn rerun_project_search_if_listed(&mut self) {
        if self.project_search().stage != ProjectSearchStage::Results {
            return;
        }
        let query = self.project_search().query.clone();
        // A toggle that leaves the regex invalid (e.g. flipping regex on with a
        // literal `(` in the query) reports the error and holds the current list,
        // rather than blanking it out from under the author (#891).
        let matches = match self.collect_project_matches(&query) {
            Ok(matches) => matches,
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        let s = self.project_search_mut();
        s.matches = matches;
        s.selected = 0;
    }

    /// Scan every chapter body for `query` under the current modes, building the
    /// match list (with context snippets). Reads each body on demand, one at a
    /// time; the active chapter's pending edits are flushed first so its matches
    /// are accurate. The shared core of [`run_project_search`] and the live
    /// re-run after a mode toggle.
    fn collect_project_matches(&mut self, query: &str) -> Result<Vec<ProjectMatch>, String> {
        let opts = self.project_search().opts;
        let _ = self.flush_active();
        let mut matches = Vec::new();
        for i in 0..self.chapter_count() {
            let body = self.project.chapter_body(i);
            let title = self.project.chapter_title(i).unwrap_or("").to_string();
            // Fallible so an invalid regex is reported once (#891), not silently
            // treated as "no matches"; a literal query never errors.
            for (start, end) in try_match_ranges(&body, query, opts)? {
                matches.push(ProjectMatch {
                    chapter: i,
                    start,
                    end,
                    title: title.clone(),
                    context: line_context(&body, start, end),
                });
            }
        }
        Ok(matches)
    }

    /// Move the selected result by `dir` (+1 next, -1 previous), wrapping.
    fn project_search_advance(&mut self, dir: i32) {
        let s = self.project_search_mut();
        if s.matches.is_empty() {
            return;
        }
        let n = s.matches.len() as i32;
        s.selected = (s.selected as i32 + dir).rem_euclid(n) as usize;
    }

    /// Jump the editor to the selected match: open its chapter (single-chapter
    /// view) and put the cursor on the match.
    fn jump_to_project_match(&mut self) {
        let target = self
            .project_search()
            .matches
            .get(self.project_search().selected)
            .map(|m| (m.chapter, m.start));
        self.mode = self.editing_mode();
        if let Some((chapter, start)) = target {
            self.activate_chapter(chapter);
            self.buffer.cursor = self.buffer.cursor_at_byte(start);
        }
    }

    /// Replace the selected match in place, shift the later matches in that same
    /// chapter by the length delta, drop it from the list, and stay on the next.
    fn replace_project_selected(&mut self) {
        let idx = self.project_search().selected;
        let Some((chapter, start, end)) = self
            .project_search()
            .matches
            .get(idx)
            .map(|m| (m.chapter, m.start, m.end))
        else {
            self.finish_project_replace();
            return;
        };
        let replacement = self.project_search().replacement.clone();
        if let Err(e) = self.apply_body_replacements(chapter, &[(start, end)], &replacement) {
            self.mode = self.editing_mode();
            self.message = Some(format!("Could not replace in that chapter: {e}"));
            return;
        }

        let delta = replacement.len() as isize - (end - start) as isize;
        let s = self.project_search_mut();
        s.replaced += 1;
        // Same-chapter matches are contiguous and after this one; shift them.
        for later in s.matches[idx + 1..]
            .iter_mut()
            .take_while(|m| m.chapter == chapter)
        {
            later.start = (later.start as isize + delta) as usize;
            later.end = (later.end as isize + delta) as usize;
        }
        s.matches.remove(idx);
        if s.matches.is_empty() {
            self.finish_project_replace();
        } else if s.selected >= s.matches.len() {
            self.project_search_mut().selected = self.project_search().matches.len() - 1;
        }
    }

    /// Replace every remaining match across the manuscript (#295), one file read
    /// + write per affected chapter, then report the total and chapter span.
    fn replace_project_all(&mut self) {
        let replacement = self.project_search().replacement.clone();
        // Group remaining matches by chapter, preserving ascending order.
        let mut by_chapter: Vec<(usize, Vec<(usize, usize)>)> = Vec::new();
        for m in &self.project_search().matches {
            match by_chapter.last_mut() {
                Some((c, ranges)) if *c == m.chapter => ranges.push((m.start, m.end)),
                _ => by_chapter.push((m.chapter, vec![(m.start, m.end)])),
            }
        }
        let chapters = by_chapter.len();
        // Tally only the chapters that actually wrote: a read failure aborts that
        // chapter's rewrite (it would otherwise wipe its prose, #565), and the
        // count must reflect what was really replaced, not what was attempted.
        let mut replaced_here = 0;
        let mut done_chapters = 0;
        let mut failure = None;
        for (chapter, ranges) in by_chapter {
            match self.apply_body_replacements(chapter, &ranges, &replacement) {
                Ok(()) => {
                    replaced_here += ranges.len();
                    done_chapters += 1;
                }
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        // The active chapter was edited in its buffer (so it's undoable); persist
        // it too, so every affected chapter is on disk after the bulk replace.
        let _ = self.flush_active();
        let replaced = self.project_search().replaced + replaced_here;
        self.mode = self.editing_mode();
        self.message = Some(failure.map_or_else(
            || {
                format!(
                    "Replaced {replaced} occurrence{} across {chapters} chapter{}.",
                    if replaced == 1 { "" } else { "s" },
                    if chapters == 1 { "" } else { "s" },
                )
            },
            |e| {
                format!(
                    "Replaced {replaced} occurrence{} across {done_chapters} chapter{}; \
                 stopped on a read failure: {e}",
                    if replaced == 1 { "" } else { "s" },
                    if done_chapters == 1 { "" } else { "s" },
                )
            },
        ));
    }

    /// Apply `ranges` (byte ranges into the chapter's body) to chapter
    /// `chapter`, replacing each with `replacement`. The active chapter goes
    /// through its live buffer (so its change stays undoable); others are
    /// rewritten on disk via [`Project::replace_chapter_body`], which preserves
    /// front matter and refreshes the cached word count.
    fn apply_body_replacements(
        &mut self,
        chapter: usize,
        ranges: &[(usize, usize)],
        replacement: &str,
    ) -> std::io::Result<()> {
        if chapter == self.project.active_index() {
            let mut ranges = ranges.to_vec();
            ranges.sort_by_key(|&(s, _)| s);
            for &(start, end) in ranges.iter().rev() {
                self.buffer.replace_byte_range(start, end, replacement);
            }
            self.cache_focus_words();
            Ok(())
        } else {
            // A read failure here is propagated, not swallowed: rewriting from an
            // empty base would wipe the chapter's prose (#565).
            self.project
                .replace_chapter_body(chapter, ranges, replacement)
        }
    }

    /// End the replace flow, report the count, and return to editing.
    fn finish_project_replace(&mut self) {
        let replaced = self.project_search().replaced;
        self.mode = self.editing_mode();
        self.message = Some(match replaced {
            0 => "No matches replaced.".into(),
            1 => "Replaced 1 occurrence.".into(),
            n => format!("Replaced {n} occurrences."),
        });
    }

    fn close_project_search(&mut self) {
        self.mode = self.editing_mode();
    }

    pub(crate) fn project_search(&self) -> &ProjectSearch {
        match &self.mode {
            Mode::ProjectSearch(s) => s,
            _ => unreachable!("project-search helper called outside Mode::ProjectSearch"),
        }
    }

    fn project_search_mut(&mut self) -> &mut ProjectSearch {
        match &mut self.mode {
            Mode::ProjectSearch(s) => s,
            _ => unreachable!("project-search helper called outside Mode::ProjectSearch"),
        }
    }
}

/// The line of `body` containing `[start, end)`, trimmed and clipped to a short
/// snippet for the results list.
fn line_context(body: &str, start: usize, end: usize) -> String {
    let line_start = body[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = body[end..].find('\n').map_or(body.len(), |i| end + i);
    body[line_start..line_end].trim().chars().take(80).collect()
}
