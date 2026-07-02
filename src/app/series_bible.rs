//! The "Refresh series bible" driver (#340): bring the whole series' story bible
//! up to date by running each book's bible pass (#339) in reading order against
//! the accumulating shared bible — one book per AI call, never the series at once
//! — with per-book progress and Esc-to-cancel. The async sequencing (advance,
//! timeout, cancel) is the shared [`Sequenced`](super::sequenced) skeleton (#344);
//! this module supplies only the per-book step (`run_series_bible_step`) and the
//! refresh's messages. The action is a no-op for a standalone novel, whose
//! per-book passes are the fast everyday path.

use super::sequenced::{plural, Sequenced, SequencedRun};
use super::*;

use crate::ai::cost::{self, Estimate};
use crate::mode::AiAction;

/// The bible trackers a series refresh runs, each accumulated across the books in
/// reading order. The bibles are independent, so the order between trackers is
/// free; within a tracker the books must go in order so each builds on the last.
const TRACKERS: [AiAction; 4] = [
    AiAction::Characters,
    AiAction::Continuity,
    AiAction::Setting,
    AiAction::Plot,
];

/// A series-bible refresh in progress (#340): the queue of `(tracker, book)`
/// passes in order, plus the shared sequenced-run bookkeeping (#344). Lives in
/// `App::series_bible_run` for the run's lifetime.
pub(crate) struct SeriesBibleRun {
    queue: Vec<(AiAction, usize)>,
    /// Cursor, tallies, cancel flag, and the in-flight pass's start time (#344).
    seq: SequencedRun,
    /// The triage length when the run started, so the summary can report how many
    /// *new* cross-book findings (#341) this refresh added.
    triage_before: usize,
    /// Force a full-manuscript rebuild for every pass (#176/#655) — the all-bibles
    /// refresh's `R`. The series refresh always runs incrementally, so it's `false`.
    force: bool,
}

impl SeriesBibleRun {
    fn new(queue: Vec<(AiAction, usize)>, triage_before: usize) -> Self {
        Self::new_forced(queue, triage_before, false)
    }

    fn new_forced(queue: Vec<(AiAction, usize)>, triage_before: usize, force: bool) -> Self {
        Self {
            queue,
            seq: SequencedRun::new(),
            triage_before,
            force,
        }
    }
}

/// The series-bible refresh's [`Sequenced`] behavior (#344) — the marker the
/// generic advance/timeout/cancel dispatch through.
pub(crate) struct SeriesBibleDriver;

impl Sequenced for SeriesBibleDriver {
    fn run_mut(app: &mut App) -> Option<&mut SequencedRun> {
        app.series_bible_run.as_mut().map(|r| &mut r.seq)
    }
    fn run_ref(app: &App) -> Option<&SequencedRun> {
        app.series_bible_run.as_ref().map(|r| &r.seq)
    }
    fn drive(app: &mut App) {
        app.run_series_bible_step();
    }
    fn stop(app: &mut App, reason: &str) {
        if let Some(run) = app.series_bible_run.take() {
            let at = run.seq.index + 1;
            app.message = Some(format!(
                "Series bible refresh stopped at pass {at}: {reason}"
            ));
        }
    }
    fn finish(app: &mut App) {
        app.finish_series_bible();
    }
    fn cancelling_message(_app: &App) -> String {
        "Series bible refresh: cancelling after this pass…".to_string()
    }
}

impl App {
    /// Start a series-bible refresh (#340): enqueue each tracker × book in reading
    /// order and run the first. A no-op (with a message) for a standalone novel or
    /// when a run is already going. (Availability is also gated by `action_status`.)
    pub(crate) fn start_refresh_series_bible(&mut self) {
        if self.project.book_count() <= 1 {
            self.message =
                Some("This project has only one book — use the per-book bible passes.".into());
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
        let queue = self.bible_refresh_queue();
        self.series_bible_run = Some(SeriesBibleRun::new(queue, self.triage.len()));
        self.run_series_bible_step();
    }

    /// The `(tracker, book)` queue a bible refresh runs — each of the four bible
    /// trackers over every book, in reading order within a tracker so each book
    /// builds on the last. Shared by the series refresh (#340) and the all-bibles
    /// refresh (#655); for a standalone novel it's the four trackers over its one
    /// book.
    fn bible_refresh_queue(&self) -> Vec<(AiAction, usize)> {
        let books = self.project.book_count();
        TRACKERS
            .iter()
            .flat_map(|&t| (0..books).map(move |b| (t, b)))
            .collect()
    }

    /// Start an all-bibles refresh (#655): run the four bible passes as one batch —
    /// over every book of a series — with a single combined cost estimate/confirm
    /// and combined progress, rather than four (or 4×books) separate confirms and
    /// spawns. Gates on a ready backend and no other run in flight, then either
    /// confirms the summed estimate or, when it's under the threshold, starts
    /// straight away. `force` rebuilds each bible from the whole manuscript (#176).
    pub(crate) fn start_refresh_bibles(&mut self, force: bool) {
        if self.series_bible_run.is_some() || self.workflow_run.is_some() {
            self.message = Some("A pass is already running.".into());
            return;
        }
        if let Err(e) = self.ai.check_ready() {
            self.message = Some(e.to_string());
            return;
        }
        let queue = self.bible_refresh_queue();
        // Sum the per-pass estimates into one figure the author confirms once.
        let est = self.bibles_refresh_estimate(&queue, force);
        if est.tokens == 0 {
            self.message = Some("Story bibles are current — no chapters changed.".into());
            return;
        }
        // One gate for the whole batch: confirm only when the combined estimate
        // exceeds the author's threshold, mirroring the single-pass cost policy.
        if est.dollars > self.ai.cost_threshold {
            let label = format!(
                "Refresh all bibles — {} — run? (y/N)",
                cost::label(&est, &self.ai.model)
            );
            self.open_prompt(Pending::ConfirmRefreshBibles { force }, label, "", None);
        } else {
            self.refresh_bibles_confirmed(force);
        }
    }

    /// Begin the confirmed all-bibles batch (#655): seed the shared series-bible run
    /// with the four-tracker × book queue and drive its first pass. Reached from the
    /// combined cost confirm, or directly when the estimate was under the threshold.
    pub(crate) fn refresh_bibles_confirmed(&mut self, force: bool) {
        if self.series_bible_run.is_some() || self.workflow_run.is_some() {
            self.message = Some("A pass is already running.".into());
            return;
        }
        let queue = self.bible_refresh_queue();
        self.series_bible_run = Some(SeriesBibleRun::new_forced(queue, self.triage.len(), force));
        self.run_series_bible_step();
    }

    /// The combined cost estimate for a bible-refresh batch (#655): the summed
    /// input size of every queued `(tracker, book)` pass — each scoped exactly as it
    /// will run (incremental delta, full rebuild, or up-to-date/zero) — resolved to
    /// one token/dollar figure. A forced run bills each pass over the whole book;
    /// an all-current batch estimates zero, so nothing is spent. The batch discount
    /// (#691) applies only when every pass that will actually run does so batched
    /// (a from-scratch full rebuild on Claude), so the one combined figure isn't
    /// under-quoted by half-pricing a pass that will bill full rate.
    fn bibles_refresh_estimate(&self, queue: &[(AiAction, usize)], force: bool) -> Estimate {
        // The passes that will actually send something (an up-to-date pass sends
        // nothing, so it neither adds size nor affects the batch rate).
        let mut running = queue
            .iter()
            .filter(|&&(action, book)| self.book_pass_context_len(action, book, force) > 0)
            .peekable();
        let batch = running.peek().is_some()
            && running.all(|&(action, book)| self.book_pass_batches(action, book, force));
        let chars: usize = queue
            .iter()
            .map(|&(action, book)| self.book_pass_context_len(action, book, force))
            .sum();
        cost::estimate(chars, ai::MAX_OUTPUT_TOKENS, &self.ai.model, batch)
    }

    /// Run queued passes until one spawns an async request (then suspend until
    /// `poll_ai` resumes us) or the queue drains. An up-to-date book has nothing to
    /// send, so it's counted skipped and we move straight on.
    fn run_series_bible_step(&mut self) {
        loop {
            let (action, book, force) = match self.series_bible_run.as_mut() {
                Some(run) if run.seq.index < run.queue.len() => {
                    run.seq.touch();
                    let (action, book) = run.queue[run.seq.index];
                    (action, book, run.force)
                }
                Some(_) => {
                    self.finish_series_bible(); // queue drained
                    return;
                }
                None => return,
            };
            self.run_book_bible_pass(action, book, force);
            if self.ai_task.is_busy() {
                // The pass is in flight; show progress and wait for `poll_ai`.
                if let Some(run) = self.series_bible_run.as_ref() {
                    let (done, total) = (run.seq.index + 1, run.queue.len());
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
                run.seq.skipped += 1;
                run.seq.index += 1;
            }
        }
    }

    /// Whether a series-bible refresh is running (Esc routes to its cancel).
    pub(crate) fn series_bible_running(&self) -> bool {
        self.series_bible_run.is_some()
    }

    /// The book whose pass is in flight during a series-bible refresh (#341), so
    /// the continuity/setting findings it produces can be attributed to that book
    /// (their chapter titles can collide with another book's). `None` outside a
    /// refresh — an active-book pass's findings target the active book, as before.
    ///
    /// Valid only while a pass is being *applied* — `poll_ai` runs the `apply_*`
    /// that reads this before the run advances `index` to the next pass, so
    /// `queue[index]` is the book that just landed.
    pub(crate) fn current_series_bible_book(&self) -> Option<usize> {
        let run = self.series_bible_run.as_ref()?;
        run.queue.get(run.seq.index).map(|&(_, book)| book)
    }

    /// Clear the run and report what it did — the [`SeriesBibleDriver`]'s
    /// [`Sequenced::finish`] hook, and the normal drain-complete path (#344).
    fn finish_series_bible(&mut self) {
        if let Some(run) = self.series_bible_run.take() {
            let (ran, skipped, cancelled) = (run.seq.ran, run.seq.skipped, run.seq.cancelled);
            let tail = if skipped > 0 {
                format!(" ({skipped} already current)")
            } else {
                String::new()
            };
            // New cross-book findings this run added to revision triage (#341).
            let found = self.triage.len().saturating_sub(run.triage_before);
            let findings = if found > 0 {
                format!("; {found} in revision triage")
            } else {
                String::new()
            };
            let verb = if cancelled {
                "cancelled —"
            } else {
                "refreshed —"
            };
            self.message = Some(format!(
                "Series bible {verb} {ran} {}{tail}{findings}",
                plural(ran, "pass", "passes")
            ));
        }
    }
}

#[cfg(test)]
impl App {
    /// Seed a series-bible run so the state machine (advance / cancel / error) can
    /// be exercised without a live AI backend.
    pub(crate) fn seed_series_bible_run(&mut self, queue: Vec<(AiAction, usize)>) {
        let before = self.triage.len();
        self.series_bible_run = Some(SeriesBibleRun::new(queue, before));
    }

    /// Whether a run is recorded (test inspection).
    pub(crate) fn has_series_bible_run(&self) -> bool {
        self.series_bible_run.is_some()
    }

    /// The `(tracker, book)` batch a bible refresh would run — for tests to assert
    /// the all-bibles command (#655) plans the four trackers over every book.
    pub(crate) fn bible_refresh_queue_for_test(&self) -> Vec<(AiAction, usize)> {
        self.bible_refresh_queue()
    }

    /// The combined cost estimate the all-bibles refresh (#655) would confirm — for
    /// tests to assert it sums the queued passes.
    pub(crate) fn bibles_refresh_estimate_for_test(&self, force: bool) -> Estimate {
        let queue = self.bible_refresh_queue();
        self.bibles_refresh_estimate(&queue, force)
    }

    /// Test seam: backdate the in-flight pass so the per-step timeout has elapsed.
    pub(crate) fn expire_series_bible_step(&mut self) {
        use super::sequenced::STEP_TIMEOUT;
        use std::time::Instant;
        if let Some(run) = self.series_bible_run.as_mut() {
            run.seq.started_at = Instant::now()
                .checked_sub(STEP_TIMEOUT * 2)
                .unwrap_or_else(Instant::now);
        }
    }
}

/// The progress label for a bible tracker — its on-disk bible key, from the
/// single descriptor source (#563), so this can't drift from the incremental
/// cache key in `worker.rs`. Non-bible actions never reach the series queue, but
/// fall back to the generic "bible".
fn tracker_label(action: AiAction) -> &'static str {
    action.bible_key().unwrap_or("bible")
}
