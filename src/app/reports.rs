//! Read-only reports and the overlay that displays them (#150): the progress and
//! analysis report rows, the word-count helpers they roll up, and the overlay
//! scroll handling. Split out of `app/mod.rs`; `App` methods via `use super::*`.

use super::*;

use crate::text::count_matches;

impl App {
    /// Open the progress report overlay (Ctrl-R, and the run-action menu #225).
    pub(crate) fn open_progress_report(&mut self) {
        self.overlay = Some(Overlay::report("progress", self.report_lines()));
    }

    /// Open the local analysis report overlay (Ctrl-T, and the run-action menu).
    pub(crate) fn open_analysis_report(&mut self) {
        self.overlay = Some(Overlay::report("analysis", self.analysis_lines()));
    }

    /// Open the writing-history view overlay (the run-action menu, #586): the
    /// daily net-words graph, the current and longest streak, the totals, and the
    /// month calendar heatmap with its per-session breakdown (#835) — built from
    /// the recorded per-day and per-session logs, read-only.
    pub(crate) fn open_writing_history(&mut self) {
        let snaps = crate::history::load(&self.project.root);
        let sessions = crate::history::load_sessions(&self.project.root);
        let lines = crate::history_view::lines(&snaps, &sessions, date::today());
        self.overlay = Some(Overlay::report("history", lines));
    }

    /// Open the built-in user's guide overlay (F1, and the run-action menu) (#281).
    pub(crate) fn open_guide(&mut self) {
        let lines = crate::guide::TEXT
            .lines()
            .map(|l| Line::raw(l.to_string()))
            .collect();
        self.overlay = Some(Overlay::guide(lines));
    }

    /// The progress report's rows, computed once when it's opened.
    pub(crate) fn report_lines(&self) -> Vec<Line<'static>> {
        let momentum = Momentum {
            added: self.added,
            removed: self.removed,
            moved_to_unused: self.moved_to_unused,
        };
        // A whole-series total leads the report when the project holds more than
        // one book (#331); a standalone novel shows just its own figures.
        let series = (self.project.book_count() > 1).then(|| report::SeriesSummary {
            words: self.series_word_count(),
            books: self.project.book_count(),
        });
        report::lines(
            &self.project.manifest,
            self.body_word_count(), // the goal & projection track the novel (#182)
            self.section_word_count(), // front/back matter, shown but not counted
            self.project.body_chapter_count(), // front/back matter aren't chapters (#180)
            self.count_todo(),
            &momentum,
            &self.part_deltas(),
            date::today(),
            series,
        )
    }

    /// The open overlay's lines, if any (test inspection helper).
    #[cfg(test)]
    pub fn overlay_lines(&self) -> Option<&[Line<'static>]> {
        self.overlay.as_ref().map(|o| o.lines.as_slice())
    }

    /// The open overlay's scroll offset (0 when none is open).
    pub(crate) fn overlay_scroll(&self) -> u16 {
        self.overlay.as_ref().map_or(0, |o| o.scroll)
    }

    /// The largest valid scroll offset for the open overlay: enough to bring its
    /// last line into view, zero when it already fits (or none is open). Shares
    /// the bound formula with the renderer ([`crate::ui::max_scroll`]).
    fn overlay_max_scroll(&self) -> u16 {
        self.overlay.as_ref().map_or(0, |o| {
            crate::ui::max_scroll(o.lines.len(), self.viewport_height)
        })
    }

    /// Handle a key while a full-screen overlay is open: `s` saves a report
    /// snapshot to a file (#601), the navigation keys scroll, anything else closes
    /// it (so it stays "press a key to dismiss").
    pub(crate) fn on_key_overlay(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // `s` on a report overlay saves the rendered report to `export/`, leaving
        // the overlay open so the author can keep reading — it's the one key the
        // report overlay handles beyond scrolling. (`s` is free: it isn't a scroll
        // key, and the AI/guide overlays return `None` from `report_label`.)
        if !ctrl
            && key.code == KeyCode::Char('s')
            && self
                .overlay
                .as_ref()
                .and_then(Overlay::report_label)
                .is_some()
        {
            self.save_report();
            return;
        }
        let max = self.overlay_max_scroll();
        let page = self.viewport_height.saturating_sub(3).max(1); // keep a line of context
        let cur = self.overlay_scroll();
        // A confirm overlay (the pre-commit review, #704) scrolls the same way, but
        // Enter proceeds to the action and any non-scroll key cancels rather than
        // silently closing — so the author never commits by mistaking a dismiss.
        let confirm = self.overlay.as_ref().is_some_and(Overlay::is_confirm);
        // `Some(row)` scrolls to `row`; a non-scroll key resolves the overlay.
        match super::scroll_target(key.code, ctrl, cur, max, page) {
            Some(row) => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.scroll = row;
                }
            }
            None if confirm => {
                self.overlay = None;
                if key.code == KeyCode::Enter {
                    self.confirm_commit_review();
                } else {
                    self.message = Some("Commit cancelled.".into());
                }
            }
            None => self.overlay = None,
        }
    }

    /// Write the open report overlay's rendered text to a dated file under the
    /// project's `export/` dir (#601): `export/<label>-<date>.md`, where `label`
    /// is the report's kind ("progress", "analysis", …) and `date` is today's ISO
    /// stamp. Reuses the on-screen lines verbatim (via [`Overlay::plain_text`]), so
    /// the file matches what the author is looking at, and the same
    /// [`write_export`](Self::write_export) path the manuscript exports use.
    /// A no-op for a non-report overlay.
    pub(crate) fn save_report(&mut self) {
        let Some((label, body)) = self
            .overlay
            .as_ref()
            .and_then(|o| Some((o.report_label()?, o.plain_text())))
        else {
            return;
        };
        let name = format!("{label}-{}", date::iso(date::today()));
        self.write_export(&name, "md", body.as_bytes());
    }

    /// The analysis report's rows (readability per chapter + overall), computed
    /// once when it's opened. Scans every chapter's prose (active chapter live,
    /// the rest from disk) — on demand, never per frame.
    pub(crate) fn analysis_lines(&self) -> Vec<Line<'static>> {
        // Style-flag list sizes / cutoffs (whole-manuscript signals).
        const TOP_WORDS: usize = 12;
        const TOP_PHRASES: usize = 8;
        const PHRASE_MIN: usize = 3; // a phrase must recur at least this often to list
        let threshold = self.project.manifest.run_on_threshold();
        // The effective stopword / filter-word lists: built-in defaults adjusted
        // by the project's `[analysis]` overrides (#119), built once for the report.
        let mut lists = analysis::WordLists::new();
        if let Some(a) = &self.project.manifest.analysis {
            lists.override_stopwords(&a.stopwords_add, &a.stopwords_remove);
            lists.override_filter_words(&a.filter_words_add, &a.filter_words_remove);
        }
        let mut chapters = Vec::with_capacity(self.chapter_count());
        let mut longest = analysis::LongestSentences::new(5);
        let mut word_counts = HashMap::new();
        let mut phrase_counts = HashMap::new();
        let mut tokens = Vec::new();
        for i in 0..self.chapter_count() {
            let title = self
                .project
                .chapter_title(i)
                .unwrap_or("Untitled")
                .to_string();
            // Measure prose only — drop heading lines so a chapter/scene title
            // doesn't glue onto its first sentence or skew readability.
            let text = analysis::prose(&self.chapter_text(i));
            let stats = analysis::stats(&text);
            let sentences = analysis::sentences(&text);
            let sentence_lengths: Vec<usize> =
                sentences.iter().map(|s| buffer::count_words(s)).collect();
            for (s, &words) in sentences.iter().zip(&sentence_lengths) {
                longest.consider(&title, s, words);
            }
            // Frequency tallies accumulate across the whole manuscript; phrases
            // are gathered per chapter so they don't span a chapter boundary.
            analysis::tally_content_words(&text, &mut word_counts, &lists);
            analysis::tally_phrases(&text, &mut phrase_counts, &lists);
            // Tokenize once: the just-appended slice is this chapter's lexical
            // variety; the whole pool is the overall variety (a true MATTR, not a
            // mean of the chapters').
            let token_start = tokens.len();
            analysis::tally_tokens(&text, &mut tokens);
            // Pacing uses the rolled-up chapter word count (headings included),
            // matching the chapter list and progress report.
            chapters.push(analysis::ChapterAnalysis {
                title,
                words: self.chapter_word_count(i),
                stats,
                sentence_lengths,
                style: analysis::style_flags(&text, &lists),
                dialogue: analysis::dialogue_split(&text),
                paragraph_lengths: analysis::paragraph_lengths(&text),
                lexical_variety: analysis::lexical_variety_tokens(&tokens[token_start..]),
            });
        }
        let longest = longest.into_vec();
        let parts = self.analysis_parts();
        let top_words = analysis::top_counts(&word_counts, TOP_WORDS, 1);
        let top_phrases = analysis::top_counts(&phrase_counts, TOP_PHRASES, PHRASE_MIN);
        let report = analysis::Report {
            title: &self.project.manifest.title,
            chapters: &chapters,
            longest: &longest,
            parts: &parts,
            run_on_threshold: threshold,
            top_words: &top_words,
            top_phrases: &top_phrases,
            overall_variety: analysis::lexical_variety_tokens(&tokens),
        };
        analysis::lines(&report)
    }

    /// The part groupings for the pacing section. Empty when the book defines no
    /// parts.
    fn analysis_parts(&self) -> Vec<analysis::PartSpan> {
        self.part_ranges()
            .into_iter()
            .map(|(name, range)| analysis::PartSpan { name, range })
            .collect()
    }

    /// Build the per-part rows for the progress report: each part's display name,
    /// current word total, and today's added / removed within it.
    fn part_deltas(&self) -> Vec<PartDelta> {
        let words = self.part_word_counts();
        let goals = self.project.manifest.parts.as_ref();
        self.part_ranges()
            .into_iter()
            .enumerate()
            // `words` and the per-part counters are all sized to the parts list.
            .map(|(i, (name, _))| PartDelta {
                name,
                words: words[i],
                goal: goals.and_then(|p| p.get(i)).and_then(|p| p.goal),
                added: self.part_added[i],
                removed: self.part_removed[i],
            })
            .collect()
    }

    /// The body text of chapter `index`: live from the active buffer, otherwise
    /// from disk. The shared basis for whole-manuscript reads (TODO counting,
    /// the AI request).
    pub(crate) fn chapter_text(&self, index: usize) -> String {
        if index == self.project.active_index() {
            self.buffer.text()
        } else {
            self.project.chapter_body(index)
        }
    }

    /// Count the configured TODO marker across the manuscript (case-insensitive).
    pub(crate) fn count_todo(&self) -> usize {
        let marker = self.project.manifest.todo_marker();
        (0..self.chapter_count())
            .map(|i| count_matches(&self.chapter_text(i), marker))
            .sum()
    }
}
