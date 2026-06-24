//! Word-count surfaces and the daily tally: per-chapter/section/project counts
//! and the idle-tick tally over changed chapters. Extracted from `mod.rs` (#314).





use crate::frontmatter::Band;

use super::App;

impl App {
    /// Re-initialize the daily word tally for the active book (#329): size the
    /// per-part counters to its parts and re-baseline its chapters, so opening
    /// (or switching to) a book neither misindexes a part nor counts the book's
    /// existing words as today's writing. The session's global added/removed
    /// totals carry over.
    pub(super) fn reset_tally_for_active_book(&mut self) {
        let parts = self.project.manifest.parts.as_ref().map_or(0, Vec::len);
        self.part_added = vec![0; parts];
        self.part_removed = vec![0; parts];
        self.seed_baselines();
    }

    /// Attribute each chapter's word change since the last idle tick to today's
    /// global and per-part added / removed counters. Walking per chapter (rather
    /// than the project total) lets a delta be booked to the part of the chapter
    /// that changed; the global counters stay equal to the sum of the parts.
    /// Structural edits (chapter delete) drop their baseline so they aren't
    /// booked as prose.
    pub(super) fn tally_words(&mut self) {
        // Only a few chapters can have changed since the last idle tick: the live
        // active chapter, plus any whose cached count moved (continuous-view saves,
        // reslide). Re-checking just those keeps the tally O(edit), not O(project)
        // (#220). Keyed by file so a reorder between ticks can't misattribute.
        let mut files = self.project.take_words_changed();
        files.insert(self.project.active_file().to_string()); // always re-check the live active chapter
        for file in files {
            let Some(i) = self.project.index_of_file(&file) else {
                continue; // chapter gone (e.g. deleted) since it was recorded
            };
            let words = self.chapter_word_count(i);
            let last = self.baselines.get(&file).copied().unwrap_or(0);
            let delta = words as i64 - last as i64;
            if delta == 0 {
                continue;
            }
            let (add, rem) = if delta > 0 {
                (delta as usize, 0)
            } else {
                (0, (-delta) as usize)
            };
            self.added += add;
            self.removed += rem;
            // part_of() only yields indices that exist, and the per-part vecs are
            // sized to the part count, so direct indexing is safe — and the global
            // counters stay equal to the sum of the parts by construction.
            if let Some(p) = self.project.manifest.part_of(i) {
                self.part_added[p] += add;
                self.part_removed[p] += rem;
            }
            self.baselines.insert(file, words);
        }
    }

    /// Current word total for each part, in manifest part order (empty when no
    /// parts). Each part sums the live counts of the chapters in its range.
    /// Each part's display name (its `name`, or `"{label} {n}"` when unnamed) and
    /// the 0-based chapter range it spans (`start..end`, clamped to the chapters
    /// that exist). Empty when the book defines no parts. The single home for the
    /// 1-based→0-based range conversion and the unnamed-part naming.
    pub(super) fn part_ranges(&self) -> Vec<(String, std::ops::Range<usize>)> {
        let Some(parts) = self.project.manifest.parts.as_ref() else {
            return Vec::new();
        };
        let label = self.project.manifest.part_label();
        // Part ranges are 1-based positions among the body chapters (#185); the
        // body block is contiguous after the band reorder, starting past the
        // front matter — so a body position maps to `front + position`.
        let front = self.project.manifest.front_chapter_count();
        let body = self.project.body_chapter_count();
        parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let name = p.name.clone().unwrap_or_else(|| format!("{label} {}", i + 1));
                let start = p.chapters[0].saturating_sub(1).min(body);
                let end = p.chapters[1].min(body).max(start);
                (name, (front + start)..(front + end))
            })
            .collect()
    }

    pub(super) fn part_word_counts(&self) -> Vec<usize> {
        self.part_ranges()
            .into_iter()
            .map(|(_, range)| range.map(|i| self.chapter_word_count(i)).sum())
            .collect()
    }

    /// Whether the active edit target has unsaved changes (either view).
    pub(super) fn edit_dirty(&self) -> bool {
        match &self.manuscript {
            Some(m) => m.dirty(),
            None => self.buffer.dirty,
        }
    }

    /// Word count for chapter `index`: live from the buffer when it's active
    /// (updates as you type), otherwise the count cached on the chapter entry.
    pub fn chapter_word_count(&self, index: usize) -> usize {
        if index == self.project.active_index() {
            self.buffer.word_count()
        } else {
            self.project.chapter_words(index)
        }
    }

    /// Total words across the whole project (the active chapter live, the rest
    /// cached). Used by the progress report.
    pub fn project_word_count(&self) -> usize {
        (0..self.chapter_count())
            .map(|i| self.chapter_word_count(i))
            .sum()
    }

    /// Words in body chapters only — what the progress report measures against the
    /// goal and projects from (#182), excluding front/back matter.
    pub fn body_word_count(&self) -> usize {
        (0..self.chapter_count())
            .filter(|&i| self.project.chapter_band(i) == Band::Body)
            .map(|i| self.chapter_word_count(i))
            .sum()
    }

    /// Words in front/back-matter sections — shown in the report but not counted
    /// toward the goal (#182).
    pub fn section_word_count(&self) -> usize {
        self.project_word_count() - self.body_word_count()
    }
}
