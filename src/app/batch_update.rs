//! The opt-in background bible update (#950): submit full-manuscript bible
//! rebuilds through the asynchronous Message Batches API (half price, a window of
//! up to ~24h), persist the batch ids, and reconcile the results on a later launch
//! or idle tick — surviving app restarts.
//!
//! This is the *explicit-action* trigger model. Quill has no daemon, so "run it in
//! the background" is: an opt-in setting (`[ai] background-batch-update`) plus an
//! explicit "Queue a background bible update" run-action. The action **submits** the
//! rebuilds and returns immediately — it never waits on the batch window (the bug
//! #955 fixed for the interactive path). Reconciliation is out-of-band: each idle
//! tick and each launch, [`reconcile_batch_update`](App::reconcile_batch_update)
//! polls the persisted batches once; a finished one is applied through the same
//! per-feature apply path an interactive pass uses, a failed one is surfaced (never
//! swallowed), and a **stale** one — the manuscript was edited while the batch was
//! in flight — is warned about and dropped rather than blindly overwriting a bible
//! the author has since moved past.
//!
//! Both the submit and the reconcile run on [`batch_task`](App::batch_task), a
//! worker separate from the interactive [`ai_task`](App::ai_task), so a background
//! batch operation neither blocks nor is blocked by an interactive pass. The
//! durable record ([`ai::batch::BatchUpdate`]) is `bible/.batch.toml`; the pattern
//! mirrors the durable pass queue (#923).

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::sequenced::plural;
use super::*;

use crate::ai::batch::{BatchUpdate, PendingBatch};
use crate::ai::cost;
use crate::ai::incremental::Digests;
use crate::ai::{BatchStatus, Request};
use crate::mode::AiAction;

/// The bible trackers a background update rebuilds — the same suite the all-bibles
/// refresh (#655) and the pass queue (#923) cover, but through the batch endpoint.
const TRACKERS: [AiAction; 5] = [
    AiAction::Characters,
    AiAction::Continuity,
    AiAction::Setting,
    AiAction::Plot,
    AiAction::Theme,
];

/// How long the idle reconcile trusts its in-memory copy of the durable record
/// before re-reading it. Every write this process makes drops the copy at once;
/// this only bounds how long a change made some other way (another instance, a
/// hand edit) goes unnoticed. Batches take minutes to hours, so it costs nothing.
const RECORD_RELOAD: Duration = Duration::from_secs(30);

/// The durable batch record as last read from `root`, and when.
pub(super) struct BatchRecordCache {
    root: PathBuf,
    record: BatchUpdate,
    loaded: Instant,
}

/// A single background-batch job's spec, assembled on the UI thread and handed to
/// the worker: which tracker pass over which book, the request to run, and the
/// manuscript digests it's built against (the staleness baseline).
struct BatchJob {
    slug: String,
    book: usize,
    digests: Digests,
    req: Request,
}

/// What one background-batch worker step produced, delivered back to the UI thread.
/// A submit step yields [`Submitted`](BatchTaskResult::Submitted); a reconcile step
/// yields [`Reconciled`](BatchTaskResult::Reconciled).
pub enum BatchTaskResult {
    /// A submitted batch: its record (with the returned id) to persist, or the error
    /// if the submit itself failed (nothing was queued for this pass).
    Submitted(Result<PendingBatch, (String, usize, String)>),
    /// A polled batch: its id + the tracker/book/digests it carries, and where it
    /// stands. `Err` is a transient status-fetch fault — the batch stays queued to
    /// retry next tick, never lost.
    Reconciled {
        batch_id: String,
        slug: String,
        book: usize,
        digests: Digests,
        status: Result<BatchStatus, ai::AiError>,
    },
}

impl App {
    /// Whether the background batch update is usable, or why not (#950): the opt-in,
    /// a ready cloud backend (the batch endpoint is a remote-API path — never local
    /// or offline), and this build's ability to reach it. The run-action gates on
    /// this; the reconcile just checks the opt-in (a batch already submitted is worth
    /// finishing even if the backend later changed).
    pub(crate) fn batch_update_status(&self) -> Result<(), String> {
        if !self.ai.background_batch_update {
            return Err(
                "the background bible update is off — set `[ai] background-batch-update = true` \
                 to enable it"
                    .into(),
            );
        }
        if self.ai.offline {
            return Err(
                "the background update uses the Claude batch API — turn off `offline`".into(),
            );
        }
        if self.ai.backend() != ai::BackendKind::Claude {
            return Err(
                "the background update uses the Claude batch API — set `[ai] backend = \"claude\"`"
                    .into(),
            );
        }
        self.ai
            .ready_for(ai::BackendKind::Claude)
            .map_err(|e| e.to_string())
    }

    /// Queue a background bible update (#950): confirm the (batch-discounted) cost,
    /// then submit a full-manuscript rebuild per bible tracker over every book to the
    /// Message Batches API. Gates on the opt-in + a ready cloud backend, skips passes
    /// already in flight from an earlier queueing and empty books, and reports if
    /// there's nothing to do. Under the cost threshold it submits straight away.
    pub(crate) fn start_batch_update(&mut self) {
        if let Err(message) = self.batch_update_status() {
            self.message = Some(message);
            return;
        }
        if self.batch_task.is_busy() {
            self.message = Some("A background bible update is already being processed…".into());
            return;
        }
        // Don't re-submit a tracker/book still pending from an earlier queueing.
        let already = BatchUpdate::load(&self.project.root).pending_keys();
        let jobs = self.plan_batch_jobs(&already);
        if jobs.is_empty() {
            self.message = Some(if already.is_empty() {
                "Nothing to update — the manuscript is empty.".into()
            } else {
                "A background bible update is already in flight for every book.".into()
            });
            return;
        }
        // The confirm/estimate applies the Batch API's half-price discount (#691/
        // #950) — the batch-decision the removed `book_pass_batches` eligibility once
        // supplied — so the quoted cost matches what the batch actually bills.
        let est = self.batch_update_estimate(&jobs);
        if est.dollars > self.ai.cost_threshold {
            let label = format!(
                "Queue {} background bible {} — {} (batch, ~24h) — run? (y/N)",
                jobs.len(),
                plural(jobs.len(), "update", "updates"),
                cost::label(&est, &self.ai.model)
            );
            self.open_prompt(Pending::ConfirmBatchUpdate, label, "", None);
        } else {
            self.submit_batch_update();
        }
    }

    /// The aggregate cost estimate for a background update (#950), with the Batch
    /// API's half-price discount applied (#691): the summed full-book input of every
    /// planned pass, each output bounded by `MAX_OUTPUT_TOKENS`. Recomputed from the
    /// same plan the submit uses so the quote can't drift from what's sent.
    fn batch_update_estimate(&self, jobs: &[BatchJob]) -> cost::Estimate {
        // Each job's request already carries its full input as `cached_context` (the
        // whole book, or prior-bible + book for a series revise), so summing that is
        // exactly what the batch sends.
        let chars: usize = jobs.iter().map(|job| job.req.cached_context.len()).sum();
        cost::estimate(chars, ai::MAX_OUTPUT_TOKENS, &self.ai.model, true)
    }

    /// Submit the background update after the cost is confirmed (or was under the
    /// threshold): spawn the batch submits and return immediately (#950). Re-plans
    /// from the live manuscript so a manuscript edited during the confirm is caught.
    pub(crate) fn submit_batch_update(&mut self) {
        if self.batch_task.is_busy() {
            self.message = Some("A background bible update is already being processed…".into());
            return;
        }
        let already = BatchUpdate::load(&self.project.root).pending_keys();
        let jobs = self.plan_batch_jobs(&already);
        if jobs.is_empty() {
            self.message = Some("Nothing to update.".into());
            return;
        }
        let Some(key) = self.ai.resolved_key() else {
            self.message = Some("No API key for the Claude batch API.".into());
            return;
        };
        let submitted = date::iso(date::today());
        let count = jobs.len();
        self.batch_task.spawn(move || {
            let backend = ClaudeBackend::new(key);
            jobs.into_iter()
                .map(|job| {
                    let result = backend.submit_batch(&job.req).map(|batch_id| PendingBatch {
                        batch_id,
                        slug: job.slug.clone(),
                        book: job.book,
                        digests: job.digests,
                        submitted: submitted.clone(),
                    });
                    BatchTaskResult::Submitted(
                        result.map_err(|e| (job.slug, job.book, e.to_string())),
                    )
                })
                .collect()
        });
        self.message = Some(format!(
            "Queuing {count} background bible {} — they'll apply when ready (up to ~24h).",
            plural(count, "update", "updates")
        ));
    }

    /// The batch jobs to submit for a background update (#950): each of the five
    /// bible trackers over every book, skipping the `already`-pending `(slug, book)`
    /// pairs and books with nothing to rebuild. Each carries a full-rebuild request
    /// and the manuscript digests it's built against.
    fn plan_batch_jobs(
        &self,
        already: &std::collections::BTreeSet<(String, usize)>,
    ) -> Vec<BatchJob> {
        let books = self.project.book_count();
        TRACKERS
            .iter()
            .flat_map(|&t| (0..books).map(move |b| (t, b)))
            .filter(|(t, book)| !already.contains(&(t.slug().to_string(), *book)))
            .filter_map(|(action, book)| {
                // The batch always runs a full rebuild; `series` is recomputed at
                // reconcile time (the manifest may change while a batch is in flight).
                let (req, digests, _series) = self.batch_full_request(action, book)?;
                Some(BatchJob {
                    slug: action.slug().to_string(),
                    book,
                    digests,
                    req,
                })
            })
            .collect()
    }

    /// Kick off an out-of-band reconcile of the in-flight batches (#950), if any:
    /// hand the persisted batches to the worker to poll each once. A no-op when the
    /// opt-in is off, there's nothing in flight, or a batch worker is already busy —
    /// so it's cheap to call every idle tick and at launch. Requires a resolvable
    /// Claude key (a batch is a Claude artifact); without one the batches simply wait.
    pub(crate) fn reconcile_batch_update(&mut self) {
        if !self.ai.background_batch_update || self.batch_task.is_busy() {
            return;
        }
        if self.cached_batch_record().is_empty() {
            return;
        }
        let Some(key) = self.ai.resolved_key() else {
            return; // no key to reach the batch API — leave them queued
        };
        let batches = self.cached_batch_record().batches.clone();
        self.batch_task.spawn(move || {
            let backend = ClaudeBackend::new(key);
            batches
                .into_iter()
                .map(|b| BatchTaskResult::Reconciled {
                    status: backend.reconcile_batch(&b.batch_id),
                    batch_id: b.batch_id,
                    slug: b.slug,
                    book: b.book,
                    digests: b.digests,
                })
                .collect()
        });
    }

    /// The durable batch record for the idle reconcile, read from disk only when
    /// there's no fresh in-memory copy for this project (none yet, this process
    /// wrote it since, or it's older than [`RECORD_RELOAD`]) — so an idle pass with
    /// nothing in flight is a few compares, not a file read and a TOML parse.
    fn cached_batch_record(&mut self) -> &BatchUpdate {
        let root = &self.project.root;
        let fresh = self
            .batch_record
            .as_ref()
            .is_some_and(|c| &c.root == root && c.loaded.elapsed() < RECORD_RELOAD);
        if !fresh {
            self.batch_record = None;
        }
        let cache = self.batch_record.get_or_insert_with(|| BatchRecordCache {
            root: root.clone(),
            record: BatchUpdate::load(root),
            loaded: Instant::now(),
        });
        &cache.record
    }

    /// Poll the background-batch worker (#950), non-blocking, once per loop tick.
    /// On a submit result, persist each returned batch id; on a reconcile result,
    /// apply the finished ones, surface the failed ones, and drop the stale ones.
    pub fn poll_batch_update(&mut self) {
        match self.batch_task.poll() {
            Poll::Ready(results) => self.apply_batch_results(results),
            // A dead worker (panic / torn down) leaves the persisted record intact,
            // so the next launch or tick simply retries — nothing is lost.
            Poll::Pending | Poll::Died(_) => {}
        }
    }

    /// Handle a batch of worker results (#950): persist submitted ids, apply / report
    /// / drop reconciled ones, and surface a summary. Split from the poll so tests can
    /// inject results without a live batch backend (network-free, #531).
    fn apply_batch_results(&mut self, results: Vec<BatchTaskResult>) {
        let mut submitted = 0usize;
        let mut submit_failures = 0usize;
        let mut applied = 0usize;
        let mut failed: Vec<String> = Vec::new();
        let mut stale = 0usize;
        let root = self.project.root.clone();
        // Every arm below may rewrite the durable record; the idle reconcile must
        // re-read it rather than resubmit batches this pass just applied or dropped.
        self.batch_record = None;
        for result in results {
            match result {
                BatchTaskResult::Submitted(Ok(batch)) => {
                    let mut record = BatchUpdate::load(&root);
                    if record.push(&root, batch).is_ok() {
                        submitted += 1;
                    }
                }
                BatchTaskResult::Submitted(Err(_)) => submit_failures += 1,
                BatchTaskResult::Reconciled {
                    batch_id,
                    slug,
                    book,
                    digests,
                    status,
                } => match status {
                    // Still going, or a transient status-fetch fault: leave the batch
                    // queued to check again next tick — never a false "timed out".
                    Ok(BatchStatus::Processing) | Err(_) => {}
                    Ok(BatchStatus::Failed(reason)) => {
                        failed.push(reason);
                        Self::forget_batch(&root, &batch_id);
                    }
                    Ok(BatchStatus::Ready(resp)) => {
                        match self.apply_reconciled_batch(&slug, book, &digests, resp) {
                            Applied::Wrote => applied += 1,
                            Applied::Stale => stale += 1,
                            Applied::Skipped => {}
                        }
                        Self::forget_batch(&root, &batch_id);
                    }
                },
            }
        }
        self.report_batch_reconcile(submitted, submit_failures, applied, &failed, stale);
    }

    /// Apply a finished batch's rebuild (#950), unless the manuscript has drifted
    /// since it was submitted. A **stale** pass (any tracked chapter changed / added
    /// / removed) is not applied — it would overwrite a bible the author has moved
    /// past — so it's reported and dropped; the author re-queues to pick up the
    /// edits. A fresh pass is applied through the same per-feature path an
    /// interactive pass uses, recording the incremental state so the next pass only
    /// re-analyzes what changes after this.
    fn apply_reconciled_batch(
        &mut self,
        slug: &str,
        book: usize,
        built_from: &Digests,
        resp: ai::Response,
    ) -> Applied {
        let Some(action) = AiAction::from_slug(slug) else {
            return Applied::Skipped; // an unknown slug (a record from a newer build)
        };
        let Some(finish) = Self::bible_finish(action) else {
            return Applied::Skipped;
        };
        // Stale-pass detection via the existing digest (#90/#950): compare what the
        // batch was built against with the book now.
        if crate::ai::batch::is_stale(built_from, &self.book_digests(book)) {
            return Applied::Stale;
        }
        // Set up the same recording context an interactive pass would: the pending
        // state to record on a clean apply, and the book its findings attribute to.
        let merge = (self.project.book_count() > 1).then(|| self.other_books_files(book));
        self.pending_pass = Some((tracker_key(action), built_from.clone(), merge));
        self.batch_apply_book.set(Some(book));
        self.apply_outcome(finish(Ok(resp)));
        self.batch_apply_book.set(None);
        // A clean bible apply records its own state; drop any leftover snapshot.
        self.pending_pass = None;
        Applied::Wrote
    }

    /// Drop a reconciled batch from the durable record (best effort).
    fn forget_batch(root: &std::path::Path, batch_id: &str) {
        let mut record = BatchUpdate::load(root);
        let _ = record.remove(root, batch_id);
    }

    /// Surface a non-intrusive summary of a reconcile pass (#950), without
    /// interrupting writing: what applied, what failed, and what was stale. Silent
    /// when nothing terminal happened (all still processing), so idle polling never
    /// nags. A submit summary (from the queue action's worker) reports separately.
    fn report_batch_reconcile(
        &mut self,
        submitted: usize,
        submit_failures: usize,
        applied: usize,
        failed: &[String],
        stale: usize,
    ) {
        // The submit worker's result: report what got queued.
        if submitted > 0 || submit_failures > 0 {
            let mut msg = format!(
                "Background bible update: queued {submitted} {}",
                plural(submitted, "pass", "passes")
            );
            if submit_failures > 0 {
                let _ = write!(msg, " ({submit_failures} couldn't be submitted)");
            }
            self.message = Some(msg);
            return;
        }
        // The reconcile worker's result: report only when something terminal landed.
        let mut parts: Vec<String> = Vec::new();
        if applied > 0 {
            parts.push(format!(
                "applied {applied} bible {}",
                plural(applied, "update", "updates")
            ));
        }
        if stale > 0 {
            parts.push(format!(
                "{stale} skipped as stale (manuscript changed — re-queue to refresh)"
            ));
        }
        if !failed.is_empty() {
            parts.push(format!("{} failed ({})", failed.len(), failed.join(", ")));
        }
        if !parts.is_empty() {
            self.message = Some(format!(
                "Background bible update ready — {}.",
                parts.join("; ")
            ));
        }
    }
}

/// The tracker's on-disk bible key for recording pass state (#563) — the same key
/// `record_pass_state` expects. A bible tracker always has one; fall back defensively.
fn tracker_key(action: AiAction) -> String {
    action.bible_key().unwrap_or("bible").to_string()
}

#[cfg(test)]
impl App {
    /// Drive the reconcile handler with injected worker results (test seam, #950):
    /// stands in for the batch backend so the persist / apply / stale / fail paths
    /// are exercised network-free (#531).
    pub(crate) fn apply_batch_results_for_test(&mut self, results: Vec<BatchTaskResult>) {
        self.apply_batch_results(results);
    }

    /// The in-flight batch ids the idle reconcile would poll right now (test seam),
    /// read through its in-memory copy of the record — so a test can assert that a
    /// write this process made is seen, network-free.
    pub(crate) fn reconcile_batch_ids_for_test(&mut self) -> Vec<String> {
        self.cached_batch_record()
            .batches
            .iter()
            .map(|b| b.batch_id.clone())
            .collect()
    }

    /// The batch-discounted aggregate estimate the queue action would confirm (test
    /// seam), so a test can assert the half-price discount is applied (#691/#950).
    pub(crate) fn batch_update_estimate_for_test(&self) -> cost::Estimate {
        let already = BatchUpdate::load(&self.project.root).pending_keys();
        self.batch_update_estimate(&self.plan_batch_jobs(&already))
    }

    /// The `(slug, book)` passes a background update would submit (test seam), so a
    /// test can assert it plans the five trackers over every book and skips ones
    /// already in flight.
    pub(crate) fn batch_update_plan_for_test(&self) -> Vec<(String, usize)> {
        let already = BatchUpdate::load(&self.project.root).pending_keys();
        self.plan_batch_jobs(&already)
            .into_iter()
            .map(|j| (j.slug, j.book))
            .collect()
    }
}

/// Test constructors for injecting worker results without a live batch backend.
#[cfg(test)]
impl BatchTaskResult {
    /// A finished, succeeded batch carrying `text` as its rebuild response, built
    /// against `digests` (the staleness baseline).
    pub(crate) fn ready(slug: &str, book: usize, digests: Digests, text: &str) -> Self {
        Self::Reconciled {
            batch_id: format!("batch-{slug}-{book}"),
            slug: slug.to_string(),
            book,
            digests,
            status: Ok(BatchStatus::Ready(ai::Response {
                text: text.to_string(),
                usage: ai::Usage::default(),
            })),
        }
    }

    /// A batch that ended in failure with `reason`.
    pub(crate) fn failed(slug: &str, book: usize, reason: &str) -> Self {
        Self::Reconciled {
            batch_id: format!("batch-{slug}-{book}"),
            slug: slug.to_string(),
            book,
            digests: Digests::new(),
            status: Ok(BatchStatus::Failed(reason.to_string())),
        }
    }

    /// A batch still processing — must leave the record untouched.
    pub(crate) fn processing(slug: &str, book: usize) -> Self {
        Self::Reconciled {
            batch_id: format!("batch-{slug}-{book}"),
            slug: slug.to_string(),
            book,
            digests: Digests::new(),
            status: Ok(BatchStatus::Processing),
        }
    }
}

/// The outcome of applying one reconciled batch — for the summary tally.
enum Applied {
    /// The rebuild was written to the bible.
    Wrote,
    /// The manuscript drifted since submit; not applied.
    Stale,
    /// The pass couldn't be resolved (an unknown slug from a newer build).
    Skipped,
}
