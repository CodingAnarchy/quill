//! A single-shot background job (#152): run work on a worker thread and poll for
//! its result without blocking the UI. The shared mechanism behind the AI
//! requests ([`App::poll_ai`]) and the remote push/pull ([`App::poll_vc`]) — one
//! place owns the channel, the thread spawn, and the non-blocking receive.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

/// What a [`Task::poll`] found.
pub enum Poll<T> {
    /// Still running.
    Pending,
    /// Finished; carries the worker's result.
    Ready(T),
    /// The worker thread died without a result — it panicked, or was torn down
    /// before it could report. The job is cleared. Carries the panic's message so
    /// callers surface the real cause (#838) instead of a generic failure; `None`
    /// when no message is available (a non-string panic, or a lost sender).
    Died(Option<String>),
}

/// What the worker sends back over the channel: the work's value on success, or
/// the panic's message (when it was a string) if the closure unwound. Catching
/// the panic here — rather than letting it drop the sender — carries the reason
/// across the thread boundary so a failed remote op keeps its cause (#838).
enum Outcome<T> {
    Value(T),
    Panic(Option<String>),
}

/// A background job whose result is delivered once, then consumed. Idle until
/// [`spawn`](Task::spawn) is called.
pub struct Task<T> {
    rx: Option<Receiver<Outcome<T>>>,
}

/// Extract a human-readable message from a panic payload, matching what the
/// default panic hook prints: `&str` and `String` payloads carry the text; any
/// other payload is opaque, so we report nothing rather than a placeholder.
fn panic_message(payload: &(dyn Any + Send)) -> Option<String> {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
}

impl<T: Send + 'static> Task<T> {
    /// A task with nothing running.
    pub(crate) const fn idle() -> Self {
        Self { rx: None }
    }

    /// Whether a job is currently in flight.
    pub(crate) const fn is_busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Spawn `work` on a worker thread; its return value is delivered by the next
    /// [`poll`](Task::poll) that sees it. Replaces any prior receiver — callers
    /// guard with [`is_busy`](Task::is_busy) first when only one job should run.
    /// A panic in `work` is caught and its message carried back (#838), rather
    /// than silently dropping the sender.
    pub(crate) fn spawn(&mut self, work: impl FnOnce() -> T + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let outcome = match panic::catch_unwind(AssertUnwindSafe(work)) {
                Ok(value) => Outcome::Value(value),
                Err(payload) => Outcome::Panic(panic_message(payload.as_ref())),
            };
            let _ = tx.send(outcome);
        });
        self.rx = Some(rx);
    }

    /// Non-blocking check for the result. Clears the job on `Ready`/`Died`.
    pub(crate) fn poll(&mut self) -> Poll<T> {
        let Some(rx) = self.rx.as_ref() else {
            return Poll::Pending;
        };
        match rx.try_recv() {
            Ok(Outcome::Value(value)) => {
                self.rx = None;
                Poll::Ready(value)
            }
            // The worker caught its own panic and reported the cause.
            Ok(Outcome::Panic(msg)) => {
                self.rx = None;
                Poll::Died(msg)
            }
            Err(TryRecvError::Empty) => Poll::Pending,
            // The sender vanished without an outcome (e.g. the process is tearing
            // down the worker) — no cause to report.
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                Poll::Died(None)
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
                Poll::Pending => {}
                Poll::Died(_) => panic!("worker shouldn't die"),
            }
        };
        assert_eq!(value, 7);
        assert!(!task.is_busy(), "the job is cleared once consumed");
        assert!(matches!(task.poll(), Poll::Pending), "no second result");
    }

    #[test]
    fn a_panicking_worker_reports_died_with_the_cause() {
        let mut task: Task<i32> = Task::idle();
        // Silence the default hook so the deliberate panic doesn't print to the
        // test's stderr; the panic is still caught and its message carried back.
        let prior = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        task.spawn(|| panic!("boom"));
        let state = loop {
            match task.poll() {
                Poll::Pending => {}
                other => break other,
            }
        };
        panic::set_hook(prior);
        // The panic's message crosses the worker boundary (#838) rather than
        // being swallowed into a bare "died".
        match state {
            Poll::Died(msg) => assert_eq!(msg.as_deref(), Some("boom")),
            _ => panic!("a panicking worker should report Died"),
        }
        assert!(!task.is_busy());
    }
}
