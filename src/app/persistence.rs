//! Recovery sidecar and history snapshots: load/persist and the recovery
//! offer/restore flow. Extracted from `mod.rs` (#314).

use crate::buffer::{self};
use crate::date;
use crate::history;
use crate::lastpos;
use crate::mode::Pending;
use crate::recovery;

use std::time::{Duration, Instant};

use super::App;

/// The fewest seconds between two recovery-sidecar writes while edits keep
/// arriving. Each write copies the buffer, serializes it, and does a durable
/// atomic write (two `fsync`s — `F_FULLFSYNC` on macOS, several ms), so writing
/// on every pause in a stop-start typing rhythm is wasted disk churn. A crash
/// loses at most this much typing beyond the last capture; the throttled write
/// is deferred, never dropped (see [`App::tick_recovery`]).
pub(super) const RECOVERY_MIN_INTERVAL: Duration = Duration::from_secs(2);

/// What a recovery capture wrote: the edit buffer's process-unique content
/// [`revision`](crate::buffer::Buffer::revision) (re-issued on every edit and
/// undo/redo, never reused across buffers) and the chapter files the text was
/// attributed to (the continuous view's dirty segments, or the active chapter).
/// Equal keys mean the sidecar already holds exactly the text a capture would
/// write — the "unchanged since last capture" check.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RecoveryKey {
    revision: u64,
    chapters: Vec<String>,
}

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
    ///
    /// Skipped when the position equals the one last written (or read at launch)
    /// for this project: a save or autosave that leaves the cursor where it was —
    /// the usual case for an idle autosave — costs a comparison, not a file write.
    pub fn persist_position(&self) {
        let cursor = match &self.manuscript {
            Some(_) => 0,
            None => self.buffer.cursor_byte(),
        };
        let pos = lastpos::Position {
            chapter_file: self.project.active_file().to_string(),
            cursor,
        };
        let root = &self.project.root;
        if let Some((last_root, last)) = self.last_position.borrow().as_ref() {
            if last_root == root && *last == pos {
                return;
            }
        }
        if lastpos::write(root, &pos).is_ok() {
            *self.last_position.borrow_mut() = Some((root.clone(), pos));
        }
    }

    /// At launch, resume the cursor at the last editing position a clean exit
    /// recorded (#588). The launch ([`App::launch`]) has already loaded the
    /// remembered chapter when it still exists; this places the cursor at the
    /// saved byte offset in it. An offset past the current end of the file is
    /// clamped (and one inside a multi-byte character floored), so a chapter
    /// edited smaller elsewhere still lands somewhere valid. A record for a chapter
    /// that isn't the active one — it's gone, or wouldn't load — leaves the cursor
    /// at the top. Also notes the record as what's on disk, so a later save that
    /// lands back here needn't rewrite it.
    pub(super) fn resume_cursor(&mut self, pos: Option<lastpos::Position>) {
        let Some(pos) = pos else {
            return;
        };
        if self.project.active_file() == pos.chapter_file {
            let clamped = pos.cursor.min(self.buffer.len_bytes());
            self.buffer.cursor = self.buffer.cursor_at_byte(clamped);
        }
        *self.last_position.borrow_mut() = Some((self.project.root.clone(), pos));
    }

    /// Capture or clear the recovery sidecar on the idle cadence. The
    /// single-chapter view maps the buffer 1:1 to a file; the continuous view
    /// captures every dirty segment in its window, each attributed to its chapter
    /// (#566). The sidecar is written while there are unsaved edits and removed
    /// once everything is clean — tracked by `recovery_written`, so a clean
    /// buffer issues at most one remove rather than one per idle tick.
    ///
    /// Only on change: a capture is keyed on the edit buffer's content revision
    /// plus the chapters it attributes text to ([`RecoveryKey`]), and an idle
    /// pass whose key matches the last successful write returns before copying,
    /// serializing, or fsyncing anything — an arrow key, a mouse move, or a pause
    /// with no new edits costs a comparison, not a durable write. Unthrottled:
    /// the event loop goes through [`tick_recovery`](Self::tick_recovery), which
    /// adds the write-rate limit; tests and explicit callers capture at once.
    pub(super) fn capture_recovery(&mut self) {
        let Some(key) = self.recovery_key() else {
            if self.recovery_written {
                self.clear_recovery();
            }
            return;
        };
        if self.recovery_written && self.recovery_key.as_ref() == Some(&key) {
            return; // the sidecar already holds exactly this text
        }
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
            // Single-chapter view: the active chapter's buffer, 1:1 to its file
            // (the key already checked it's dirty and actually the active chapter).
            None => Some(recovery::Recovery::single(
                self.project.active_file().to_string(),
                self.buffer.text(),
            )),
        };
        if let Some(rec) = rec {
            if recovery::write(&self.project.root, &rec).is_ok() {
                self.recovery_written = true;
                self.recovery_key = Some(key);
                self.recovery_captured_at = Some(Instant::now());
            }
        }
    }

    /// The event loop's recovery capture: [`capture_recovery`](Self::capture_recovery)
    /// rate-limited to one write per [`RECOVERY_MIN_INTERVAL`] while edits keep
    /// coming. Called after every frame (so sustained typing that never pauses is
    /// still captured every couple of seconds) and from the debounced idle pass.
    /// A write the throttle holds back is not lost: the next call after the
    /// interval — at most one loop tick (~100ms) late, input or not — sees the
    /// changed key and writes the latest text, so the final state of a burst is
    /// always captured within about `RECOVERY_MIN_INTERVAL` of the last edit.
    /// Clearing the sidecar (everything saved) is never throttled.
    pub(crate) fn tick_recovery(&mut self) {
        let throttled = self
            .recovery_captured_at
            .is_some_and(|t| t.elapsed() < RECOVERY_MIN_INTERVAL);
        if throttled && self.recovery_key().is_some() {
            return;
        }
        self.capture_recovery();
    }

    /// The change key for a capture right now, or `None` when there is nothing
    /// to recover (clean, or no dirty text attributable to a chapter).
    ///
    /// Single-chapter view: only when the focused editor actually *is* the active
    /// chapter. A focused reference pane (a story-bible entry / loose note opened
    /// beside the prose, #165) is also `self.buffer`, yet its text is NOT the
    /// active chapter's. Tagging it with `active_file()` would make crash recovery
    /// offer to overwrite the active chapter with the reference file's content —
    /// data corruption. We can't instead tag it with the reference file's own name
    /// either: the restore path resolves a captured file through the manifest
    /// (`index_of_file` in `restore_single_recovery` / `offer_recovery`), and a
    /// non-chapter reference file has no manifest entry, so such a record could
    /// never be restored anyway. So skip the snapshot while a reference pane is
    /// focused; the active chapter's file on disk is left untouched (#165
    /// follow-up).
    fn recovery_key(&self) -> Option<RecoveryKey> {
        self.manuscript.as_ref().map_or_else(
            || {
                (self.buffer.dirty && self.focus_shows_active_chapter()).then(|| RecoveryKey {
                    revision: self.buffer.revision(),
                    chapters: vec![self.project.active_file().to_string()],
                })
            },
            |m| {
                let chapters: Vec<String> = m
                    .dirty_chapters()
                    .into_iter()
                    .map(|index| self.project.chapter_file(index).to_string())
                    .collect();
                (!chapters.is_empty()).then(|| RecoveryKey {
                    revision: m.buffer().revision(),
                    chapters,
                })
            },
        )
    }

    /// Remove the recovery sidecar — nothing left to recover (a clean save, an
    /// explicit discard, or a deliberate quit despite unsaved changes).
    pub(super) fn clear_recovery(&mut self) {
        recovery::clear(&self.project.root);
        self.recovery_written = false;
        self.recovery_key = None;
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
    /// screen. Reads only what the open recorded about each chapter file plus the
    /// backups dir — no chapter file is touched again — so it costs an open next
    /// to nothing.
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
    /// unreadable chapter reads as damaged — the signal the on-open restore offer
    /// keys on. A missing chapter file counts even though it loads as the
    /// legitimate "empty chapter".
    ///
    /// Answered from what the open already learned, with no file I/O: the manifest
    /// parsed (or the project wouldn't be open — a `book.toml` that won't parse
    /// fails the open itself, before this runs), and the per-chapter cache pass at
    /// open recorded every file that was missing or unreadable
    /// ([`chapters_intact`](crate::project::Project::chapters_intact)). It used to re-open the whole project and
    /// re-read every chapter to find out, doubling the cost of a launch.
    fn project_looks_intact(&self) -> bool {
        self.project.chapters_intact()
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
