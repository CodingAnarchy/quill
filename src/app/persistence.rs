//! Recovery sidecar and history snapshots: load/persist and the recovery
//! offer/restore flow. Extracted from `mod.rs` (#314).

use crate::buffer::{self};
use crate::date;
use crate::history;
use crate::lastpos;
use crate::mode::Pending;
use crate::recovery;

use super::App;
use crate::project::Project;

impl App {
    /// Reset the per-chapter word baselines to the current counts, so the next
    /// tally measures edits from here. Used at startup and after a structural
    /// edit that shouldn't be booked as prose.
    pub(super) fn seed_baselines(&mut self) {
        let active = self.project.active_index();
        self.baselines = (0..self.chapter_count())
            .map(|i| {
                (
                    self.project.chapter_file(i).to_string(),
                    self.chapter_word_count_with_active(i, active),
                )
            })
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
        // The counters seeded below are today's; anchor the rollover watcher to the
        // same day so the first cross-midnight idle tick rebases them (#1170).
        self.tally_day = date::iso(date::today());
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
        // The session starts from today's already-recorded output, so a recorded
        // session captures only this run's growth (#835) — never a day's worth
        // re-logged every reopen.
        self.session_base_added = self.added;
        self.session_base_removed = self.removed;
    }

    /// Append the current writing session's output to the session log (#835): the
    /// growth in `added` / `removed` since the session baseline, dated today.
    /// Called at quit. Advances the baseline so a second record in the same run
    /// logs only new growth, and an empty session is skipped by the log itself.
    /// Best-effort — a failed write just drops one session record.
    pub fn record_session(&mut self) {
        let session = history::SessionRecord {
            date: date::iso(date::today()),
            added: self.added.saturating_sub(self.session_base_added),
            removed: self.removed.saturating_sub(self.session_base_removed),
        };
        let _ = history::persist_session(&self.project.root, &session);
        self.session_base_added = self.added;
        self.session_base_removed = self.removed;
    }

    /// Write today's snapshot to the history log (best-effort).
    ///
    /// [`history::persist`] appends, and `load`/`compact` coalesce to one line per
    /// day — so a run of autosaves with unchanged totals would leave a stack of
    /// identical day lines to tidy later. Skip the append when this snapshot equals
    /// the last one we wrote (#869): the same totals produce the same coalesced
    /// record, so nothing is lost, and an untouched idle autosave stops growing the
    /// log. (Only equal snapshots are skipped, so the first write of the day and any
    /// changed write still land.)
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
        if self.last_history_snapshot.borrow().as_ref() == Some(&snapshot) {
            return;
        }
        if history::persist(&self.project.root, &snapshot).is_ok() {
            *self.last_history_snapshot.borrow_mut() = Some(snapshot);
        }
    }

    /// Record where the author is editing right now, so the next clean open can
    /// resume here (#588). Written on a clean save and at quit — the last-position
    /// counterpart to the crash-recovery sidecar, and separate from it: this
    /// remembers the *location* of a saved session, not unsaved *text*. In the
    /// single-chapter view the cursor is a byte offset into that chapter's prose;
    /// the continuous view (a multi-chapter window) records only the active
    /// chapter, resuming at its top. Best-effort — a failed write just means the
    /// next open starts at the top.
    pub fn persist_position(&self) {
        let cursor = match &self.manuscript {
            Some(_) => 0,
            None => self.buffer.cursor_byte(),
        };
        let pos = lastpos::Position {
            chapter_file: self.project.active_file().to_string(),
            cursor,
        };
        let _ = lastpos::write(&self.project.root, &pos);
    }

    /// At launch, resume the last editing position a clean exit recorded (#588):
    /// activate the saved chapter and place the cursor at the saved byte offset.
    /// A record for a chapter that no longer exists is ignored (start at the top);
    /// an offset past the current end of the file is clamped, so a chapter edited
    /// smaller elsewhere still lands somewhere valid. Never fails loudly — with no
    /// (or an unusable) record the open just stays at chapter 0, top of file.
    pub fn restore_position(&mut self) {
        let Some(pos) = lastpos::read(&self.project.root) else {
            return;
        };
        let Some(index) = self.project.index_of_file(&pos.chapter_file) else {
            return; // that chapter is gone — start at the top
        };
        self.activate_chapter(index); // a no-op when it's already active
        if self.project.active_file() == pos.chapter_file {
            let clamped = pos.cursor.min(self.buffer.len_bytes());
            self.buffer.cursor = self.buffer.cursor_at_byte(clamped);
        }
    }

    /// Capture or clear the recovery sidecar on the idle cadence. The
    /// single-chapter view maps the buffer 1:1 to a file; the continuous view
    /// captures every dirty segment in its window, each attributed to its chapter
    /// (#566). The sidecar is written while there are unsaved edits and removed
    /// once everything is clean — tracked by `recovery_written`, so a clean
    /// buffer issues at most one remove rather than one per idle tick.
    pub(super) fn capture_recovery(&mut self) {
        let rec = match &self.manuscript {
            // Continuous view: snapshot each dirty segment's in-memory prose,
            // attributing it to that segment's chapter file.
            Some(m) => {
                let chapters = m
                    .dirty_segments()
                    .into_iter()
                    .map(|(index, body)| recovery::Chapter {
                        chapter_file: self.project.chapter_file(index).to_string(),
                        body,
                    })
                    .collect();
                recovery::Recovery::from_chapters(chapters)
            }
            // Single-chapter view: the active chapter's buffer, 1:1 to its file —
            // but only when the focused editor actually *is* the active chapter.
            // A focused reference pane (a story-bible entry / loose note opened
            // beside the prose, #165) is also `self.buffer`, yet its text is NOT
            // the active chapter's. Tagging it with `active_file()` would make
            // crash recovery offer to overwrite the active chapter with the
            // reference file's content — data corruption. We can't instead tag it
            // with the reference file's own name either: the restore path resolves
            // a captured file through the manifest (`index_of_file` in
            // `restore_single_recovery` / `offer_recovery`), and a non-chapter
            // reference file has no manifest entry, so such a record could never be
            // restored anyway. So skip the snapshot while a reference pane is
            // focused; the active chapter's file on disk is left untouched
            // (#165 follow-up).
            None => (self.buffer.dirty && self.focus_shows_active_chapter()).then(|| {
                recovery::Recovery::single(
                    self.project.active_file().to_string(),
                    self.buffer.text(),
                )
            }),
        };
        if let Some(rec) = rec {
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
    /// A captured chapter that's gone, or whose body already matches what's on
    /// disk (so nothing was actually lost), is dropped; a record left with no
    /// recoverable chapter is stale and silently cleared. Continuous-view records
    /// can carry several chapters at once (#566).
    pub fn offer_recovery(&mut self) {
        let Some(rec) = recovery::read(&self.project.root) else {
            return;
        };
        // Keep only the chapters that still exist and actually differ from disk.
        // Track whether any captured chapter was skipped because its file couldn't
        // be *read* (a hard error, not a missing file): that chapter's fate is
        // unknown this launch, so the sidecar must be preserved rather than cleared.
        //
        // A hard read error (`Err`) is *not* grounds to offer recovery: we can't
        // prove the captured body differs from disk, and treating an unreadable
        // chapter as "changed" would spuriously re-offer content that may never
        // have been lost (#871). Only a readable file whose content genuinely
        // differs keeps the captured chapter; an unreadable one is skipped now but
        // preserved for a later launch.
        let mut unreadable = false;
        let mut live: Vec<recovery::Chapter> = Vec::new();
        for c in rec.chapters() {
            let Some(index) = self.project.index_of_file(&c.chapter_file) else {
                continue; // the captured chapter is gone — nothing to restore
            };
            match self.project.try_chapter_body(index) {
                Ok(disk) => {
                    let differs = buffer::normalize_trailing_newline(&disk)
                        != buffer::normalize_trailing_newline(&c.body);
                    if differs {
                        live.push(c);
                    }
                }
                Err(_) => unreadable = true,
            }
        }
        let Some(pruned) = recovery::Recovery::from_chapters(live) else {
            // Nothing to offer. Clear the sidecar only if every captured chapter
            // was genuinely resolved (matched disk or is gone); if any was merely
            // unreadable this launch, keep the sidecar so a later launch — when the
            // file may read again — can still recover it (#871).
            if !unreadable {
                recovery::clear(&self.project.root);
            }
            return;
        };
        let descriptor = self.recovery_descriptor(&pruned);
        self.pending_recovery = Some(pruned);
        self.open_prompt(
            Pending::ConfirmRecovery,
            format!(
                "Unsaved work for {descriptor} from a previous session — restore (y), discard (d), or keep for later (Esc)"
            ),
            "",
            None,
        );
    }

    /// On open, if the project looks damaged, offer to restore from the latest
    /// local backup (#934). "Damaged" is a manifest that won't parse or a chapter
    /// file that's missing or unreadable — the kind of data loss a backup exists to
    /// undo. When the newest archive #888 wrote is available, this opens a y/N
    /// restore confirm targeting it; otherwise it's a clean no-op (nothing to offer,
    /// or the project is intact). Skipped when a prompt/overlay is already up (the
    /// crash-recovery offer takes precedence), so the two never fight over the
    /// screen. Reads only the chapter files' existence + the backups dir — never a
    /// re-parse of the manuscript — so it costs an open next to nothing.
    pub fn offer_backup_restore_on_damage(&mut self) {
        // Don't pre-empt an already-open prompt/overlay (e.g. the crash-recovery
        // offer, which is about *unsaved* work and comes first).
        if !matches!(
            self.mode,
            crate::mode::Mode::Insert | crate::mode::Mode::Normal
        ) {
            return;
        }
        if self.project_looks_intact() {
            return;
        }
        // The project looks damaged. Offer the newest backup, if there is one.
        let Some(latest) = self.list_local_backups().into_iter().next() else {
            self.message =
                Some("This project looks damaged, and there's no local backup to restore.".into());
            return;
        };
        self.pending_restore_backup = Some(latest.path);
        self.open_prompt(
            Pending::ConfirmRestoreBackup,
            format!(
                "This project looks damaged. Restore from the latest backup ({})? (y/N)",
                latest.name
            ),
            "",
            None,
        );
    }

    /// Whether the open project's files look intact (#934): its manifest parses and
    /// every chapter file the manifest references exists and reads. A missing or
    /// unreadable chapter, or a manifest that no longer parses, reads as damaged —
    /// the signal the on-open restore offer keys on. A missing chapter file counts:
    /// [`try_chapter_body`](crate::project::Project::try_chapter_body) reads it as
    /// the legitimate "empty chapter" (`Ok("")`), so existence is checked
    /// separately. Checks file existence + a cheap read probe, never a full re-parse
    /// of the prose, so it stays proportional to the chapter *count*, not the
    /// novel's size.
    fn project_looks_intact(&self) -> bool {
        // The manifest must still parse — a corrupt `book.toml` is damage even when
        // the in-memory project (opened earlier) is fine.
        if Project::open(&self.project.root).is_err() {
            return false;
        }
        (0..self.chapter_count()).all(|i| {
            self.project.chapter_path(i).is_file() && self.project.try_chapter_body(i).is_ok()
        })
    }

    /// How the recovery prompt names what it found: one chapter by title, or a
    /// count when several chapters from the continuous view were captured (#566).
    fn recovery_descriptor(&self, rec: &recovery::Recovery) -> String {
        let count = rec.chapters().count();
        if count == 1 {
            let title = self
                .project
                .index_of_file(&rec.chapter_file)
                .and_then(|i| self.project.chapter_title(i))
                .unwrap_or("a chapter");
            format!("\"{title}\"")
        } else {
            format!("{count} chapters")
        }
    }

    /// Restore the unsaved work the recovery prompt offered. A single captured
    /// chapter loads into the single-chapter buffer; several chapters (from the
    /// continuous view) open the continuous view and overlay their recovered
    /// prose onto the loaded window (#566). Either way the recovered work lands in
    /// the editable buffer, dirty, so a save persists it and the sidecar keeps
    /// protecting it meanwhile — files are never rewritten behind the author.
    pub(super) fn restore_recovery(&mut self) {
        let Some(rec) = self.pending_recovery.take() else {
            return;
        };
        if rec.extra.is_empty() {
            self.restore_single_recovery(&rec);
        } else {
            self.restore_continuous_recovery(&rec);
        }
    }

    /// Restore a single captured chapter into the single-chapter view.
    fn restore_single_recovery(&mut self, rec: &recovery::Recovery) {
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

    /// Restore several captured chapters into the continuous view: center the
    /// window on the first recovered chapter, then overlay each recovered body
    /// onto its segment, leaving them dirty for review and save (#566).
    fn restore_continuous_recovery(&mut self, rec: &recovery::Recovery) {
        // Resolve the captured chapters to manifest indices that still exist.
        let chapters: Vec<(usize, String)> = rec
            .chapters()
            .filter_map(|c| {
                self.project
                    .index_of_file(&c.chapter_file)
                    .map(|i| (i, c.body))
            })
            .collect();
        let Some(&(first, _)) = chapters.first() else {
            self.clear_recovery();
            self.message = Some("Could not recover: those chapters no longer exist.".into());
            return;
        };
        // Open the continuous view centered on the first recovered chapter, then
        // overlay the recovered prose. `enter_continuous` already flushes and
        // sets up the window; do nothing if it couldn't open (a chapter read).
        self.project.set_active(first);
        self.enter_continuous();
        let Some(m) = self.manuscript.as_mut() else {
            self.message = Some("Could not open continuous view to recover.".into());
            return;
        };
        let restored = m.restore_bodies(&chapters);
        self.clear_annotation_history();
        self.recovery_written = true; // the sidecar already matches; a save clears it
        self.message = Some(format!(
            "Recovered unsaved work in {} chapters — review and save.",
            restored.len()
        ));
    }
}
