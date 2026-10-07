//! Saving off the UI thread. A save (Ctrl-S, `:w`, autosave) snapshots what to
//! write — an O(1) clone of the edit buffer's rope, the chapter file(s), the
//! front matter, the staged annotations side file — and hands it to the
//! project's background [`FileWriter`]; the event loop picks up the outcome
//! ([`App::poll_saves`]) and only then marks the buffer clean, clears the
//! recovery sidecar, and records the history snapshot and resume position (both
//! also written by the worker). Under disk contention a chapter `fsync` has been
//! measured at ~0.9s, which on the UI thread froze typing that resumed as
//! autosave fired.
//!
//! **The state machine.** A [`PendingSave`] remembers the buffer's content
//! [`revision`](crate::buffer::Buffer::revision) (plus path and front matter)
//! at the snapshot. When the write lands, the buffer is marked clean only if it
//! still holds exactly that text; if the author typed meanwhile it stays dirty,
//! and the recovery sidecar — which by then holds (or is getting) the newer text
//! — is not cleared, since a clear is issued only when nothing is left dirty. A
//! failed write surfaces "Save failed: …" (or the autosave form) and leaves the
//! buffer dirty and the sidecar alone. The writer is latest-wins per file set,
//! so an older snapshot never lands after a newer one.
//!
//! **Who waits.** Saves are only ever in flight while the author types, moves
//! the cursor, or sits idle. Everything else first settles them
//! ([`App::settle_saves`], a bounded wait that is a single uncontended lock when
//! nothing is pending): every non-typing key, palette action, and mouse
//! gesture; and, explicitly, every synchronous write path (`flush_active` before
//! a chapter switch, pane close, project switch, archive, …; leaving or paging
//! the continuous view; corkboard rewrites; the annotations side file) and the
//! quit request. So structural operations, exports, VCS commands and quit all
//! see the save on disk, and no synchronous write can race an in-flight one.
//! [`App::save_edit`] (VCS commands, `:wq`, a workflow's save step) submits and
//! then settles, for callers that read the files next.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ropey::Rope;

use crate::atomic;
use crate::bgwrite::{self, Done};
use crate::buffer::{count_words, normalize_trailing_newline_in_place};
use crate::date;
use crate::frontmatter::{self, FrontMatter};
use crate::history;
use crate::lastpos;
use crate::project::AnnotationsWrite;
use crate::text::{count_matches, rope_line_range};

use super::App;

/// The longest [`App::settle_saves`] waits for the writer. Generous — a
/// contended disk has stalled one write for seconds — but bounded, so a disk
/// that has stopped answering can't hang the editor: the action that wanted
/// the save settled is refused with a message instead.
const SAVE_SETTLE_WAIT: Duration = Duration::from_secs(10);

/// The project files writer: chapter saves, the history log, the resume point.
pub(super) type FileWriter = bgwrite::Writer<FileKey, FileJob>;

/// The latest-wins slot a [`FileJob`] occupies: a newer job with an equal key
/// replaces one still waiting.
#[derive(PartialEq, Eq)]
pub(super) enum FileKey {
    /// A save, by the chapter files it writes.
    Save(Vec<PathBuf>),
    /// The day's history snapshot (keyed by date, so the last snapshot of a
    /// day is never superseded by the first of the next).
    History(PathBuf, String),
    /// The resume position.
    Position(PathBuf),
}

/// One unit of work for the project files writer.
pub(super) enum FileJob {
    Save(SaveJob),
    History {
        root: PathBuf,
        snap: history::Snapshot,
    },
    Position {
        root: PathBuf,
        pos: lastpos::Position,
    },
}

/// What a [`FileJob`] reports back.
pub(super) enum FileOutcome {
    Save(SaveOutcome),
    History(bool),
    Position(bool),
}

/// A save's writes: the chapter files, then — only if they all landed, so the
/// notes stay in lockstep with the prose — the annotations side file.
pub(super) struct SaveJob {
    chapters: Vec<ChapterWrite>,
    annotations: Option<AnnotationsWrite>,
    todo_marker: String,
}

/// One chapter file to write: the whole rope (single view) or a line span of
/// it (a continuous-view segment), with its front matter re-attached.
struct ChapterWrite {
    path: PathBuf,
    text: Rope,
    lines: Option<(usize, usize)>,
    front: Option<String>,
    /// The manifest index to report fresh word/TODO counts for (the continuous
    /// view; the single view's counts come from the buffer at the snapshot).
    count_as: Option<usize>,
}

/// How a [`SaveJob`] went.
pub(super) struct SaveOutcome {
    /// The chapter writes: `(index, words, todos)` for each counted chapter, or
    /// the "Save failed: …" message.
    chapters: Result<Vec<(usize, usize, usize)>, String>,
    /// The annotations write, when one was staged and the chapters landed.
    annotations: Option<Result<(), String>>,
}

impl SaveJob {
    fn perform(self) -> SaveOutcome {
        let mut counts = Vec::new();
        for chapter in self.chapters {
            let mut body = match chapter.lines {
                None => chapter.text.to_string(),
                Some((start, end)) => rope_line_range(&chapter.text, start, end),
            };
            normalize_trailing_newline_in_place(&mut body);
            let content = frontmatter::join(chapter.front.as_deref(), &body);
            if let Err(e) = atomic::write(&chapter.path, content.as_bytes()) {
                return SaveOutcome {
                    chapters: Err(format!("Save failed: {e}")),
                    annotations: None,
                };
            }
            if let Some(index) = chapter.count_as {
                let todos = count_matches(&body, &self.todo_marker);
                counts.push((index, count_words(&body), todos));
            }
        }
        SaveOutcome {
            chapters: Ok(counts),
            annotations: self
                .annotations
                .map(|w| w.perform().map_err(|e| e.to_string())),
        }
    }
}

impl bgwrite::Job for FileJob {
    type Outcome = FileOutcome;

    fn perform(self) -> FileOutcome {
        match self {
            Self::Save(job) => FileOutcome::Save(job.perform()),
            Self::History { root, snap } => {
                FileOutcome::History(history::persist(&root, &snap).is_ok())
            }
            Self::Position { root, pos } => {
                FileOutcome::Position(lastpos::write(&root, &pos).is_ok())
            }
        }
    }

    #[cfg(test)]
    fn is_write(&self) -> bool {
        matches!(self, Self::Save(_))
    }
}

/// Whether a save was asked for (Ctrl-S, `:w`: "Saved", the on-save workflow)
/// or is the idle autosave (quiet on success).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SaveKind {
    Explicit,
    Auto,
}

/// What a pending save snapshotted, to tell at completion whether the editor
/// still holds exactly the text that was written.
enum Saved {
    /// The single-chapter view's buffer: its file and front matter.
    Single {
        path: PathBuf,
        front: Option<String>,
    },
    /// The continuous window: each written chapter and its front matter.
    Window {
        chapters: Vec<(usize, Option<String>)>,
    },
}

/// A save handed to the writer and not yet reported back.
pub(super) struct PendingSave {
    ticket: u64,
    /// The edit buffer's content revision at the snapshot.
    revision: u64,
    saved: Saved,
    kind: SaveKind,
    /// An annotations write rides along, owed until it's reported landed.
    annotations: bool,
}

impl App {
    /// Ctrl-S / `:w` / the palette's Save: hand the save to the background
    /// writer and return; "Saved" (and the on-save workflow, #228) follow when
    /// it lands.
    pub(super) fn request_save(&mut self) {
        self.submit_save(SaveKind::Explicit);
    }

    /// An explicit save that has landed (or failed) by the time it returns —
    /// for callers that read or commit the files next (the VCS commands, `:wq`,
    /// a workflow's save step).
    pub(super) fn save_edit(&mut self) {
        self.submit_save(SaveKind::Explicit);
        self.settle_saves();
    }

    /// Autosave on the idle timer (#409): when enabled and there are unsaved
    /// changes, save after the configured seconds of genuine idle. Driven from
    /// the event loop (like `tick_sprint`), so it catches an idle pause even with
    /// no keypress. Cheap and O(1) when off or already saved, and it never waits
    /// on the disk: the save is handed to the background writer. While one is
    /// still in flight it doesn't queue another.
    pub(crate) fn tick_autosave(&mut self) {
        let secs = self.autosave_interval_secs;
        if secs == 0 || !self.edit_dirty() || !self.pending_saves.is_empty() {
            return;
        }
        if self.last_activity.elapsed() >= Duration::from_secs(secs) {
            self.submit_save(SaveKind::Auto);
            // Reset the idle window so a failed save doesn't retry every tick.
            self.last_activity = Instant::now();
        }
    }

    /// Snapshot the active edit target (the continuous window, or the single
    /// chapter) and queue its write, with the persistence that travels with a
    /// save: annotations in lockstep (#302/#370) and the word-count cache.
    fn submit_save(&mut self, kind: SaveKind) {
        // The project is open read-only because another live instance holds it
        // (#871): refuse the write rather than race the owner's saves and clobber
        // the shared chapter files. The open-time warning already told the author.
        if self.project.is_read_only() {
            let e = "Read-only: this project is open in another Quill instance; not saved.";
            self.save_failed(kind, e.into());
            return;
        }
        let revision = self.edit_buffer().revision();
        let (chapters, saved) = if let Some(m) = &self.manuscript {
            let text = m.buffer().rope().clone();
            let dirty = m.dirty_saves();
            let saved = dirty.iter().map(|(i, _, _, f)| (*i, f.clone())).collect();
            let chapters = dirty
                .into_iter()
                .map(|(index, start, end, front)| ChapterWrite {
                    path: self.project.chapter_path(index),
                    text: text.clone(),
                    lines: Some((start, end)),
                    front,
                    count_as: Some(index),
                })
                .collect();
            (chapters, Saved::Window { chapters: saved })
        } else {
            let Some(path) = self.buffer.path.clone() else {
                self.save_failed(kind, "Save failed: no file path set".into());
                return;
            };
            let front = self
                .buffer
                .front_matter()
                .filter(|f| !f.is_empty())
                .map(|f| f.as_str().to_string());
            // The counts the snapshot will have on disk, from the buffer's own
            // (cached) count — no need to wait for the write.
            self.cache_focus_words();
            let chapter = ChapterWrite {
                path: path.clone(),
                text: self.buffer.rope().clone(),
                lines: None,
                front: front.clone(),
                count_as: None,
            };
            (vec![chapter], Saved::Single { path, front })
        };
        // A save in the same burst as a chapter switch must persist the new
        // chapter's notes, not the previous chapter's set (which has none).
        self.adopt_view_annotations_if_stale();
        let bases = self.chapter_bases();
        let annotations = self.stage_annotations_to(&bases);
        let key = FileKey::Save(chapters.iter().map(|c| c.path.clone()).collect());
        let pending_annotations = annotations.is_some();
        let job = SaveJob {
            chapters,
            annotations,
            todo_marker: self.project.manifest.todo_marker().to_string(),
        };
        let ticket = self.file_writer.submit(key, FileJob::Save(job));
        self.pending_saves.push(PendingSave {
            ticket,
            revision,
            saved,
            kind,
            annotations: pending_annotations,
        });
        if kind == SaveKind::Explicit {
            self.message = Some("Saving…".into());
        }
        #[cfg(test)]
        self.settle_if_eager();
    }

    /// Report a save that failed (or was refused): the message, and — for an
    /// explicit save — the on-save workflow, which ran after a failed save
    /// before saves went to the background too. The buffer stays dirty.
    fn save_failed(&mut self, kind: SaveKind, e: String) {
        match kind {
            // On failure autosave surfaces the error rather than failing silently
            // every idle tick while the author keeps typing, trusting it.
            SaveKind::Auto => {
                self.message = Some(format!(
                    "Autosave failed: {e} — save with Ctrl-S or check disk"
                ));
            }
            SaveKind::Explicit => {
                self.message = Some(e);
                self.trigger_on_save();
            }
        }
    }

    /// Apply whatever the project files writer has finished: a landed save
    /// marks the buffer clean (if it still holds the saved text) and runs the
    /// post-save tail; a failed one reports and leaves it dirty. A brief,
    /// uncontended lock — called every event-loop tick.
    pub(crate) fn poll_saves(&mut self) {
        for Done { ticket, outcome } in self.file_writer.take_done() {
            match outcome {
                Some(FileOutcome::Save(out)) => {
                    if let Some(p) = self.take_pending_save(ticket) {
                        self.finish_save(&p, out);
                    }
                }
                // A failed best-effort write: forget what was "last written", so
                // the next save writes it again.
                Some(FileOutcome::History(ok)) => {
                    if !ok {
                        *self.last_history_snapshot.borrow_mut() = None;
                    }
                }
                Some(FileOutcome::Position(ok)) => {
                    if !ok {
                        *self.last_position.borrow_mut() = None;
                    }
                }
                // Superseded by a newer save of the same files, which carries the
                // newer text (and staged notes). An explicit save's "Saved" and
                // on-save workflow carry over to it.
                None => {
                    if let Some(p) = self.take_pending_save(ticket) {
                        if p.kind == SaveKind::Explicit {
                            if let Some(newest) = self.pending_saves.last_mut() {
                                newest.kind = SaveKind::Explicit;
                            }
                        }
                    }
                }
            }
        }
    }

    fn take_pending_save(&mut self, ticket: u64) -> Option<PendingSave> {
        let i = self.pending_saves.iter().position(|p| p.ticket == ticket)?;
        Some(self.pending_saves.remove(i))
    }

    /// A save's outcome has arrived: apply it.
    fn finish_save(
        &mut self,
        p: &PendingSave,
        SaveOutcome {
            chapters,
            annotations,
        }: SaveOutcome,
    ) {
        let counts = match chapters {
            Ok(counts) => counts,
            Err(e) => {
                if p.annotations {
                    self.project.annotations_written(false);
                }
                self.save_failed(p.kind, e);
                return;
            }
        };
        for (index, words, todos) in counts {
            self.project.set_chapter_words(index, words);
            self.project.set_chapter_todo(index, todos);
        }
        // Clean only if the editor still holds exactly the text written; typing
        // since the snapshot keeps it dirty for the next save.
        match &p.saved {
            Saved::Single { path, front } => {
                let current = self.manuscript.is_none()
                    && self.buffer.revision() == p.revision
                    && self.buffer.path.as_ref() == Some(path)
                    && self
                        .buffer
                        .front_matter()
                        .filter(|f| !f.is_empty())
                        .map(FrontMatter::as_str)
                        == front.as_deref();
                if current {
                    self.buffer.dirty = false;
                }
                self.refresh_split_sibling();
            }
            Saved::Window { chapters } => {
                if let Some(m) = &mut self.manuscript {
                    if m.buffer().revision() == p.revision {
                        m.mark_saved(chapters);
                    }
                }
            }
        }
        let notes_failed = match annotations {
            Some(result) => {
                self.project.annotations_written(result.is_ok());
                result.err()
            }
            None => None,
        };
        self.finalize_save();
        if p.kind == SaveKind::Explicit {
            self.message = Some("Saved".into());
        }
        if let Some(e) = notes_failed {
            self.message = Some(format!("Couldn't save annotations: {e}"));
        }
        if p.kind == SaveKind::Explicit {
            // Run the on-save workflow trigger (#228) for the explicit save
            // (Ctrl-S / `:w`); implicit flushes (chapter switch, leaving the
            // continuous view) and autosave intentionally don't fire it. A no-op
            // when this save is itself a workflow's own step — no recursion.
            self.trigger_on_save();
        }
    }

    /// The post-write tail of a landed save: a clean buffer means there's
    /// nothing left to recover (#263), and either way we checkpoint today's
    /// momentum to the history log.
    fn finalize_save(&mut self) {
        // A save is the one moment pure typing can change git status; flag it so
        // the next idle tick recomputes the count off-thread (#779), rather than
        // re-walking the tree on every idle burst.
        self.mark_vc_status_dirty();
        // A save reached disk: note that an automatic off-site backup is owed
        // (#887). Just sets a flag — the actual push is decided and spawned off the
        // save path by the cadence hook, so a save never touches the network.
        self.mark_backup_on_save();
        // Only once nothing is dirty: if the author typed during the write, the
        // sidecar holds (or is getting) that newer text and must stay.
        if !self.edit_dirty() {
            self.clear_recovery();
        }
        self.persist_snapshot();
        // Keep the resume point current on every save, so an unclean exit after a
        // save still reopens where the author was working (#588).
        self.persist_position();
    }

    /// Wait (bounded by [`SAVE_SETTLE_WAIT`]) until every write handed to the
    /// project files writer has landed, then apply the outcomes. Returns whether
    /// it settled; if not, sets a message, and the caller must not go on as if
    /// the files were current. A single uncontended lock when nothing is
    /// pending, so it's cheap to call on every non-typing action.
    pub(crate) fn settle_saves(&mut self) -> bool {
        let settled = self.file_writer.is_idle() || self.file_writer.flush(SAVE_SETTLE_WAIT);
        self.poll_saves();
        if !settled {
            self.message =
                Some("Still saving — the disk isn't responding; try again in a moment".into());
        }
        settled
    }

    /// Settle saves at exit (bounded): returns a note for the terminal if a save
    /// that was still in flight didn't land, so the author knows the recovery
    /// sidecar holds those edits.
    pub fn finish_saves(&mut self) -> Option<String> {
        let pending = !self.pending_saves.is_empty();
        if !self.settle_saves() {
            return Some("Quill: the last save didn't finish (the disk isn't responding); your edits are in the recovery file.".into());
        }
        (pending && self.edit_dirty()).then(|| {
            let why = self.message.clone().unwrap_or_default();
            format!("Quill: the last save didn't complete ({why}); your edits are in the recovery file.")
        })
    }

    /// Test seam, after queueing a file write: tests settle it at once by
    /// default, so the many that read the disk right after a save keep working;
    /// the tests of the asynchronous behavior turn that off.
    #[cfg(test)]
    fn settle_if_eager(&mut self) {
        if self.settle_saves_eagerly {
            self.settle_saves();
        }
    }

    /// Write today's snapshot to the history log (best-effort), on the
    /// background writer.
    ///
    /// [`history::persist`] appends, and `load`/`compact` coalesce to one line per
    /// day — so a run of autosaves with unchanged totals would leave a stack of
    /// identical day lines to tidy later. Skip the append when this snapshot equals
    /// the last one we wrote (#869): the same totals produce the same coalesced
    /// record, so nothing is lost, and an untouched idle autosave stops growing the
    /// log. (Only equal snapshots are skipped, so the first write of the day and any
    /// changed write still land; a failed append forgets the last one, so the next
    /// retries.)
    #[allow(clippy::needless_pass_by_ref_mut)] // tests settle the write eagerly
    pub fn persist_snapshot(&mut self) {
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
        *self.last_history_snapshot.borrow_mut() = Some(snapshot.clone());
        let root = self.project.root.clone();
        let key = FileKey::History(root.clone(), snapshot.date.clone());
        self.file_writer.submit(
            key,
            FileJob::History {
                root,
                snap: snapshot,
            },
        );
        #[cfg(test)]
        self.settle_if_eager();
    }

    /// Record where the author is editing right now, so the next clean open can
    /// resume here (#588). Written on a clean save and at quit — the last-position
    /// counterpart to the crash-recovery sidecar, and separate from it: this
    /// remembers the *location* of a saved session, not unsaved *text*. In the
    /// single-chapter view the cursor is a byte offset into that chapter's prose;
    /// the continuous view (a multi-chapter window) records only the active
    /// chapter, resuming at its top. Best-effort, on the background writer — a
    /// failed write just means the next open starts at the top.
    ///
    /// Skipped when the position equals the one last written (or read at launch)
    /// for this project: a save or autosave that leaves the cursor where it was —
    /// the usual case for an idle autosave — costs a comparison, not a file write.
    #[allow(clippy::needless_pass_by_ref_mut)] // tests settle the write eagerly
    pub fn persist_position(&mut self) {
        let cursor = match &self.manuscript {
            Some(_) => 0,
            None => self.buffer.cursor_byte(),
        };
        let pos = lastpos::Position {
            chapter_file: self.project.active_file().to_string(),
            cursor,
        };
        let root = self.project.root.clone();
        if let Some((last_root, last)) = self.last_position.borrow().as_ref() {
            if *last_root == root && *last == pos {
                return;
            }
        }
        *self.last_position.borrow_mut() = Some((root.clone(), pos.clone()));
        self.file_writer.submit(
            FileKey::Position(root.clone()),
            FileJob::Position { root, pos },
        );
        #[cfg(test)]
        self.settle_if_eager();
    }
}
