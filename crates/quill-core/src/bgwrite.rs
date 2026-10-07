//! A background writer: one long-lived worker thread that performs file writes
//! handed to it by the UI thread, so a durable write on a busy disk never stalls
//! typing.
//!
//! A durable write — a temp file, `F_FULLFSYNC`, a rename, a directory `fsync` —
//! is ~10ms on an idle disk, but under contention every step can stall for tens
//! of milliseconds to seconds. The editor therefore never does one on the typing
//! path: it snapshots what to write (an O(1) rope clone plus a path) and submits a
//! [`Job`]; the worker performs it and reports the outcome by ticket.
//!
//! The queue is **latest-wins per key**: a newer job with the same key replaces
//! one still waiting (reported as superseded), so a burst of submissions while a
//! slow write is in flight costs one more write of the newest data, never a
//! backlog — and an older job can never land after a newer one with its key. A
//! replacement goes to the back of the queue, after everything submitted before
//! it. The worker holds the lock only to take a job or report one finished, never
//! across the I/O, so the UI thread's lock is effectively uncontended.
//!
//! The editor runs two of these: the crash-recovery sidecar
//! ([`recovery::writer`](crate::recovery::writer)) and the project's own files on
//! save (chapters, annotations, the history log, the resume position). They are
//! separate so a stalled chapter `fsync` can't hold up the recovery capture —
//! the safety net exactly while saves are slow — and vice versa.

use std::cell::Cell;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(any(test, feature = "test-support"))]
use crate::atomic;

/// One unit of background work.
pub trait Job: Send + 'static {
    /// What the job reports back to the UI thread.
    type Outcome: Send + 'static;

    /// Do the work, off the UI thread.
    fn perform(self) -> Self::Outcome;

    /// Test seam: whether this job writes a file — what [`Writer::fail_next`]
    /// arms its injected failure for, and what [`Writer::writes`] counts.
    #[cfg(any(test, feature = "test-support"))]
    fn is_write(&self) -> bool {
        true
    }
}

/// A finished (or superseded) job: its ticket and, unless a newer job with the
/// same key replaced it before it ran, its outcome.
pub struct Done<O> {
    pub ticket: u64,
    pub outcome: Option<O>,
}

struct Queued<K, J> {
    ticket: u64,
    key: K,
    job: J,
}

struct State<K, J: Job> {
    /// Waiting jobs, at most one per key (latest wins), oldest first.
    queue: Vec<Queued<K, J>>,
    /// A job has been taken and is being performed.
    busy: bool,
    /// Finished and superseded jobs, until the UI thread takes them.
    done: Vec<Done<J::Outcome>>,
    /// The ticket of the most recently submitted job.
    last_ticket: u64,
    /// The [`Writer`] was dropped: finish the queue, then exit.
    closed: bool,
    /// Test seam: the worker parks after taking a job until released.
    #[cfg(any(test, feature = "test-support"))]
    held: bool,
    /// Test seam: the next write fails (just before its rename).
    #[cfg(any(test, feature = "test-support"))]
    fail_next: bool,
    /// Test seam: how many writes the worker has performed.
    #[cfg(any(test, feature = "test-support"))]
    writes: usize,
}

struct Shared<K, J: Job> {
    state: Mutex<State<K, J>>,
    /// Signalled when a job is queued, finished, or the writer closes.
    changed: Condvar,
}

impl<K, J: Job> Shared<K, J> {
    fn lock(&self) -> MutexGuard<'_, State<K, J>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(&self, guard: MutexGuard<'a, State<K, J>>) -> MutexGuard<'a, State<K, J>> {
        self.changed
            .wait(guard)
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The UI thread's handle on a background writer: submit jobs without blocking,
/// take finished outcomes, and settle the queue at the moments that need it.
/// The worker thread starts on the first job and exits once the handle is
/// dropped and the queue is done.
pub struct Writer<K: PartialEq + Send + 'static, J: Job> {
    shared: Arc<Shared<K, J>>,
    spawned: Cell<bool>,
    name: &'static str,
}

impl<K: PartialEq + Send + 'static, J: Job> Writer<K, J> {
    /// A writer with no thread yet — the first job starts one named `name`, so
    /// an editor that never writes anything never spawns it.
    pub fn new(name: &'static str) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    queue: Vec::new(),
                    busy: false,
                    done: Vec::new(),
                    last_ticket: 0,
                    closed: false,
                    #[cfg(any(test, feature = "test-support"))]
                    held: false,
                    #[cfg(any(test, feature = "test-support"))]
                    fail_next: false,
                    #[cfg(any(test, feature = "test-support"))]
                    writes: 0,
                }),
                changed: Condvar::new(),
            }),
            spawned: Cell::new(false),
            name,
        }
    }

    /// Queue `job` under `key`, replacing (and reporting as superseded) any job
    /// still waiting with an equal key; a job already in flight finishes first.
    /// Returns the job's ticket at once.
    pub fn submit(&self, key: K, job: J) -> u64 {
        let ticket = {
            let mut state = self.shared.lock();
            state.last_ticket += 1;
            let ticket = state.last_ticket;
            if let Some(i) = state.queue.iter().position(|q| q.key == key) {
                let old = state.queue.remove(i);
                state.done.push(Done {
                    ticket: old.ticket,
                    outcome: None,
                });
            }
            state.queue.push(Queued { ticket, key, job });
            ticket
        };
        self.shared.changed.notify_all();
        self.ensure_worker();
        ticket
    }

    /// Start the worker thread if it isn't running. Should the OS refuse a
    /// thread, perform the queue right here instead: a slow write beats none.
    fn ensure_worker(&self) {
        if self.spawned.get() {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let spawned = thread::Builder::new()
            .name(self.name.into())
            .spawn(move || work(&shared, true));
        match spawned {
            Ok(_) => self.spawned.set(true),
            Err(_) => work(&self.shared, false),
        }
    }

    /// The ticket of the most recently submitted job (0 before any).
    pub fn last_ticket(&self) -> u64 {
        self.shared.lock().last_ticket
    }

    /// Take every finished or superseded job's report, oldest first.
    pub fn take_done(&self) -> Vec<Done<J::Outcome>> {
        std::mem::take(&mut self.shared.lock().done)
    }

    /// Whether nothing is queued or in flight.
    pub fn is_idle(&self) -> bool {
        let state = self.shared.lock();
        state.queue.is_empty() && !state.busy
    }

    /// Wait — at most `timeout` — until every queued job has been performed.
    /// Returns whether the queue drained. Never for the typing path.
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
    #[cfg(any(test, feature = "test-support"))]
    pub fn hold(&self) {
        self.shared.lock().held = true;
    }

    /// Test seam: let a [`hold`](Self::hold)-parked worker carry on.
    #[cfg(any(test, feature = "test-support"))]
    pub fn release(&self) {
        self.shared.lock().held = false;
        self.shared.changed.notify_all();
    }

    /// Test seam: a handle that releases a [`hold`](Self::hold) from another
    /// thread — so a test can bound how long a stuck write may block it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn releaser(&self) -> impl FnOnce() + Send + 'static {
        let shared = Arc::clone(&self.shared);
        move || {
            shared.lock().held = false;
            shared.changed.notify_all();
        }
    }

    /// Test seam: block until the worker has taken a job (and, if held, parked
    /// on it), so a test knows which job is in flight.
    #[cfg(any(test, feature = "test-support"))]
    pub fn wait_in_flight(&self) {
        let mut state = self.shared.lock();
        while !state.busy {
            state = self.shared.wait(state);
        }
    }

    /// Test seam: make the worker's next write fail before its rename.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_next(&self) {
        self.shared.lock().fail_next = true;
    }

    /// Test seam: how many writes the worker has performed.
    #[cfg(any(test, feature = "test-support"))]
    pub fn writes(&self) -> usize {
        self.shared.lock().writes
    }
}

impl<K: PartialEq + Send + 'static, J: Job> Drop for Writer<K, J> {
    /// Let the worker finish what's queued, then exit. Never waits for it: the
    /// moments that need the queue settled [`flush`](Writer::flush) first.
    fn drop(&mut self) {
        {
            let mut state = self.shared.lock();
            state.closed = true;
            #[cfg(any(test, feature = "test-support"))]
            {
                state.held = false;
            }
        }
        self.shared.changed.notify_all();
    }
}

/// The worker loop: take the next job, perform it with the lock released,
/// report its outcome, repeat. With `block`, waits for jobs until the writer
/// closes; without (the no-thread fallback), returns once the queue is empty.
fn work<K, J: Job>(shared: &Shared<K, J>, block: bool) {
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
            #[cfg(any(test, feature = "test-support"))]
            while state.held {
                state = shared.wait(state);
            }
            // The fail point is thread-local, so it's armed here, on the thread
            // that performs the write.
            #[cfg(any(test, feature = "test-support"))]
            if job.job.is_write() && std::mem::take(&mut state.fail_next) {
                atomic::fail_point::arm_pre_rename_failure();
            }
            job
        };
        #[cfg(any(test, feature = "test-support"))]
        let is_write = job.job.is_write();
        let outcome = job.job.perform();
        let mut state = shared.lock();
        state.busy = false;
        state.done.push(Done {
            ticket: job.ticket,
            outcome: Some(outcome),
        });
        #[cfg(any(test, feature = "test-support"))]
        if is_write {
            state.writes += 1;
        }
        drop(state);
        shared.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A job that just reports its number.
    struct Num(u32);

    impl Job for Num {
        type Outcome = u32;
        fn perform(self) -> u32 {
            self.0
        }
    }

    const WAIT: Duration = Duration::from_secs(10);

    #[test]
    fn a_waiting_job_is_superseded_and_the_replacement_runs_last() {
        let writer: Writer<&str, Num> = Writer::new("bgwrite-test");
        writer.hold();
        let first = writer.submit("a", Num(1)); // taken and parked
        writer.wait_in_flight();
        let stale = writer.submit("a", Num(2)); // waiting
        let other = writer.submit("b", Num(3)); // waiting, after it
        let newest = writer.submit("a", Num(4)); // replaces `stale`, goes last
        writer.release();
        assert!(writer.flush(WAIT));

        let done: Vec<_> = writer
            .take_done()
            .into_iter()
            .map(|d| (d.ticket, d.outcome))
            .collect();
        assert_eq!(
            done,
            vec![
                (stale, None),
                (first, Some(1)),
                (other, Some(3)),
                (newest, Some(4))
            ]
        );
        assert_eq!(writer.last_ticket(), newest);
        assert!(writer.is_idle());
    }
}
