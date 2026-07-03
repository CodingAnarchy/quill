//! Chapter CRUD and navigation (activate, create, rename, move, delete).
//! Extracted from `mod.rs` (#314).

use crate::buffer::Buffer;

use super::App;
use super::ONLY_CHAPTER_MSG;

impl App {
    pub(super) fn activate_chapter(&mut self, index: usize) {
        if index >= self.project.manifest.chapters.len() || index == self.project.active_index() {
            return;
        }
        // Flush the chapter we're leaving before loading the next.
        if self.flush_active().is_err() {
            return;
        }
        match Buffer::from_path(self.project.chapter_path(index)) {
            Ok(buffer) => {
                self.buffer = buffer;
                self.project.set_active(index);
                // No need to cache the new active chapter's count: while active,
                // chapter_word_count reads it live from the buffer.
                if let Some(title) = self.project.active_title() {
                    self.message = Some(format!("Chapter: {title}"));
                }
            }
            Err(e) => self.message = Some(format!("Open failed: {e}")),
        }
    }

    pub(super) const fn chapter_count(&self) -> usize {
        self.project.manifest.chapters.len()
    }

    pub(super) fn next_chapter(&mut self) {
        self.activate_chapter(self.project.active_index() + 1);
    }

    pub(super) fn prev_chapter(&mut self) {
        // At chapter 0 this targets 0 — a no-op via activate_chapter's guard.
        self.activate_chapter(self.project.active_index().saturating_sub(1));
    }

    pub(super) fn create_chapter(&mut self, at: usize, title: &str) -> usize {
        match self.project.add_chapter(at, title) {
            Ok(()) => self.message = Some(format!("Added '{title}'")),
            Err(e) => self.message = Some(format!("Add failed: {e}")),
        }
        at.min(self.chapter_count() - 1)
    }

    pub(super) fn rename_chapter(&mut self, at: usize, title: &str) {
        match self.project.rename_chapter(at, title) {
            Ok(()) => self.message = Some(format!("Renamed to '{title}'")),
            Err(e) => self.message = Some(format!("Rename failed: {e}")),
        }
    }

    /// Move chapter `sel` up or down one slot; returns its new position (the
    /// active chapter follows its content automatically via identity tracking).
    pub(super) fn move_chapter(&mut self, sel: usize, up: bool) -> usize {
        let target = if up {
            sel.checked_sub(1)
        } else {
            (sel + 1 < self.chapter_count()).then_some(sel + 1)
        };
        let Some(j) = target else { return sel };
        // Reordering stays within a band — a section can't be shuffled out of
        // front/back matter into the body or vice versa (#180).
        if self.project.chapter_band(sel) != self.project.chapter_band(j) {
            self.message =
                Some("Reordering stays within front matter / body / back matter.".into());
            return sel;
        }
        match self.project.swap_chapters(sel, j) {
            Ok(()) => j,
            Err(e) => {
                self.message = Some(format!("Move failed: {e}"));
                sel
            }
        }
    }

    /// Split the active chapter at the cursor into two (#585): the prose before
    /// the cursor stays in the current chapter, the prose after it becomes a new
    /// chapter titled `title`, inserted right after. Only the active chapter can
    /// be split — its live buffer is the source of the cursor — so callers pass
    /// its index as `at`. Returns the chapter the browser should select (the new
    /// tail), or `at` unchanged when there's nothing after the cursor to split off.
    pub(super) fn split_chapter(&mut self, at: usize, title: &str) -> usize {
        if at != self.project.active_index() {
            self.message = Some("Open a chapter before splitting it.".into());
            return at;
        }
        // Split the live body at the cursor; the tail becomes the new chapter.
        let text = self.buffer.text();
        let cut = self.buffer.cursor_byte().min(text.len());
        let (head, tail) = text.split_at(cut);
        if tail.trim().is_empty() {
            self.message = Some("Nothing after the cursor to split off.".into());
            return at;
        }
        let head = head.to_string();
        let tail = tail.to_string();
        // The current chapter keeps the head (front matter rides along on save);
        // persist it before writing the tail so both halves land on disk.
        self.buffer.replace_text(&head);
        if self.flush_active().is_err() {
            return at;
        }
        if let Err(e) = self.project.insert_chapter_with_body(at + 1, title, &tail) {
            self.message = Some(format!("Split failed: {e}"));
            return at;
        }
        // Head + tail = the original words, so nothing was added or removed:
        // re-baseline so the next tally doesn't book the structural move.
        self.seed_baselines();
        self.message = Some(format!("Split into '{title}'"));
        at + 1
    }

    /// Merge the chapter at `at` into the chapter above it (#585): its prose is
    /// appended to the previous chapter and its file retired to `unused/`, exactly
    /// like [`delete_chapter`](Self::delete_chapter) — nothing the author wrote is
    /// lost. Returns the chapter the browser should select (the now-combined
    /// chapter). Merging stays within a band, and the first chapter has no
    /// neighbor above to merge into.
    pub(super) fn merge_chapter(&mut self, at: usize) -> usize {
        if at == 0 {
            self.message = Some("No chapter above to merge into.".into());
            return at;
        }
        let target = at - 1;
        // Merging stays within a band — like reordering, a section can't fold into
        // the body or vice versa (#180).
        if self.project.chapter_band(at) != self.project.chapter_band(target) {
            self.message = Some("Merging stays within front matter / body / back matter.".into());
            return at;
        }
        // Persist the active chapter first so the bodies read from disk are current
        // (either merged chapter may be the active one).
        if self.flush_active().is_err() {
            return at;
        }
        let active_involved =
            self.project.active_index() == at || self.project.active_index() == target;
        let body = self.project.chapter_body(at);
        if let Err(e) = self.project.append_to_chapter(target, &body) {
            self.message = Some(format!("Merge failed: {e}"));
            return at;
        }
        if let Err(e) = self.project.remove_chapter(at) {
            self.message = Some(format!("Merge failed: {e}"));
            return at;
        }
        // The combined chapter sits where the target was. If the active chapter
        // was one of the two merged, its on-disk content changed (or its file is
        // gone) — point at the combined chapter and reload its buffer.
        if active_involved {
            self.project.set_active(target);
            match Buffer::from_path(self.project.active_path()) {
                Ok(buffer) => self.buffer = buffer,
                Err(e) => self.message = Some(format!("Open failed: {e}")),
            }
        }
        // The words moved into the target rather than leaving the manuscript, so
        // nothing is booked as removed: re-baseline to absorb the structural move.
        self.seed_baselines();
        self.message = Some("Merged chapters".into());
        target
    }

    /// Duplicate chapter `at` (#585): copy its file verbatim — front matter and
    /// all — into a new chapter titled `title`, inserted right after. Returns the
    /// chapter the browser should select (the copy).
    pub(super) fn duplicate_chapter(&mut self, at: usize, title: &str) -> usize {
        // If the original is the active chapter, persist its latest text so the
        // copy reflects unsaved edits.
        if at == self.project.active_index() && self.flush_active().is_err() {
            return at;
        }
        if let Err(e) = self.project.duplicate_chapter(at, title) {
            self.message = Some(format!("Duplicate failed: {e}"));
            return at;
        }
        // The copy's words are a structural addition, not authored prose, so
        // re-baseline to keep them off the daily tally.
        self.seed_baselines();
        self.message = Some(format!("Duplicated to '{title}'"));
        at + 1
    }

    /// Delete chapter `at`; returns the chapter the browser should select next.
    pub(super) fn delete_chapter(&mut self, at: usize) -> usize {
        if self.chapter_count() <= 1 {
            self.message = Some(ONLY_CHAPTER_MSG.into());
            return at;
        }
        // Deleting the active chapter is the one structural edit that changes the
        // buffer's content — every other op keeps the active file present, so its
        // index just re-derives.
        let deleting_active = at == self.project.active_index();
        if deleting_active && self.buffer.dirty {
            // Persist the latest text first so the copy moved to unused/ is current.
            let _ = self.buffer.save();
        }
        // The chapter's words leave the manuscript — booked as "moved to unused"
        // today, not as words deleted (that's prose editing).
        let removed_words = self.chapter_word_count(at);
        if let Err(e) = self.project.remove_chapter(at) {
            self.message = Some(format!("Delete failed: {e}"));
            return at.min(self.chapter_count() - 1);
        }

        if deleting_active {
            // The active file is gone — point at a neighbor and reload.
            let neighbor = at.min(self.chapter_count() - 1);
            self.project.set_active(neighbor);
            match Buffer::from_path(self.project.active_path()) {
                Ok(buffer) => self.buffer = buffer,
                Err(e) => self.message = Some(format!("Open failed: {e}")),
            }
        }
        self.moved_to_unused += removed_words;
        // Absorb the structural drop so the next tally doesn't book it as removed:
        // re-baseline every chapter to its current count (this also drops the
        // deleted file's stale entry, since the map is rebuilt by file identity).
        self.seed_baselines();
        self.message = Some("Moved chapter to unused/".into());
        at.min(self.chapter_count() - 1)
    }
}
