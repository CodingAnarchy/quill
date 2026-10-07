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
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ropey::Rope;

#[cfg(test)]
use crate::atomic;
use crate::buffer::normalize_trailing_newline;

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
                        body: normalize_trailing_newline(&line_range(&text, s.start, s.end)),
                    })
                    .collect(),
            ),
        }
    }
}

/// Lines `[start, end)` of `rope` as a string (with their newlines), clamped to
/// its length — `Buffer::line_range_text` over a detached rope.
fn line_range(rope: &Rope, start: usize, end: usize) -> String {
    let last = rope.len_lines();
    let (start, end) = (start.min(last), end.min(last));
    if start >= end {
        return String::new();
    }
    rope.slice(rope.line_to_char(start)..rope.line_to_char(end))
        .to_string()
}

/// What a job does to its project's sidecar.
enum Op {
    Write(Snapshot),
    Clear,
}

/// One queued sidecar operation, numbered so a failure report can be matched
/// to the latest submission.
struct Job {
    ticket: u64,
    root: PathBuf,
    op: Op,
}

/// The state the UI thread and the worker share, behind one mutex. The worker
/// holds the lock only to take a job or report one finished — never across the
/// write — so the UI thread's lock is effectively uncontended.
#[derive(Default)]
struct State {
    /// Waiting jobs, at most one per project root (latest wins).
    queue: Vec<Job>,
    /// A job has been taken and is being performed.
    busy: bool,
    /// The ticket of the most recent write that failed, until polled.
    failed: Option<u64>,
    /// The [`Writer`] was dropped: finish the queue, then exit.
    closed: bool,
    /// Test seam: the worker parks after taking a job until released.
    #[cfg(test)]
    held: bool,
    /// Test seam: the next write fails (just before its rename).
    #[cfg(test)]
    fail_next: bool,
    /// Test seam: how many sidecar writes the worker has performed.
    #[cfg(test)]
    writes: usize,
}

struct Shared {
    state: Mutex<State>,
    /// Signalled when a job is queued, finished, or the writer closes.
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(&self, guard: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.changed
            .wait(guard)
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The editor's handle on the recovery writer: submit sidecar writes and
/// clears without blocking, poll for a failed write, and settle the queue at
/// the moments that need it. The worker thread starts on the first job and
/// exits once the handle is dropped and the queue is done.
pub struct Writer {
    shared: Arc<Shared>,
    spawned: bool,
    /// The ticket of the most recently submitted job.
    ticket: u64,
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
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
            }),
            spawned: false,
            ticket: 0,
        }
    }

    /// Queue a write of `snapshot` to the sidecar under `root`, replacing any
    /// job still waiting for that root. Returns at once.
    pub fn write(&mut self, root: PathBuf, snapshot: Snapshot) {
        self.submit(root, Op::Write(snapshot));
    }

    /// Queue removing the sidecar under `root`, superseding any write still
    /// waiting for that root; a write already in flight finishes first, so the
    /// clear always has the last word. Returns at once.
    pub fn clear(&mut self, root: PathBuf) {
        self.submit(root, Op::Clear);
    }

    fn submit(&mut self, root: PathBuf, op: Op) {
        self.ticket += 1;
        let job = Job {
            ticket: self.ticket,
            root,
            op,
        };
        {
            let mut state = self.shared.lock();
            match state.queue.iter_mut().find(|j| j.root == job.root) {
                Some(waiting) => *waiting = job,
                None => state.queue.push(job),
            }
        }
        self.shared.changed.notify_all();
        self.ensure_worker();
    }

    /// Start the worker thread if it isn't running. Should the OS refuse a
    /// thread, perform the queue right here instead: a slow write beats no
    /// crash protection.
    fn ensure_worker(&mut self) {
        if self.spawned {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let spawned = thread::Builder::new()
            .name("quill-recovery".into())
            .spawn(move || work(&shared, true));
        match spawned {
            Ok(_) => self.spawned = true,
            Err(_) => work(&self.shared, false),
        }
    }

    /// Whether the most recently submitted job was a write that failed —
    /// consuming the report. A failure of an older job is moot (a newer job for
    /// the same text, or a clear, has superseded it) and reads as `false`.
    pub fn take_failure(&self) -> bool {
        self.shared.lock().failed.take() == Some(self.ticket)
    }

    /// Wait — at most `timeout` — until every queued job has been performed.
    /// Returns whether the queue drained. Not for the typing path: callers are
    /// quit, project switch/rename, and backup restore, which must see the
    /// sidecar settled before they move on.
    pub fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        while !state.queue.is_empty() || state.busy {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Test seam: park the worker after it takes its next job (and every job
    /// after it) until [`release`](Self::release) — a disk that's stuck.
    #[cfg(test)]
    pub fn hold(&self) {
        self.shared.lock().held = true;
    }

    /// Test seam: let a [`hold`](Self::hold)-parked worker carry on.
    #[cfg(test)]
    pub fn release(&self) {
        self.shared.lock().held = false;
        self.shared.changed.notify_all();
    }

    /// Test seam: a handle that releases a [`hold`](Self::hold) from another
    /// thread — so a test can bound how long a stuck write may block it.
    #[cfg(test)]
    pub fn releaser(&self) -> impl FnOnce() + Send + 'static {
        let shared = Arc::clone(&self.shared);
        move || {
            shared.lock().held = false;
            shared.changed.notify_all();
        }
    }

    /// Test seam: block until the worker has taken a job (and, if held, parked
    /// on it), so a test knows which job is in flight.
    #[cfg(test)]
    pub fn wait_in_flight(&self) {
        let mut state = self.shared.lock();
        while !state.busy {
            state = self.shared.wait(state);
        }
    }

    /// Test seam: make the worker's next sidecar write fail before its rename.
    #[cfg(test)]
    pub fn fail_next_write(&self) {
        self.shared.lock().fail_next = true;
    }

    /// Test seam: how many sidecar writes the worker has performed.
    #[cfg(test)]
    pub fn writes(&self) -> usize {
        self.shared.lock().writes
    }
}

impl Drop for Writer {
    /// Let the worker finish what's queued, then exit. Never waits for it: the
    /// moments that need the queue settled [`flush`](Writer::flush) first.
    fn drop(&mut self) {
        {
            let mut state = self.shared.lock();
            state.closed = true;
            #[cfg(test)]
            {
                state.held = false;
            }
        }
        self.shared.changed.notify_all();
    }
}

/// The worker loop: take the next job, perform it with the lock released,
/// report a failed write, repeat. With `block`, waits for jobs until the writer
/// closes; without (the no-thread fallback), returns once the queue is empty.
fn work(shared: &Shared, block: bool) {
    loop {
        let job = {
            let mut state = shared.lock();
            while state.queue.is_empty() {
                if state.closed || !block {
                    return;
                }
                state = shared.wait(state);
            }
            let job = state.queue.remove(0);
            state.busy = true;
            shared.changed.notify_all();
            #[cfg(test)]
            while state.held {
                state = shared.wait(state);
            }
            // The fail point is thread-local, so it's armed here, on the thread
            // that performs the write.
            #[cfg(test)]
            if matches!(job.op, Op::Write(_)) && std::mem::take(&mut state.fail_next) {
                atomic::fail_point::arm_pre_rename_failure();
            }
            job
        };
        let ticket = job.ticket;
        #[cfg(test)]
        let is_write = matches!(job.op, Op::Write(_));
        let ok = perform(job);
        let mut state = shared.lock();
        state.busy = false;
        if !ok {
            state.failed = Some(ticket);
        }
        #[cfg(test)]
        if ok && is_write {
            state.writes += 1;
        }
        drop(state);
        shared.changed.notify_all();
    }
}

/// Carry out one job, reporting whether it succeeded. A clear can't fail in a
/// way worth retrying (the sidecar is gone either way), so it reads as success.
fn perform(job: Job) -> bool {
    match job.op {
        Op::Write(snapshot) => snapshot
            .into_record()
            .is_none_or(|rec| super::write(&job.root, &rec).is_ok()),
        Op::Clear => {
            super::clear(&job.root);
            true
        }
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

        let mut writer = Writer::new();
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
        let mut writer = Writer::new();
        writer.write(root.clone(), single("late\n"));
        assert!(writer.flush(WAIT));
        assert!(writer.take_failure(), "the late write reports failure");
        assert!(!root.exists(), "the folder stays gone");
    }
}
