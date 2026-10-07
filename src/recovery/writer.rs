//! The crash-recovery sidecar's background writer.
//!
//! A sidecar write is durable — a temp file, `F_FULLFSYNC`, a rename, a
//! directory `fsync` — and on a busy disk every one of those steps can stall
//! for tens of milliseconds to seconds. Done on the UI thread, that stall froze
//! typing: keys queued in the pty and echoed in one burst. So the editor never
//! writes the sidecar itself. Its idle capture hands this writer a cheap
//! [`Snapshot`] (an O(1) rope clone plus the chapter files to attribute it to)
//! and returns at once; one long-lived worker thread serializes the snapshot
//! and calls the same durable [`recovery::write`](super::write).
//!
//! The queue is a **latest-wins slot per project root**: a newer job for a root
//! replaces one still waiting, so a burst of captures while a slow write is in
//! flight costs one more write (of the newest text), never a backlog. A clear
//! (a clean save, an explicit discard) is a job too, so it runs *after* any
//! write already in flight for that root and supersedes any still waiting — a
//! late write can never resurrect a sidecar after the save that cleared it.
//!
//! The UI thread never waits on the worker on the typing path. It learns about
//! a failed write by polling [`Writer::take_failure`] (a brief lock the worker
//! only ever holds between jobs, never across I/O), and the few non-typing
//! moments that must see the disk settled — quitting, switching or renaming the
//! project, restoring a backup — call the bounded [`Writer::flush`].

use std::path::PathBuf;
use std::time::Duration;

use ropey::Rope;

use crate::bgwrite;
use crate::buffer::normalize_trailing_newline;
use crate::text::rope_line_range;

use super::{Chapter, Recovery};

/// The unsaved text a capture hands the writer — cheap to take on the UI
/// thread (a rope clone shares its tree), turned into a [`Recovery`] record
/// (the string copy, the normalization, the JSON) on the worker.
pub enum Snapshot {
    /// The single-chapter view: the active chapter's whole buffer.
    Single { chapter_file: String, text: Rope },
    /// The continuous view: the window's buffer and each dirty segment in it.
    Segments { text: Rope, segments: Vec<Segment> },
}

/// One dirty segment of a continuous-view [`Snapshot`]: the chapter file its
/// text belongs to and its line span `[start, end)` in the window buffer.
pub struct Segment {
    pub chapter_file: String,
    pub start: usize,
    pub end: usize,
}

impl Snapshot {
    /// The record to write, or `None` when there is no dirty text in it.
    fn into_record(self) -> Option<Recovery> {
        match self {
            Self::Single { chapter_file, text } => {
                Some(Recovery::single(chapter_file, text.to_string()))
            }
            Self::Segments { text, segments } => Recovery::from_chapters(
                segments
                    .into_iter()
                    .map(|s| Chapter {
                        chapter_file: s.chapter_file,
                        // Normalized the way a save computes the body, matching
                        // what `Manuscript::dirty_segments` reports.
                        body: normalize_trailing_newline(&rope_line_range(&text, s.start, s.end)),
                    })
                    .collect(),
            ),
        }
    }
}

/// What a job does to its project's sidecar.
pub enum Op {
    Write(Snapshot),
    Clear,
}

/// One queued sidecar operation for the project at `root`.
pub struct Job {
    root: PathBuf,
    op: Op,
}

impl bgwrite::Job for Job {
    /// Whether the job succeeded. A clear can't fail in a way worth retrying
    /// (the sidecar is gone either way), so it reads as success.
    type Outcome = bool;

    fn perform(self) -> bool {
        match self.op {
            Op::Write(snapshot) => snapshot
                .into_record()
                .is_none_or(|rec| super::write(&self.root, &rec).is_ok()),
            Op::Clear => {
                super::clear(&self.root);
                true
            }
        }
    }

    #[cfg(test)]
    fn is_write(&self) -> bool {
        matches!(self.op, Op::Write(_))
    }
}

/// The editor's handle on the recovery writer: submit sidecar writes and
/// clears without blocking, poll for a failed write, and settle the queue at
/// the moments that need it — a [`bgwrite::Writer`] keyed by project root.
pub struct Writer {
    inner: bgwrite::Writer<PathBuf, Job>,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Writer {
    /// A writer with no thread yet — the first job starts it, so an editor
    /// that never captures anything never spawns one.
    pub fn new() -> Self {
        Self {
            inner: bgwrite::Writer::new("quill-recovery"),
        }
    }

    /// Queue a write of `snapshot` to the sidecar under `root`, replacing any
    /// job still waiting for that root. Returns at once.
    pub fn write(&self, root: PathBuf, snapshot: Snapshot) {
        self.submit(root, Op::Write(snapshot));
    }

    /// Queue removing the sidecar under `root`, superseding any write still
    /// waiting for that root; a write already in flight finishes first, so the
    /// clear always has the last word. Returns at once.
    pub fn clear(&self, root: PathBuf) {
        self.submit(root, Op::Clear);
    }

    fn submit(&self, root: PathBuf, op: Op) {
        self.inner.submit(root.clone(), Job { root, op });
    }

    /// Whether the most recently submitted job was a write that failed —
    /// consuming the report. A failure of an older job is moot (a newer job for
    /// the same text, or a clear, has superseded it) and reads as `false`.
    pub fn take_failure(&self) -> bool {
        let latest = self.inner.last_ticket();
        self.inner
            .take_done()
            .into_iter()
            .any(|d| d.ticket == latest && d.outcome == Some(false))
    }

    /// Wait — at most `timeout` — until every queued job has been performed.
    /// Returns whether the queue drained. Not for the typing path: callers are
    /// quit, project switch/rename, and backup restore, which must see the
    /// sidecar settled before they move on.
    pub fn flush(&self, timeout: Duration) -> bool {
        self.inner.flush(timeout)
    }

    /// Test seam: park the worker after it takes its next job (and every job
    /// after it) until [`release`](Self::release) — a disk that's stuck.
    #[cfg(test)]
    pub fn hold(&self) {
        self.inner.hold();
    }

    /// Test seam: let a [`hold`](Self::hold)-parked worker carry on.
    #[cfg(test)]
    pub fn release(&self) {
        self.inner.release();
    }

    /// Test seam: a handle that releases a [`hold`](Self::hold) from another
    /// thread — so a test can bound how long a stuck write may block it.
    #[cfg(test)]
    pub fn releaser(&self) -> impl FnOnce() + Send + 'static {
        self.inner.releaser()
    }

    /// Test seam: block until the worker has taken a job (and, if held, parked
    /// on it), so a test knows which job is in flight.
    #[cfg(test)]
    pub fn wait_in_flight(&self) {
        self.inner.wait_in_flight();
    }

    /// Test seam: make the worker's next sidecar write fail before its rename.
    #[cfg(test)]
    pub fn fail_next_write(&self) {
        self.inner.fail_next();
    }

    /// Test seam: how many sidecar writes the worker has performed.
    #[cfg(test)]
    pub fn writes(&self) -> usize {
        self.inner.writes()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::recovery;
    use crate::testutil::scratch_dir;

    const WAIT: Duration = Duration::from_secs(10);

    fn single(text: &str) -> Snapshot {
        Snapshot::Single {
            chapter_file: "one.md".into(),
            text: Rope::from_str(text),
        }
    }

    #[test]
    fn segments_are_sliced_and_normalized_like_a_save() {
        let snapshot = Snapshot::Segments {
            text: Rope::from_str("alpha\n\nbeta\ngamma\n"),
            segments: vec![
                Segment {
                    chapter_file: "a.md".into(),
                    start: 0,
                    end: 2,
                },
                Segment {
                    chapter_file: "c.md".into(),
                    start: 3,
                    end: 5,
                },
            ],
        };
        let rec = snapshot.into_record().unwrap();
        let got: Vec<_> = rec.chapters().map(|c| (c.chapter_file, c.body)).collect();
        assert_eq!(
            got,
            vec![
                ("a.md".into(), "alpha\n".into()),
                ("c.md".into(), "gamma\n".into())
            ]
        );
    }

    #[test]
    fn a_job_for_another_project_never_supersedes_this_ones() {
        // Latest-wins is per project root: a clear queued for A must survive a
        // write queued for B behind it (a project switch mid-write).
        let a = scratch_dir("recwriter", "root-a");
        let b = scratch_dir("recwriter", "root-b");
        recovery::write(
            &a,
            &recovery::Recovery::single("one.md".into(), "x\n".into()),
        )
        .unwrap();

        let writer = Writer::new();
        writer.hold();
        writer.write(b.clone(), single("first\n")); // taken and parked
        writer.wait_in_flight();
        writer.clear(a.clone());
        writer.write(b.clone(), single("second\n")); // replaces only B's job
        writer.release();
        assert!(writer.flush(WAIT));

        assert!(recovery::read(&a).is_none(), "A's clear still ran");
        assert_eq!(recovery::read(&b).unwrap().body, "second\n");
        assert_eq!(writer.writes(), 2, "B's first (in flight) and newest");
        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
    }

    #[test]
    fn a_write_never_recreates_a_vanished_project_folder() {
        let root = scratch_dir("recwriter", "vanished");
        fs::remove_dir_all(&root).unwrap();
        let writer = Writer::new();
        writer.write(root.clone(), single("late\n"));
        assert!(writer.flush(WAIT));
        assert!(writer.take_failure(), "the late write reports failure");
        assert!(!root.exists(), "the folder stays gone");
    }
}
