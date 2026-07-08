//! The shared async-sequencing run skeleton (#344). Two features run a queue of
//! steps one at a time, suspending on each async AI step and resuming from
//! [`poll_ai`](App::poll_ai): the author-workflow runner (`workflows.rs`) and the
//! "Refresh series bible" driver (`series_bible.rs`). They differ only in *what a
//! step is* and how their progress/summary messages read; the bookkeeping —
//! advancing after a step, the no-hang timeout, the clean cancel — is identical
//! and lives here once.
//!
//! Each feature embeds a [`SequencedRun`] in its run-state and implements
//! [`Sequenced`] on a marker type. The generic [`advance_sequenced`] /
//! [`tick_sequenced_timeout`] / [`cancel_sequenced`] drive any of them; `App`'s
//! [`advance_run`](App::advance_run) / [`tick_run_timeout`](App::tick_run_timeout)
//! / [`cancel_run`](App::cancel_run) dispatch to whichever run is active (at most
//! one is, by construction). Adding a third sequenced feature is a `Sequenced`
//! impl plus one dispatch arm — not another copy of the state machine.

use std::time::{Duration, Instant};

use super::pass_queue::PassQueueDriver;
use super::series_bible::SeriesBibleDriver;
use super::task::Task;
use super::workflows::WorkflowDriver;
use super::App;

/// How long a single async step may run before the no-hang guardrail aborts it
/// and stops the run (#227). Generous — a real AI pass can take many seconds;
/// this only catches a wedged request, not a slow one.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(120);

/// The bookkeeping every sequenced run needs while it walks its queue and
/// suspends on async steps. The feature-specific queue, names, and extra state
/// live alongside this in each run-state struct.
pub struct SequencedRun {
    /// The step currently running (or about to run); the queue length when done.
    pub(crate) index: usize,
    /// Steps that completed and sent a request (for the summary). Excludes skips.
    pub(crate) ran: usize,
    /// Steps skipped (AI off, or nothing to send) — reported, not failed.
    pub(crate) skipped: usize,
    /// Set on a clean cancel: stop after the in-flight step finishes, not mid-request.
    pub(crate) cancelled: bool,
    /// When the in-flight async step was spawned, for the per-step timeout.
    pub(crate) started_at: Instant,
}

impl SequencedRun {
    pub(crate) fn new() -> Self {
        Self {
            index: 0,
            ran: 0,
            skipped: 0,
            cancelled: false,
            started_at: Instant::now(),
        }
    }

    /// Mark the in-flight step as started now — call when spawning an async step,
    /// so the timeout measures from the spawn.
    pub(crate) fn touch(&mut self) {
        self.started_at = Instant::now();
    }

    /// Whether the in-flight step has run past the no-hang timeout.
    pub(crate) fn timed_out(&self) -> bool {
        self.started_at.elapsed() >= STEP_TIMEOUT
    }
}

/// A sequenced run's feature-specific behavior (#344). The marker type carries no
/// state — the run lives in `App` — so every method is an associated function
/// taking `&mut App`. Only step-running (`drive`) and message wording (`stop` /
/// `finish` / `cancelling_message`) differ between features.
pub trait Sequenced {
    /// This feature's run-state bookkeeping, or `None` when it isn't running.
    fn run_mut(app: &mut App) -> Option<&mut SequencedRun>;
    fn run_ref(app: &App) -> Option<&SequencedRun>;
    /// Run steps from the cursor until one suspends on an async request or the
    /// queue drains (draining reports the normal-completion summary and clears the
    /// run). Owns the feature's step semantics — sync vs async, skip vs stop.
    fn drive(app: &mut App);
    /// Tear the run down with a stop reason (a step error, a timeout, or an
    /// up-front-unavailable step), building the feature's "stopped" message.
    fn stop(app: &mut App, reason: &str);
    /// Tear the run down cleanly after a cancel, reporting what ran.
    fn finish(app: &mut App);
    /// The message shown when a cancel is requested (the in-flight step still
    /// completes first).
    fn cancelling_message(app: &App) -> String;
}

/// Resume a run after its in-flight async step finished (from `poll_ai`). A step
/// error stops it; a pending cancel stops it cleanly with the just-finished step
/// counted; otherwise it drives on to the next step. A no-op when `T` isn't running.
pub fn advance_sequenced<T: Sequenced>(app: &mut App, step_error: Option<String>) {
    let Some(run) = T::run_mut(app) else {
        return;
    };
    if let Some(e) = step_error {
        T::stop(app, &e);
        return;
    }
    run.ran += 1;
    run.index += 1;
    if run.cancelled {
        T::finish(app);
    } else {
        T::drive(app);
    }
}

/// The no-hang guardrail: if the in-flight async step has run past the timeout,
/// drop it (its orphaned worker's result is ignored) and stop the run. Called each
/// poll; a no-op unless `T` is running and its step is actually stuck.
pub fn tick_sequenced_timeout<T: Sequenced>(app: &mut App) {
    let Some(run) = T::run_ref(app) else {
        return;
    };
    if !app.ai_task.is_busy() || !run.timed_out() {
        return;
    }
    app.ai_task = Task::idle();
    T::stop(app, "timed out");
}

/// Ask a running sequenced run to stop after the in-flight step finishes — the
/// clean Esc cancel. Sets the flag; `advance_sequenced` sees it and finishes.
pub fn cancel_sequenced<T: Sequenced>(app: &mut App) {
    let Some(run) = T::run_mut(app) else {
        return;
    };
    run.cancelled = true;
    app.message = Some(T::cancelling_message(app));
}

/// "step" / "steps", "pass" / "passes" — pick the right form for a count.
pub const fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

impl App {
    /// Resume whichever sequenced run is active after an AI result lands (#344) —
    /// the single hook `poll_ai` calls. At most one run is active by construction
    /// (each refuses to start while the other, or a manual request, is in flight).
    pub(crate) fn advance_run(&mut self, step_error: Option<String>) {
        if self.workflow_run.is_some() {
            advance_sequenced::<WorkflowDriver>(self, step_error);
        } else if self.series_bible_run.is_some() {
            advance_sequenced::<SeriesBibleDriver>(self, step_error);
        } else if self.pass_queue_running() {
            advance_sequenced::<PassQueueDriver>(self, step_error);
        }
    }

    /// Tick the no-hang timeout for whichever sequenced run is active (#344).
    pub(crate) fn tick_run_timeout(&mut self) {
        if self.workflow_run.is_some() {
            tick_sequenced_timeout::<WorkflowDriver>(self);
        } else if self.series_bible_run.is_some() {
            tick_sequenced_timeout::<SeriesBibleDriver>(self);
        } else if self.pass_queue_running() {
            tick_sequenced_timeout::<PassQueueDriver>(self);
        }
    }

    /// Request a clean cancel of whichever sequenced run is active (#344) — bound
    /// to Esc; the in-flight step still finishes.
    pub(crate) fn cancel_run(&mut self) {
        if self.workflow_run.is_some() {
            cancel_sequenced::<WorkflowDriver>(self);
        } else if self.series_bible_run.is_some() {
            cancel_sequenced::<SeriesBibleDriver>(self);
        } else if self.pass_queue_running() {
            cancel_sequenced::<PassQueueDriver>(self);
        }
    }

    /// Whether any sequenced run is in progress (so Esc routes to its cancel).
    pub(crate) const fn sequenced_run_active(&self) -> bool {
        self.workflow_run.is_some() || self.series_bible_run.is_some()
    }
}
