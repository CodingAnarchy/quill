//! The durable multi-pass queue (#923): queue several analysis passes and run
//! them back-to-back with a single aggregate cost confirmation, per-pass progress
//! ("continuity: chapter 5/12"), and crash-safe resume. Where the series-bible
//! refresh (#340/#655) is an in-memory run lost on a crash, this run persists its
//! queue and cursor to `bible/.queue.toml` after every pass lands, so an
//! interrupted or crashed queue resumes from where it stopped rather than losing
//! the passes already done.
//!
//! The async sequencing — advance after a step, the no-hang timeout, the clean
//! Esc cancel — is the shared [`Sequenced`](super::sequenced) skeleton (#344);
//! this module supplies the durable cursor, the per-pass step, the aggregate cost
//! gate, and the resume-at-launch offer. The passes queued are the five bible
//! trackers over every book (the "refresh whole bible suite" the ticket asks for),
//! run in reading order within each tracker so each book builds on the last.

use super::sequenced::{plural, Sequenced, SequencedRun};
use super::*;

use crate::ai::cost::{self, Estimate};
use crate::ai::incremental::{PassQueue, QueuedPass};
use crate::mode::AiAction;

/// The bible trackers a queue run refreshes, each accumulated across the books in
/// reading order — the same suite the all-bibles refresh runs (#655), but durable.
const TRACKERS: [AiAction; 5] = [
    AiAction::Characters,
    AiAction::Continuity,
    AiAction::Setting,
    AiAction::Plot,
    AiAction::Theme,
];

/// A durable multi-pass queue run in progress (#923): the persisted queue (its
/// passes and completed cursor), the shared sequenced-run bookkeeping (#344), and
/// the triage length when the run started so the summary can report new findings.
/// Lives in `App::pass_queue_run` for the run's lifetime.
pub struct PassQueueRun {
    /// The durable queue mirrored to `bible/.queue.toml`; its `done` cursor is kept
    /// in lockstep with `seq.index` and persisted after each pass completes.
    queue: PassQueue,
    /// Cursor, tallies, cancel flag, and the in-flight pass's start time (#344).
    seq: SequencedRun,
    /// Triage length at the start, so the summary can report how many findings the
    /// run added.
    triage_before: usize,
}

impl PassQueueRun {
    fn new(queue: PassQueue, triage_before: usize) -> Self {
        Self {
            queue,
            seq: SequencedRun::new(),
            triage_before,
        }
    }

    /// Resume a persisted queue: its cursor becomes the sequenced run's starting
    /// index so already-completed passes aren't re-run (#923).
    fn resume(queue: PassQueue, triage_before: usize) -> Self {
        let mut run = Self::new(queue, triage_before);
        run.seq.index = run.queue.done;
        run
    }
}

/// The pass-queue run's [`Sequenced`] behavior (#344) — the marker the generic
/// advance/timeout/cancel dispatch through.
pub struct PassQueueDriver;

impl Sequenced for PassQueueDriver {
    fn run_mut(app: &mut App) -> Option<&mut SequencedRun> {
        app.pass_queue_run.as_mut().map(|r| &mut r.seq)
    }
    fn run_ref(app: &App) -> Option<&SequencedRun> {
        app.pass_queue_run.as_ref().map(|r| &r.seq)
    }
    fn drive(app: &mut App) {
        app.run_pass_queue_step();
    }
    fn stop(app: &mut App, reason: &str) {
        if let Some(run) = app.pass_queue_run.take() {
            let at = run.seq.index + 1;
            // The queue file is left on disk (cursor at the last completed pass), so
            // the author can resume the remaining passes on the next launch — the
            // crash-safety the whole feature is for.
            app.set_pass_progress(None);
            app.message = Some(format!(
                "Pass queue stopped at pass {at}: {reason}. Reopen to resume the rest."
            ));
        }
    }
    fn finish(app: &mut App) {
        app.finish_pass_queue();
    }
    fn cancelling_message(_app: &App) -> String {
        "Pass queue: cancelling after this pass…".to_string()
    }
}

impl App {
    /// Start a durable multi-pass queue (#923): enqueue the five bible trackers over
    /// every book, confirm the one aggregate cost, persist the queue, and run the
    /// first pass. Gates on a ready backend and no other run in flight; a run that's
    /// entirely up to date reports so and spends nothing. `force` rebuilds each
    /// bible from the whole manuscript (#176).
    pub(crate) fn start_queue_passes(&mut self, force: bool) {
        if self.a_run_is_active() {
            self.message = Some("A pass is already running.".into());
            return;
        }
        if let Err(e) = self.ai.check_ready() {
            self.message = Some(e.to_string());
            return;
        }
        let planned = self.pass_queue_plan();
        // One aggregate estimate the author confirms once, mirroring the all-bibles
        // batch (#655): a whole run under the threshold starts straight away.
        let est = self.pass_queue_estimate(&planned, force);
        if est.tokens == 0 {
            self.message = Some("Story bibles are current — no chapters changed.".into());
            return;
        }
        if est.dollars > self.ai.cost_threshold {
            let label = format!(
                "Queue {} {} — {} — run? (y/N)",
                planned.len(),
                plural(planned.len(), "pass", "passes"),
                cost::label(&est, &self.ai.model)
            );
            self.open_prompt(Pending::ConfirmQueuePasses { force }, label, "", None);
        } else {
            self.queue_passes_confirmed(force);
        }
    }

    /// Begin the confirmed queue run (#923): build the durable queue, persist it,
    /// and drive its first pass. Reached from the aggregate cost confirm, or
    /// directly when the estimate was under the threshold.
    pub(crate) fn queue_passes_confirmed(&mut self, force: bool) {
        if self.a_run_is_active() {
            self.message = Some("A pass is already running.".into());
            return;
        }
        let passes = self.pass_queue_plan();
        let queue = PassQueue::new(passes, force);
        // Persist up front so even a crash before the first pass lands leaves a
        // resumable queue on disk.
        if let Err(e) = queue.save(&self.project.root) {
            self.message = Some(format!("Couldn't write the pass queue: {e}"));
            return;
        }
        self.pass_queue_run = Some(PassQueueRun::new(queue, self.triage.len()));
        self.run_pass_queue_step();
    }

    /// Whether any sequenced run (a workflow, the series refresh, or a pass queue)
    /// is already in flight — the shared "only one run at a time" gate.
    const fn a_run_is_active(&self) -> bool {
        self.sequenced_run_active() || self.pass_queue_run.is_some()
    }

    /// The `(action, book)` passes a queue run covers — each of the five bible
    /// trackers over every book, in reading order within a tracker so each book
    /// builds on the last. For a standalone novel it's the five trackers over its
    /// one book. Stored as slugs in the durable queue.
    fn pass_queue_plan(&self) -> Vec<QueuedPass> {
        let books = self.project.book_count();
        TRACKERS
            .iter()
            .flat_map(|&t| (0..books).map(move |b| (t, b)))
            .map(|(t, book)| QueuedPass {
                slug: t.slug().to_string(),
                book,
            })
            .collect()
    }

    /// The combined cost estimate for a planned queue (#923): the summed input size
    /// of every pass, each scoped exactly as it will run (incremental delta, full
    /// rebuild, or up-to-date/zero). Every pass runs synchronously at the standard
    /// rate (the Batch API's half price is reserved for a future opt-in background
    /// update, #950), so the figure applies no discount. Mirrors the all-bibles
    /// estimate (#655).
    fn pass_queue_estimate(&self, planned: &[QueuedPass], force: bool) -> Estimate {
        let resolved: Vec<(AiAction, usize)> = planned
            .iter()
            .filter_map(|p| AiAction::from_slug(&p.slug).map(|a| (a, p.book)))
            .collect();
        let chars: usize = resolved
            .iter()
            .map(|&(action, book)| self.book_pass_context_len(action, book, force))
            .sum();
        cost::estimate(chars, ai::MAX_OUTPUT_TOKENS, &self.ai.model, false)
    }

    /// Run queued passes until one spawns an async request (then suspend until
    /// `poll_ai` resumes us) or the queue drains. Before running the pass at the
    /// cursor, the durable cursor is synced to the completed count and persisted,
    /// so a crash after a pass lands resumes from the next one. An up-to-date book
    /// has nothing to send, so it's counted skipped and the run moves straight on.
    fn run_pass_queue_step(&mut self) {
        loop {
            // Sync the durable cursor to the passes completed so far and persist —
            // this is the crash-safety point (#923): after pass N lands, `seq.index`
            // is N+1, so `done` catches up to it and the file records the progress.
            self.persist_pass_queue_cursor();

            let (action, book, force) = match self.pass_queue_run.as_mut() {
                Some(run) if run.seq.index < run.queue.passes.len() => {
                    run.seq.touch();
                    let pass = &run.queue.passes[run.seq.index];
                    if let Some(action) = AiAction::from_slug(&pass.slug) {
                        (action, pass.book, run.queue.force)
                    } else {
                        // An unknown slug (a queue from a newer build): skip it
                        // rather than wedge the run.
                        run.seq.skipped += 1;
                        run.seq.index += 1;
                        continue;
                    }
                }
                Some(_) => {
                    self.finish_pass_queue(); // queue drained
                    return;
                }
                None => return,
            };
            self.set_pass_progress(None); // a fresh pass starts with no chapter count
            self.run_book_bible_pass(action, book, force);
            if self.ai_task.is_busy() {
                self.show_pass_queue_progress(action, book);
                return;
            }
            // Not busy → that book was already current (nothing to send): skip on.
            if let Some(run) = self.pass_queue_run.as_mut() {
                run.seq.skipped += 1;
                run.seq.index += 1;
            }
        }
    }

    /// Advance the durable `done` cursor to match how many passes have completed
    /// (`seq.index`) and persist the queue file (#923). Best effort: a write failure
    /// doesn't stop the run (the pass already ran), it just weakens resume.
    fn persist_pass_queue_cursor(&mut self) {
        let root = self.project.root.clone();
        if let Some(run) = self.pass_queue_run.as_mut() {
            while run.queue.done < run.seq.index {
                let _ = run.queue.complete_current(&root);
            }
        }
    }

    /// Show per-pass progress for the in-flight queue pass (#923): the pass name,
    /// the queue position, and — while a fold is walking the book — the chapter
    /// count ("continuity: chapter 5/12"). The chapter count comes from the shared
    /// progress cell the fold worker updates; a one-shot pass shows pass-level
    /// progress only.
    fn show_pass_queue_progress(&mut self, action: AiAction, book: usize) {
        let Some(run) = self.pass_queue_run.as_ref() else {
            return;
        };
        let (done, total) = (run.seq.index + 1, run.queue.passes.len());
        let name = tracker_label(action);
        let per_pass = self
            .pass_progress()
            .map_or_else(String::new, |(c, t)| format!(" — chapter {c}/{t}"));
        let book_tag = if self.project.book_count() > 1 {
            self.project
                .book_title(book)
                .map(|t| format!(" · {t}"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        self.message = Some(format!(
            "Pass queue — {done}/{total}: {name}{book_tag}{per_pass}… (Esc to cancel)"
        ));
    }

    /// Whether a pass-queue run is active (Esc routes to its cancel).
    pub(crate) const fn pass_queue_running(&self) -> bool {
        self.pass_queue_run.is_some()
    }

    /// The book whose queue pass is in flight (#923), so its findings are attributed
    /// to that book — like the series refresh's `current_series_bible_book` (#341).
    fn current_pass_queue_book(&self) -> Option<usize> {
        let run = self.pass_queue_run.as_ref()?;
        run.queue.passes.get(run.seq.index).map(|pass| pass.book)
    }

    /// The book a sequenced bible pass's findings should be attributed to (#341/
    /// #923): the series-bible refresh's current book, or the pass queue's current
    /// book, whichever run is applying a pass. `None` outside a run — an active-book
    /// pass targets the active book, as before.
    pub(crate) fn current_pass_book(&self) -> Option<usize> {
        self.current_series_bible_book()
            .or_else(|| self.current_pass_queue_book())
    }

    /// Clear the run and report what it did — the [`PassQueueDriver`]'s
    /// [`Sequenced::finish`] hook and the normal drain-complete path (#344). The
    /// durable cursor is synced first, which clears the queue file once the last
    /// pass completes, so a finished run leaves nothing to resume.
    fn finish_pass_queue(&mut self) {
        self.persist_pass_queue_cursor();
        self.set_pass_progress(None);
        if let Some(run) = self.pass_queue_run.take() {
            // A clean finish clears the queue file. A cancel that leaves passes
            // unrun keeps them on disk so the author can resume; a cancel on the
            // very last pass has nothing left, so the file is already gone.
            if run.seq.cancelled && !run.queue.is_finished() {
                let _ = run.queue.save(&self.project.root);
            } else {
                PassQueue::clear(&self.project.root);
            }
            let (ran, skipped, cancelled) = (run.seq.ran, run.seq.skipped, run.seq.cancelled);
            let tail = if skipped > 0 {
                format!(" ({skipped} already current)")
            } else {
                String::new()
            };
            let found = self.triage.len().saturating_sub(run.triage_before);
            let findings = if found > 0 {
                format!("; {found} in revision triage")
            } else {
                String::new()
            };
            let verb = if cancelled {
                "cancelled —"
            } else {
                "finished —"
            };
            self.message = Some(format!(
                "Pass queue {verb} {ran} {}{tail}{findings}",
                plural(ran, "pass", "passes")
            ));
        }
    }

    /// At launch, offer to resume an interrupted pass queue (#923): if the project
    /// has a `bible/.queue.toml` with passes still to run, open a y/N resume confirm
    /// so a crashed queue picks up rather than losing the completed passes. A no-op
    /// when there's no queue, it's already finished, or a prompt is already up (a
    /// crash recovery takes precedence).
    pub fn offer_pass_queue_resume(&mut self) {
        // Don't pre-empt an already-open prompt/overlay (a crash-recovery offer
        // takes precedence) — the same guard the backup-restore offer uses.
        if !matches!(self.mode, Mode::Insert | Mode::Normal) || self.overlay.is_some() {
            return;
        }
        let Some(queue) = PassQueue::load(&self.project.root) else {
            return;
        };
        let remaining = queue.remaining().len();
        if remaining == 0 {
            PassQueue::clear(&self.project.root);
            return;
        }
        self.open_prompt(
            Pending::ResumePassQueue,
            format!(
                "A pass queue was interrupted — {remaining} {} left. Resume (y) or discard (Esc)?",
                plural(remaining, "pass", "passes")
            ),
            "",
            None,
        );
    }

    /// Resume the persisted pass queue after the author confirms (#923): rehydrate
    /// the run from the queue file's cursor and drive the next pass. A no-op if the
    /// backend isn't ready or the queue vanished.
    pub(crate) fn resume_pass_queue(&mut self) {
        if self.a_run_is_active() {
            return;
        }
        if let Err(e) = self.ai.check_ready() {
            self.message = Some(e.to_string());
            return;
        }
        let Some(queue) = PassQueue::load(&self.project.root) else {
            self.message = Some("No pass queue to resume.".into());
            return;
        };
        if queue.is_finished() {
            PassQueue::clear(&self.project.root);
            return;
        }
        self.pass_queue_run = Some(PassQueueRun::resume(queue, self.triage.len()));
        self.run_pass_queue_step();
    }

    /// Discard the persisted queue instead of resuming it (#923) — the Esc answer to
    /// the launch resume prompt.
    pub(crate) fn discard_pass_queue(&mut self) {
        PassQueue::clear(&self.project.root);
        self.message = Some("Pass queue discarded.".into());
    }
}

#[cfg(test)]
impl App {
    /// Seed a pass-queue run (persisted) so the state machine (advance / cancel /
    /// error / resume) can be exercised without a live AI backend.
    pub(crate) fn seed_pass_queue_run(&mut self, passes: Vec<(AiAction, usize)>) {
        let queue = PassQueue::new(
            passes
                .into_iter()
                .map(|(a, book)| QueuedPass {
                    slug: a.slug().to_string(),
                    book,
                })
                .collect(),
            false,
        );
        let _ = queue.save(&self.project.root);
        self.pass_queue_run = Some(PassQueueRun::new(queue, self.triage.len()));
    }

    /// Whether a pass-queue run is recorded (test inspection).
    pub(crate) fn has_pass_queue_run(&self) -> bool {
        self.pass_queue_run.is_some()
    }

    /// The durable queue's completed-cursor (test inspection): how many passes the
    /// on-disk queue records as done.
    pub(crate) fn pass_queue_done(&self) -> Option<usize> {
        PassQueue::load(&self.project.root).map(|q| q.done)
    }

    /// The `(action, book)` pass the run currently sits on — the one in flight when
    /// parked, or the next to run (test inspection). `None` when no run is active or
    /// the queue has drained past its end. Lets a resume test assert the run picked
    /// up from the cursor (the second pass) rather than restarting at the top,
    /// independent of whether a backend is actually compiled in.
    pub(crate) fn pass_queue_current_for_test(&self) -> Option<(AiAction, usize)> {
        let run = self.pass_queue_run.as_ref()?;
        let pass = run.queue.passes.get(run.seq.index)?;
        AiAction::from_slug(&pass.slug).map(|a| (a, pass.book))
    }

    /// The planned `(action, book)` queue for the current project — for tests to
    /// assert the queue command plans the five trackers over every book.
    pub(crate) fn pass_queue_plan_for_test(&self) -> Vec<(AiAction, usize)> {
        self.pass_queue_plan()
            .iter()
            .filter_map(|p| AiAction::from_slug(&p.slug).map(|a| (a, p.book)))
            .collect()
    }

    /// The aggregate estimate the queue command would confirm — for tests to assert
    /// it sums the planned passes.
    pub(crate) fn pass_queue_estimate_for_test(&self, force: bool) -> Estimate {
        self.pass_queue_estimate(&self.pass_queue_plan(), force)
    }

    /// Publish per-pass chapter progress directly (test seam): stand in for the fold
    /// worker so the run's status line can be asserted without a backend.
    pub(crate) fn set_pass_progress_for_test(&self, progress: Option<(usize, usize)>) {
        self.set_pass_progress(progress);
    }

    /// Render the in-flight pass's progress line (test seam) so the per-pass
    /// "chapter n/total" status can be asserted without a live pass.
    pub(crate) fn show_pass_queue_progress_for_test(&mut self, action: AiAction, book: usize) {
        self.show_pass_queue_progress(action, book);
    }

    /// Backdate the in-flight pass so the per-step timeout has elapsed (test seam).
    pub(crate) fn expire_pass_queue_step(&mut self) {
        use super::sequenced::STEP_TIMEOUT;
        use std::time::Instant;
        if let Some(run) = self.pass_queue_run.as_mut() {
            run.seq.started_at = Instant::now()
                .checked_sub(STEP_TIMEOUT * 2)
                .unwrap_or_else(Instant::now);
        }
    }
}

/// The progress label for a bible tracker — its on-disk bible key (#563), so this
/// can't drift from the incremental cache key. Non-bible actions never reach the
/// queue, but fall back to the generic "bible".
fn tracker_label(action: AiAction) -> &'static str {
    action.bible_key().unwrap_or("bible")
}
