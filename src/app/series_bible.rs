//! The "Refresh series bible" driver (#340): bring the whole series' story bible
//! up to date by running each book's bible pass (#339) in reading order against
//! the accumulating shared bible — one book per AI call, never the series at once
//! — with per-book progress and Esc-to-cancel. It mirrors the workflow runner's
//! async sequencing (a run-state advanced from `poll_ai` after each pass applies).
//! The action is a no-op for a standalone novel, whose per-book passes are the
//! fast everyday path.

use std::time::{Duration, Instant};

use super::task::Task;
use super::*;

use crate::mode::AiAction;

/// A wedged pass shouldn't stall the refresh; drop it past this and stop (#340).
const STEP_TIMEOUT: Duration = Duration::from_secs(120);

/// The bible trackers a series refresh runs, each accumulated across the books in
/// reading order. The bibles are independent, so the order between trackers is
/// free; within a tracker the books must go in order so each builds on the last.
const TRACKERS: [AiAction; 3] = [AiAction::Characters, AiAction::Continuity, AiAction::Setting];

/// A series-bible refresh in progress (#340): the queue of `(tracker, book)`
/// passes in order, the cursor through them, and the suspended-on-async
/// bookkeeping. Lives in `App::series_bible_run` for the run's lifetime.
pub(crate) struct SeriesBibleRun {
    queue: Vec<(AiAction, usize)>,
    /// The pass about to run (or in flight); `queue.len()` when done. Every step
    /// past a pass bumps this and exactly one of `skipped`/(an implicit "ran"), so
    /// the passes that sent a request are always `index - skipped`.
    index: usize,
    /// Passes skipped because that book was already current.
    skipped: usize,
    /// Set on Esc: stop cleanly after the in-flight pass finishes.
    cancelled: bool,
    /// When the in-flight pass was spawned, for the no-hang timeout.
    started_at: Instant,
}

impl SeriesBibleRun {
    fn new(queue: Vec<(AiAction, usize)>) -> Self {
        Self { queue, index: 0, skipped: 0, cancelled: false, started_at: Instant::now() }
    }
}

impl App {
    /// Start a series-bible refresh (#340): enqueue each tracker × book in reading
    /// order and run the first. A no-op (with a message) for a standalone novel or
    /// when a run is already going. (Availability is also gated by `action_status`.)
    pub(crate) fn start_refresh_series_bible(&mut self) {
        if self.project.book_count() <= 1 {
            self.message = Some("This project has only one book — use the per-book bible passes.".into());
            return;
        }
        if self.series_bible_run.is_some() || self.workflow_run.is_some() {
            self.message = Some("A pass is already running.".into());
            return;
        }
        if let Err(e) = self.ai.check_ready() {
            self.message = Some(e.to_string());
            return;
        }
        let books = self.project.book_count();
        let queue = TRACKERS.iter().flat_map(|&t| (0..books).map(move |b| (t, b))).collect();
        self.series_bible_run = Some(SeriesBibleRun::new(queue));
        self.run_series_bible_step();
    }

    /// Run queued passes until one spawns an async request (then suspend until
    /// `poll_ai` resumes us) or the queue drains. An up-to-date book has nothing to
    /// send, so it's counted skipped and we move straight on.
    fn run_series_bible_step(&mut self) {
        loop {
            let (action, book) = match self.series_bible_run.as_mut() {
                Some(run) if run.index < run.queue.len() => {
                    run.started_at = Instant::now();
                    run.queue[run.index]
                }
                Some(_) => {
                    self.finish_series_bible(); // queue drained
                    return;
                }
                None => return,
            };
            self.run_book_bible_pass(action, book, false);
            if self.ai_task.is_busy() {
                // The pass is in flight; show progress and wait for `poll_ai`.
                if let Some(run) = self.series_bible_run.as_ref() {
                    let (done, total) = (run.index + 1, run.queue.len());
                    let title = self.project.book_title(book).unwrap_or("");
                    self.message = Some(format!(
                        "Refreshing series bible — {done}/{total}: {} · {title}… (Esc to cancel)",
                        tracker_label(action),
                    ));
                }
                return;
            }
            // Not busy → that book was already current (nothing to send): skip on.
            if let Some(run) = self.series_bible_run.as_mut() {
                run.skipped += 1;
                run.index += 1;
            }
        }
    }

    /// Resume after an in-flight series-bible pass finished (called from `poll_ai`).
    /// A no-op when no refresh is running. A pass error stops the run; a pending
    /// cancel stops cleanly; otherwise it runs the next pass.
    pub(crate) fn advance_series_bible(&mut self, step_error: Option<String>) {
        let Some(run) = self.series_bible_run.as_mut() else {
            return;
        };
        if let Some(e) = step_error {
            let at = run.index + 1;
            self.series_bible_run = None;
            self.message = Some(format!("Series bible refresh stopped at pass {at}: {e}"));
            return;
        }
        run.index += 1;
        if run.cancelled {
            self.finish_series_bible();
            return;
        }
        self.run_series_bible_step();
    }

    /// Ask a running refresh to stop after the in-flight pass finishes (#340) — the
    /// clean Esc cancel, mirroring the workflow cancel.
    pub(crate) fn cancel_series_bible(&mut self) {
        if let Some(run) = self.series_bible_run.as_mut() {
            run.cancelled = true;
            self.message = Some("Series bible refresh: cancelling after this pass…".into());
        }
    }

    /// Whether a series-bible refresh is running (Esc routes to its cancel).
    pub(crate) fn series_bible_running(&self) -> bool {
        self.series_bible_run.is_some()
    }

    /// The no-hang guardrail: drop a wedged in-flight pass and stop the run.
    pub(crate) fn tick_series_bible_timeout(&mut self) {
        let Some(run) = self.series_bible_run.as_ref() else {
            return;
        };
        if !self.ai_task.is_busy() || run.started_at.elapsed() < STEP_TIMEOUT {
            return;
        }
        self.ai_task = Task::idle();
        let at = run.index + 1;
        self.series_bible_run = None;
        self.message = Some(format!("Series bible refresh timed out at pass {at}"));
    }

    /// Clear the run and report what it did.
    fn finish_series_bible(&mut self) {
        if let Some(run) = self.series_bible_run.take() {
            let (skipped, cancelled) = (run.skipped, run.cancelled);
            let ran = run.index - skipped;
            let tail = if skipped > 0 { format!(" ({skipped} already current)") } else { String::new() };
            let verb = if cancelled { "cancelled —" } else { "refreshed —" };
            self.message = Some(format!("Series bible {verb} {ran} {}{tail}", passes(ran)));
        }
    }
}

#[cfg(test)]
impl App {
    /// Seed a series-bible run so the state machine (advance / cancel / error) can
    /// be exercised without a live AI backend.
    pub(crate) fn seed_series_bible_run(&mut self, queue: Vec<(AiAction, usize)>) {
        self.series_bible_run = Some(SeriesBibleRun::new(queue));
    }

    /// Whether a run is recorded (test inspection).
    pub(crate) fn has_series_bible_run(&self) -> bool {
        self.series_bible_run.is_some()
    }
}

fn passes(n: usize) -> &'static str {
    if n == 1 {
        "pass"
    } else {
        "passes"
    }
}

fn tracker_label(action: AiAction) -> &'static str {
    match action {
        AiAction::Characters => "characters",
        AiAction::Continuity => "timeline",
        AiAction::Setting => "setting",
        _ => "bible",
    }
}
