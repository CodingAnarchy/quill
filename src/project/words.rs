//! The running word-total and per-chapter cache accessors (#867): the cached
//! per-chapter word and TODO-marker counts, the maintained project word total
//! that lets the save-path snapshot read a total instead of re-summing, and the
//! set of files whose count changed since the daily tally last drained it (#220).
//! Extracted from `mod.rs` to keep that file under the ~1,000-line convention
//! (#894) — a pure move, no behavior change.

use std::collections::HashSet;

use super::Project;

impl Project {
    /// Cached word count for chapter `index` (0 if out of range).
    pub fn chapter_words(&self, index: usize) -> usize {
        self.manifest.chapters.get(index).map_or(0, |c| c.words)
    }

    /// Refresh the cached word count for chapter `index` (no-op if out of range).
    /// The continuous view uses this to update each chapter it saves.
    pub fn set_chapter_words(&mut self, index: usize, words: usize) {
        if let Some(chapter) = self.manifest.chapters.get_mut(index) {
            if chapter.words != words {
                // Keep the running project total in step by the delta (#867), so
                // the save-path snapshot reads a maintained total, not a re-sum.
                self.words_total = self.words_total + words - chapter.words;
                chapter.words = words;
                // Record the change so the daily tally re-checks just this chapter
                // (#220) rather than the whole project.
                self.words_changed.insert(chapter.file.clone());
            }
        }
    }

    /// Cached TODO-marker count for chapter `index` (#867), 0 if out of range.
    /// Populated from the file at open and refreshed for the active chapter on
    /// its save, so the save-path roll-up ([`App::count_todo`]) sums cached counts
    /// instead of re-reading every chapter from disk.
    pub fn chapter_todo(&self, index: usize) -> usize {
        self.manifest.chapters.get(index).map_or(0, |c| c.todo)
    }

    /// Refresh the cached TODO-marker count for chapter `index` (#867), no-op if
    /// out of range. Set alongside the word count on an active-chapter save so the
    /// cached roll-up stays current without a disk read.
    pub fn set_chapter_todo(&mut self, index: usize, todo: usize) {
        if let Some(chapter) = self.manifest.chapters.get_mut(index) {
            chapter.todo = todo;
        }
    }

    /// The maintained running total of the active book's cached per-chapter word
    /// counts (#867) — every chapter counted from its cache, *including* the active
    /// one (whose live buffer edits the caller folds in separately). Used by the
    /// save-path snapshot so the project word total is read, not re-summed.
    pub const fn words_total(&self) -> usize {
        self.words_total
    }

    /// Recompute the running word total from the active book's caches (#867). O(n)
    /// in the chapter count; call it after a bulk re-cache (open, a book switch, an
    /// import) rather than the per-chapter delta path, so the total resets cleanly
    /// to the freshly cached counts.
    pub(super) fn sync_words_total(&mut self) {
        self.words_total = self.manifest.chapters.iter().map(|c| c.words).sum();
    }

    /// Take the files whose cached word count changed since the last call — what
    /// the daily tally needs to re-check (#220).
    pub fn take_words_changed(&mut self) -> HashSet<String> {
        std::mem::take(&mut self.words_changed)
    }

    /// Discard any pending word-count changes — used when the daily tally
    /// re-baselines everything (a structural edit), so nothing stale carries over.
    pub fn clear_words_changed(&mut self) {
        self.words_changed.clear();
    }
}
