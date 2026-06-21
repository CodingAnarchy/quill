//! Read-only reports and the overlay that displays them (#150): the progress and
//! analysis report rows, the word-count helpers they roll up, and the overlay
//! scroll handling. Split out of `app/mod.rs`; `App` methods via `use super::*`.

use super::*;

use crate::text::count_matches;

impl App {
    /// The progress report's rows, computed once when it's opened.
    pub(crate) fn report_lines(&self) -> Vec<Line<'static>> {
        let momentum = Momentum {
            added: self.added,
            removed: self.removed,
            moved_to_unused: self.moved_to_unused,
        };
        report::lines(
            &self.project.manifest,
            self.project_word_count(),
            self.chapter_count(),
            self.count_todo(),
            &momentum,
            &self.part_deltas(),
            date::today(),
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
        self.overlay.as_ref().map_or(0, |o| crate::ui::max_scroll(o.lines.len(), self.viewport_height))
    }

    /// Handle a key while a full-screen overlay is open: scroll on the navigation
    /// keys, close on anything else (so it stays "press a key to dismiss").
    pub(crate) fn on_key_overlay(&mut self, key: KeyEvent) {
        let max = self.overlay_max_scroll() as i32;
        let page = self.viewport_height.saturating_sub(3).max(1) as i32; // keep a line of context
        let cur = self.overlay_scroll() as i32;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // `Some(row)` scrolls to `row`; `None` dismisses the overlay.
        let target = match (ctrl, key.code) {
            (false, KeyCode::Down | KeyCode::Char('j')) => Some((cur + 1).clamp(0, max)),
            (false, KeyCode::Up | KeyCode::Char('k')) => Some((cur - 1).clamp(0, max)),
            (false, KeyCode::PageDown | KeyCode::Char(' ')) | (true, KeyCode::Char('d')) => Some((cur + page).clamp(0, max)),
            (false, KeyCode::PageUp | KeyCode::Char('b')) | (true, KeyCode::Char('u')) => Some((cur - page).clamp(0, max)),
            (false, KeyCode::Home | KeyCode::Char('g')) => Some(0),
            (false, KeyCode::End | KeyCode::Char('G')) => Some(max),
            _ => None,
        };
        match target {
            Some(row) => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.scroll = row as u16;
                }
            }
            None => self.overlay = None,
        }
    }

    /// The analysis report's rows (readability per chapter + overall), computed
    /// once when it's opened. Scans every chapter's prose (active chapter live,
    /// the rest from disk) — on demand, never per frame.
    pub(crate) fn analysis_lines(&self) -> Vec<Line<'static>> {
        let threshold = self.project.manifest.run_on_threshold();
        let mut chapters = Vec::with_capacity(self.chapter_count());
        let mut longest = analysis::LongestSentences::new(5);
        let mut word_counts = HashMap::new();
        let mut phrase_counts = HashMap::new();
        let mut tokens = Vec::new();
        for i in 0..self.chapter_count() {
            let title = self.project.chapter_title(i).unwrap_or("Untitled").to_string();
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
            analysis::tally_content_words(&text, &mut word_counts);
            analysis::tally_phrases(&text, &mut phrase_counts);
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
                style: analysis::style_flags(&text),
                dialogue: analysis::dialogue_split(&text),
                paragraph_lengths: analysis::paragraph_lengths(&text),
                lexical_variety: analysis::lexical_variety_tokens(&tokens[token_start..]),
            });
        }
        let longest = longest.into_vec();
        let parts = self.analysis_parts();
        // Style-flag list sizes / cutoffs (whole-manuscript signals).
        const TOP_WORDS: usize = 12;
        const TOP_PHRASES: usize = 8;
        const PHRASE_MIN: usize = 3; // a phrase must recur at least this often to list
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
