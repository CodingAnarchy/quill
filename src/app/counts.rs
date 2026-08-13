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
        let active = self.project.active_index();
        for file in files {
            let Some(i) = self.project.index_of_file(&file) else {
                continue; // chapter gone (e.g. deleted) since it was recorded
            };
            let words = self.chapter_word_count_with_active(i, active);
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
                let name = p
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("{label} {}", i + 1));
                let start = p.chapters[0].saturating_sub(1).min(body);
                let end = p.chapters[1].min(body).max(start);
                (name, (front + start)..(front + end))
            })
            .collect()
    }

    pub(super) fn part_word_counts(&self) -> Vec<usize> {
        let active = self.project.active_index();
        self.part_ranges()
            .into_iter()
            .map(|(_, range)| {
                range
                    .map(|i| self.chapter_word_count_with_active(i, active))
                    .sum()
            })
            .collect()
    }

    /// Whether the active edit target has unsaved changes (either view).
    pub(super) fn edit_dirty(&self) -> bool {
        self.manuscript
            .as_ref()
            .map_or(self.buffer.dirty, crate::manuscript::Manuscript::dirty)
    }

    /// Word count for chapter `index`: live from the buffer when it's active
    /// (updates as you type), otherwise the count cached on the chapter entry.
    pub fn chapter_word_count(&self, index: usize) -> usize {
        self.chapter_word_count_with_active(index, self.project.active_index())
    }

    /// Like [`Self::chapter_word_count`] but with the active index already
    /// resolved. `active_index()` is an O(n) scan, so callers looping over every
    /// chapter (e.g. the per-frame outline render) resolve it once and pass it
    /// in here to avoid an accidental O(chapters²) per frame.
    pub fn chapter_word_count_with_active(&self, index: usize, active: usize) -> usize {
        // The live buffer stands in for the active chapter only when it *is* the
        // active chapter. When a non-chapter reference pane is focused (a bible
        // entry / note, #165) `self.buffer` holds that file, so fall back to the
        // active chapter's cached count rather than attribute the reference file's
        // length to it.
        if index == active && self.focus_shows_active_chapter() {
            self.buffer.word_count()
        } else {
            self.project.chapter_words(index)
        }
    }

    /// Total words across the whole project (the active chapter live, the rest
    /// cached). Used by the progress report and the save-path snapshot (#867).
    ///
    /// Reads the maintained running total ([`Project::words_total`]) rather than
    /// re-summing every chapter, so a save costs O(1) here — the total is kept in
    /// step as chapters are cached, saved, added, and removed. The total counts the
    /// active chapter from its *cache*, so swap in the live buffer count to reflect
    /// unsaved edits (the buffer's word count updates as you type).
    pub fn project_word_count(&self) -> usize {
        // Swap the active chapter's cached count for the live buffer only when the
        // buffer actually holds it. With a reference pane focused (#165) the buffer
        // is a bible entry / note, so keep the all-cached total rather than let the
        // reference file's length distort the project word count.
        if !self.focus_shows_active_chapter() {
            return self.project.words_total();
        }
        let active = self.project.active_index();
        let cached_active = self.project.chapter_words(active);
        (self.project.words_total() + self.buffer.word_count()).saturating_sub(cached_active)
    }

    /// Words in body chapters only — what the progress report measures against the
    /// goal and projects from (#182), excluding front/back matter.
    pub fn body_word_count(&self) -> usize {
        let active = self.project.active_index();
        (0..self.chapter_count())
            .filter(|&i| self.project.chapter_band(i) == Band::Body)
            .map(|i| self.chapter_word_count_with_active(i, active))
            .sum()
    }

    /// Words in front/back-matter sections — shown in the report but not counted
    /// toward the goal (#182).
    pub fn section_word_count(&self) -> usize {
        self.project_word_count() - self.body_word_count()
    }

    /// Total words in book `b` (#331): the active book counted live (its active
    /// chapter from the buffer), the others from their caches.
    pub fn book_word_count(&self, b: usize) -> usize {
        if b == self.project.active_book() {
            self.project_word_count()
        } else {
            self.project.book_word_count(b)
        }
    }

    /// Total words across the whole series (#331): the active book live, the rest
    /// from their caches. Equals `project_word_count` for a standalone novel.
    pub fn series_word_count(&self) -> usize {
        (0..self.project.book_count())
            .map(|b| self.book_word_count(b))
            .sum()
    }

    /// Today's net word change (#1012): words added minus removed, from the daily
    /// tally the idle tick maintains. Reads the cached counters only — no
    /// manuscript walk — so it's O(1) and safe on every draw. Negative when the
    /// author cut more than they wrote today.
    pub const fn day_net_words(&self) -> i64 {
        self.added as i64 - self.removed as i64
    }

    /// This writing session's net output (#1012): the growth in the daily tally
    /// since the session baseline ([`record_session`](Self::record_session) advances
    /// it), so it's just this run's writing, not the day's accumulated total. Reads
    /// the cached counters only — O(1). Negative when the session was a net trim.
    pub const fn session_net_words(&self) -> i64 {
        let added = self.added.saturating_sub(self.session_base_added);
        let removed = self.removed.saturating_sub(self.session_base_removed);
        added as i64 - removed as i64
    }

    /// The configured daily word target (#1012), the `[goals] daily` key — `None`
    /// when unset. The ambient below-target cue and the recap read this; it's the
    /// same cached goal the progress report projects against, no file read.
    pub fn daily_target(&self) -> Option<usize> {
        self.project.goals().and_then(|g| g.daily)
    }

    /// How far under the daily target today's net is (#1012), or `None` when there's
    /// nothing to nudge about: no target set, or the net already meets it. The single
    /// source of truth for whether the status-bar cue shows and by how much — reads
    /// only the cached tally + goal, so it's O(1) per draw. A net at or above the
    /// target (or a negative net still below a target) yields `Some(remaining)` only
    /// while genuinely short; a met target returns `None`.
    pub fn daily_target_shortfall(&self) -> Option<usize> {
        let target = self.daily_target()?;
        let net = self.day_net_words();
        (net < target as i64).then(|| (target as i64 - net).max(0) as usize)
    }

    /// Whether the ambient below-target cue is currently showing (#1012): the nudge
    /// is enabled, not dismissed this session, and today's net is under a set daily
    /// target. The single predicate the status-bar tag and the Esc-dismiss handler
    /// both read, so they can't drift. O(1) — cached tally + goal only.
    pub fn nudge_showing(&self) -> bool {
        self.daily_nudge && !self.nudge_dismissed && self.daily_target_shortfall().is_some()
    }

    /// Seed today's tally directly (#1012), for render tests outside the `app`
    /// module that can't reach the private counters. Sets the day's added/removed so
    /// the below-target cue and recap have a known net, without driving a full edit.
    #[cfg(test)]
    pub const fn set_day_tally_for_test(&mut self, added: usize, removed: usize) {
        self.added = added;
        self.removed = removed;
    }

    /// The end-of-session recap (#1012): a quiet one-line summary of this session's
    /// net words and the day's net, shown on quit. `None` when the session wrote
    /// nothing (net zero) so a glance-and-quit run stays silent. When a daily target
    /// is set, notes whether the day met it. Pure string work over the cached tally.
    pub fn session_recap(&self) -> Option<String> {
        let session = self.session_net_words();
        if session == 0 {
            return None;
        }
        let day = self.day_net_words();
        let target_note = match self.daily_target() {
            Some(target) if day >= target as i64 => format!(" · daily target met ({target})"),
            Some(target) => format!(" · {} short of {target}", (target as i64 - day).max(0)),
            None => String::new(),
        };
        Some(format!(
            "This session: {session:+} words · today: {day:+} net{target_note}"
        ))
    }
}
