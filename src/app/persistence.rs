//! Recovery sidecar and history snapshots: load/persist and the recovery
//! offer/restore flow. Extracted from `mod.rs` (#314).





use crate::buffer::{self};
use crate::date;
use crate::history;
use crate::mode::Pending;
use crate::recovery;

use super::App;

impl App {
    /// Reset the per-chapter word baselines to the current counts, so the next
    /// tally measures edits from here. Used at startup and after a structural
    /// edit that shouldn't be booked as prose.
    pub(super) fn seed_baselines(&mut self) {
        self.baselines = (0..self.chapter_count())
            .map(|i| (self.project.chapter_file(i).to_string(), self.chapter_word_count(i)))
            .collect();
        // A full re-baseline supersedes any pending per-chapter changes (#220).
        self.project.clear_words_changed();
    }

    /// Seed today's counters from the history log (resume the day across a
    /// reopen). Called once at startup after the project is open.
    pub fn load_history(&mut self) {
        // Coalesce any within-session duplicate lines a prior run appended (#439),
        // so the file stays one line per day. Best-effort, off the save path.
        let _ = history::compact(&self.project.root);
        if let Some(snap) = history::for_date(&self.project.root, &date::iso(date::today())) {
            self.added = snap.added;
            self.removed = snap.removed;
            self.moved_to_unused = snap.moved_to_unused;
            // Resume per-part counters only when they match the current parts
            // config; a changed manifest starts the day's per-part tally fresh.
            if snap.part_added.len() == self.part_added.len() {
                self.part_added = snap.part_added;
                self.part_removed = snap.part_removed;
            }
        }
    }

    /// Write today's snapshot to the history log (best-effort).
    pub fn persist_snapshot(&self) {
        let snapshot = history::Snapshot {
            date: date::iso(date::today()),
            words: self.project_word_count(),
            added: self.added,
            removed: self.removed,
            todo: self.count_todo(),
            moved_to_unused: self.moved_to_unused,
            parts: self.part_word_counts(),
            part_added: self.part_added.clone(),
            part_removed: self.part_removed.clone(),
        };
        let _ = history::persist(&self.project.root, &snapshot);
    }

    /// Capture or clear the recovery sidecar on the idle cadence. Only the
    /// single-chapter view maps the buffer 1:1 to a file; the continuous view
    /// writes back per chapter on its own transitions and is out of scope here.
    /// The sidecar is written while the buffer is dirty and removed once it goes
    /// clean — tracked by `recovery_written`, so a clean buffer issues at most
    /// one remove rather than one per idle tick.
    pub(super) fn capture_recovery(&mut self) {
        if self.manuscript.is_some() {
            return;
        }
        if self.buffer.dirty {
            let rec = recovery::Recovery {
                chapter_file: self.project.active_file().to_string(),
                body: self.buffer.text(),
            };
            if recovery::write(&self.project.root, &rec).is_ok() {
                self.recovery_written = true;
            }
        } else if self.recovery_written {
            self.clear_recovery();
        }
    }

    /// Remove the recovery sidecar — nothing left to recover (a clean save, an
    /// explicit discard, or a deliberate quit despite unsaved changes).
    pub(super) fn clear_recovery(&mut self) {
        recovery::clear(&self.project.root);
        self.recovery_written = false;
    }

    /// At launch, offer back any unsaved work a previous session left behind.
    /// A sidecar whose chapter is gone, or whose body already matches what's on
    /// disk (so nothing was actually lost), is stale and silently cleared.
    pub fn offer_recovery(&mut self) {
        let Some(rec) = recovery::read(&self.project.root) else {
            return;
        };
        let Some(index) = self.project.index_of_file(&rec.chapter_file) else {
            recovery::clear(&self.project.root);
            return;
        };
        let disk_body = self.project.chapter_body(index);
        if buffer::normalize_trailing_newline(&disk_body) == buffer::normalize_trailing_newline(&rec.body) {
            recovery::clear(&self.project.root); // already saved — nothing was lost
            return;
        }
        let title = self.project.chapter_title(index).unwrap_or("a chapter").to_string();
        self.pending_recovery = Some(rec);
        self.open_prompt(
            Pending::ConfirmRecovery,
            format!(
                "Unsaved work for \"{title}\" from a previous session — restore (y), discard (d), or keep for later (Esc)"
            ),
            "",
            None,
        );
    }

    /// Restore the unsaved work the recovery prompt offered: make its chapter
    /// active and load the recovered prose into the buffer. The buffer is now
    /// dirty, so a save persists it and the sidecar keeps protecting it meanwhile.
    pub(super) fn restore_recovery(&mut self) {
        let Some(rec) = self.pending_recovery.take() else {
            return;
        };
        let Some(index) = self.project.index_of_file(&rec.chapter_file) else {
            self.clear_recovery();
            self.message = Some("Could not recover: that chapter no longer exists.".into());
            return;
        };
        self.activate_chapter(index); // a no-op when it's already the active chapter
        if self.project.active_file() == rec.chapter_file {
            self.buffer.replace_text(&rec.body);
            // The recovered body is a fresh undo baseline, so the annotation
            // undo/redo history starts over with it too (#370).
            self.clear_annotation_history();
            self.recovery_written = true; // the sidecar already matches; a save clears it
            self.message = Some("Recovered unsaved work — review and save.".into());
        }
    }
}
