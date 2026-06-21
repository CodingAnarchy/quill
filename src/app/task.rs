//! A single-shot background job (#152): run work on a worker thread and poll for
//! its result without blocking the UI. The shared mechanism behind the AI
//! requests ([`App::poll_ai`]) and the remote push/pull ([`App::poll_vc`]) — one
//! place owns the channel, the thread spawn, and the non-blocking receive.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

/// What a [`Task::poll`] found.
pub(crate) enum Poll<T> {
    /// Still running.
    Pending,
    /// Finished; carries the worker's result.
    Ready(T),
    /// The worker thread dropped its sender without a result (it panicked) — the
    /// job is cleared; callers surface a generic failure.
    Died,
}

/// A background job whose result is delivered once, then consumed. Idle until
/// [`spawn`](Task::spawn) is called.
pub(crate) struct Task<T> {
    rx: Option<Receiver<T>>,
}

impl<T: Send + 'static> Task<T> {
    /// A task with nothing running.
    pub(crate) fn idle() -> Self {
        Self { rx: None }
    }

    /// Whether a job is currently in flight.
    pub(crate) fn is_busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Spawn `work` on a worker thread; its return value is delivered by the next
    /// [`poll`](Task::poll) that sees it. Replaces any prior receiver — callers
    /// guard with [`is_busy`](Task::is_busy) first when only one job should run.
    pub(crate) fn spawn(&mut self, work: impl FnOnce() -> T + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(work());
        });
        self.rx = Some(rx);
    }

    /// Non-blocking check for the result. Clears the job on `Ready`/`Died`.
    pub(crate) fn poll(&mut self) -> Poll<T> {
        let Some(rx) = self.rx.as_ref() else {
            return Poll::Pending;
        };
        match rx.try_recv() {
            Ok(value) => {
                self.rx = None;
                Poll::Ready(value)
            }
            Err(TryRecvError::Empty) => Poll::Pending,
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                Poll::Died
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_task_is_not_busy_and_polls_pending() {
        let mut task: Task<i32> = Task::idle();
        assert!(!task.is_busy());
        assert!(matches!(task.poll(), Poll::Pending));
    }

    #[test]
    fn spawned_work_is_delivered_once_then_cleared() {
        let mut task = Task::idle();
        task.spawn(|| 7);
        assert!(task.is_busy());
        // Spin until the worker finishes (it's a trivial closure).
        let value = loop {
            match task.poll() {
                Poll::Ready(v) => break v,
                Poll::Pending => continue,
                Poll::Died => panic!("worker shouldn't die"),
            }
        };
        assert_eq!(value, 7);
        assert!(!task.is_busy(), "the job is cleared once consumed");
        assert!(matches!(task.poll(), Poll::Pending), "no second result");
    }

    #[test]
    fn a_panicking_worker_reports_died() {
        let mut task: Task<i32> = Task::idle();
        task.spawn(|| panic!("boom"));
        let state = loop {
            match task.poll() {
                Poll::Pending => continue,
                other => break other,
            }
        };
        assert!(matches!(state, Poll::Died));
        assert!(!task.is_busy());
    }
}
