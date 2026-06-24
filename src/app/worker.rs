//! The AI worker: backend resolution, request spawning on the worker thread,
//! incremental-scope plumbing (#90), and the `poll_ai` outcome router. Extracted
//! from `assist.rs` (#315).

use super::*;

use crate::mode::AiAction;

/// Shown when the local backend is selected but wasn't compiled in. One source
/// so the fast-fail (UI thread) and the `build_local` stub can't drift.
const LOCAL_UNAVAILABLE: &str =
    "local backend isn't in this build — rebuild Quill with `--features local`, or set `backend = \"claude\"`";

/// A resolved backend choice, ready to build on the worker thread. Built here
/// (not in `spawn_ai`) so loading the local model can't block the UI.
pub(super) enum BackendSpec {
    /// The Claude HTTP backend, with its API key.
    Claude(String),
    /// The local backend, sharing `App`'s cache slot so the loaded model is
    /// reused across requests.
    Local(Arc<Mutex<Option<Arc<dyn Backend>>>>),
}

impl BackendSpec {
    /// Build (or fetch the cached) backend. Runs on the worker thread.
    pub(super) fn build(self) -> Result<Arc<dyn Backend>, ai::AiError> {
        match self {
            BackendSpec::Claude(key) => Ok(Arc::new(ClaudeBackend::new(key))),
            BackendSpec::Local(slot) => build_local(slot),
        }
    }
}

/// Load the local model once and cache it in the shared slot, reusing it on later
/// requests (the model resets its KV cache each request, so reuse is correct).
#[cfg(feature = "local")]
fn build_local(slot: Arc<Mutex<Option<Arc<dyn Backend>>>>) -> Result<Arc<dyn Backend>, ai::AiError> {
    let mut guard = slot.lock().map_err(|_| ai::AiError::Internal("model lock poisoned".into()))?;
    if let Some(backend) = guard.as_ref() {
        return Ok(backend.clone());
    }
    let (backend, _rec) = ai::local::LocalBackend::recommended()?;
    let backend: Arc<dyn Backend> = Arc::new(backend);
    *guard = Some(backend.clone());
    Ok(backend)
}

#[cfg(not(feature = "local"))]
fn build_local(_slot: Arc<Mutex<Option<Arc<dyn Backend>>>>) -> Result<Arc<dyn Backend>, ai::AiError> {
    Err(ai::AiError::NotConfigured(LOCAL_UNAVAILABLE.into()))
}

/// Where a persistent-bible tracker keeps its prior bible, for the incremental
/// cache (#90).
enum BibleSource {
    /// A directory of files, e.g. `bible/characters/`.
    Dir(&'static str),
    /// A single file, e.g. `bible/setting.md`.
    File(&'static str),
}

/// A persistent-bible tracker's incremental-scoping descriptor (#90): its state
/// key, where its prior bible lives, and the status message for each scope.
pub(super) struct BibleTracker {
    key: &'static str,
    source: BibleSource,
    full_msg: &'static str,
    incr_msg: &'static str,
}

/// The bible tracker behind a costed AI action, or `None` for findings-only and
/// interactive actions (which send the whole manuscript or nothing).
pub(super) fn bible_tracker(action: AiAction) -> Option<BibleTracker> {
    let make = |key, source, full_msg, incr_msg| BibleTracker { key, source, full_msg, incr_msg };
    match action {
        AiAction::Characters => Some(make(
            "characters",
            BibleSource::Dir("characters"),
            "⟳ Analyzing characters…",
            "⟳ Updating characters from changed chapters…",
        )),
        AiAction::Continuity => Some(make(
            "timeline",
            BibleSource::File("timeline.md"),
            "⟳ Checking continuity…",
            "⟳ Updating the timeline from changed chapters…",
        )),
        AiAction::Setting => Some(make(
            "setting",
            BibleSource::File("setting.md"),
            "⟳ Building setting bible…",
            "⟳ Updating the setting bible from changed chapters…",
        )),
        _ => None,
    }
}

impl App {
    /// Spawn a generative AI pass (#255/#256/#257) from a short typed input — a
    /// premise or concept, not the manuscript. Trims and guards the input, then
    /// runs `produce` off the worker thread (the input + model are handed to it).
    /// A no-op for empty input; works on an empty project.
    pub(super) fn spawn_generative(
        &mut self,
        input: &str,
        message: &str,
        produce: impl FnOnce(&dyn Backend, &str, String) -> Outcome + Send + 'static,
    ) {
        let input = input.trim().to_string();
        if input.is_empty() {
            return;
        }
        let model = self.ai.model.clone();
        self.spawn_outcome(message, move |backend| produce(backend, &input, model));
    }

    /// Run an AI request on a worker thread: assemble the manuscript here, hand it
    /// (with a backend + model) to `run` off-thread, and deliver the `Outcome` via
    /// `ai_rx` for [`App::poll_ai`]. A no-op with a status message when AI isn't
    /// ready or a request is already in flight.
    pub(super) fn spawn_ai(
        &mut self,
        message: &str,
        run: impl FnOnce(&dyn Backend, String, String) -> Outcome + Send + 'static,
    ) {
        let model = self.ai.model.clone();
        let manuscript = self.manuscript_text();
        self.spawn_outcome(message, move |backend| run(backend, manuscript, model));
    }

    /// The shared worker-spawn lifecycle for an AI request: bail if one's already
    /// in flight, resolve the backend (gating readiness on the UI thread; the
    /// heavy build happens off-thread), then run `produce` to an `Outcome`.
    pub(super) fn spawn_outcome(
        &mut self,
        message: &str,
        produce: impl FnOnce(&dyn Backend) -> Outcome + Send + 'static,
    ) {
        if self.ai_task.is_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        let spec = match self.backend_spec() {
            Ok(spec) => spec,
            Err(e) => {
                self.message = Some(e.to_string());
                return;
            }
        };
        self.ai_task.spawn(move || match spec.build() {
            Ok(backend) => produce(backend.as_ref()),
            Err(e) => Outcome::Error(e),
        });
        self.message = Some(message.to_string());
    }

    /// Read a tracker's prior bible and decide its scope from the revision delta
    /// (#90) — shared by the cost estimate and the run so they always agree. A
    /// missing/empty bible can't be revised and isn't truly "up to date" (e.g. it
    /// was hand-deleted while the state file lingered), so it rebuilds in full.
    fn plan_bible_pass(&self, t: &BibleTracker) -> (ai::incremental::Scope, String, ai::incremental::Digests) {
        let prior = match t.source {
            BibleSource::Dir(dir) => bible::raw_dir(&self.project.root, dir),
            BibleSource::File(file) => bible::raw_file(&self.project.root, file),
        };
        let current = self.chapter_digests();
        let scope = if prior.trim().is_empty() {
            ai::incremental::Scope::Full
        } else {
            let state = ai::incremental::State::load(&self.project.root);
            ai::incremental::decide(state.built_from(t.key), &current, self.ai.refresh_after_changes)
        };
        (scope, prior, current)
    }

    /// Run a persistent-bible tracker with incremental scoping (#90): revise the
    /// bible from only the changed chapters, rebuild it from the whole manuscript,
    /// or skip (already current). `full`/`incr` build the request for each case.
    pub(super) fn run_bible_tracker(
        &mut self,
        t: BibleTracker,
        force: bool,
        full: impl FnOnce(String, &str) -> ai::Request,
        incr: impl FnOnce(&str, &str, &str) -> ai::Request,
        finish: impl FnOnce(Result<ai::Response, ai::AiError>) -> Outcome + Send + 'static,
    ) {
        let (scope, prior, current) = self.plan_bible_pass(&t);
        // The AI palette's `R` forces a full rebuild regardless of the delta (#176).
        let scope = if force { ai::incremental::Scope::Full } else { scope };
        // Full rebuilds run through the Batch API (half price, higher latency) on
        // the Claude backend; the local backend has no batch path.
        let claude = self.ai.backend() == ai::BackendKind::Claude;
        let model = self.ai.model.clone();
        let (req, base_msg, batch) = match scope {
            ai::incremental::Scope::UpToDate => {
                self.message =
                    Some(format!("{}: bible is current — no chapters changed since the last pass.", t.key));
                return;
            }
            ai::incremental::Scope::Full => (full(self.manuscript_text(), &model), t.full_msg, claude),
            ai::incremental::Scope::Incremental(changed) => {
                (incr(&prior, &self.chapters_text(&changed), &model), t.incr_msg, false)
            }
        };
        // Snapshot the manuscript state now; recorded once the pass applies cleanly.
        self.pending_pass = Some((t.key.to_string(), current));
        let message = if batch {
            format!("{base_msg} (batch — may take a few minutes)")
        } else {
            base_msg.to_string()
        };
        self.spawn_request(&message, req, batch, finish);
    }

    /// Run a pre-built request on a worker thread (#90), delivering `finish`'s
    /// `Outcome` via `ai_rx`. Like [`App::spawn_ai`], but the request is assembled
    /// by the caller (so it can be a full or an incremental pass).
    fn spawn_request(
        &mut self,
        message: &str,
        req: ai::Request,
        batch: bool,
        finish: impl FnOnce(Result<ai::Response, ai::AiError>) -> Outcome + Send + 'static,
    ) {
        self.spawn_outcome(message, move |backend| {
            let result = if batch { backend.complete_batched(&req) } else { backend.complete(&req) };
            finish(result)
        });
    }

    /// The chapter `file → content-digest` map for the manuscript as it is now —
    /// the revision-delta baseline for incremental scoping (#90).
    pub(crate) fn chapter_digests(&self) -> ai::incremental::Digests {
        (0..self.chapter_count())
            .filter_map(|i| {
                let file = self.project.chapter_file(i);
                (!file.is_empty())
                    .then(|| (file.to_string(), ai::incremental::digest(&self.chapter_text(i))))
            })
            .collect()
    }

    /// The text of the chapters whose files are in `files`, each under its title
    /// heading — the changed-chapter context for an incremental pass (#90).
    pub(crate) fn chapters_text(&self, files: &[String]) -> String {
        self.render_chapters(|i| files.iter().any(|f| f == self.project.chapter_file(i)))
    }

    /// Record what the just-applied bible pass was built from (#90), so the next
    /// run only re-analyzes what changed. Best effort — a state-write failure
    /// doesn't undo the pass that already succeeded. A no-op for non-bible passes
    /// (which leave `pending_pass` unset).
    pub(super) fn record_pass_state(&mut self) {
        if let Some((tracker, digests)) = self.pending_pass.take() {
            let today = date::iso(date::today());
            let _ = ai::incremental::State::record(&self.project.root, &tracker, &today, digests);
        }
    }

    /// The byte length of the context a costed pass will actually send — for the
    /// cost estimate (#90). The whole manuscript for most passes, but only the
    /// bible + changed chapters for an incremental tracker pass, and zero when its
    /// bible is already current (so an up-to-date pass never prompts a confirm).
    pub(crate) fn pass_context_len(&self, action: AiAction, force: bool) -> usize {
        let Some(t) = bible_tracker(action) else {
            return self.manuscript_text().len();
        };
        if force {
            return self.manuscript_text().len(); // a forced rebuild sends the whole book
        }
        let (scope, prior, _) = self.plan_bible_pass(&t);
        match scope {
            ai::incremental::Scope::UpToDate => 0,
            ai::incremental::Scope::Full => self.manuscript_text().len(),
            ai::incremental::Scope::Incremental(changed) => prior.len() + self.chapters_text(&changed).len(),
        }
    }

    /// Choose the backend for a request from settings (after the readiness gate).
    /// Cheap — the heavy local-model load is deferred to [`BackendSpec::build`] on
    /// the worker thread.
    pub(super) fn backend_spec(&self) -> Result<BackendSpec, ai::AiError> {
        self.ai.check_ready()?;
        Ok(match self.ai.backend() {
            ai::BackendKind::Claude => {
                // check_ready already guaranteed a key for Claude; resolve it
                // defensively rather than unwrapping.
                let key = self
                    .ai
                    .resolved_key()
                    .ok_or_else(|| ai::AiError::NotConfigured("no API key".into()))?;
                BackendSpec::Claude(key.to_string())
            }
            ai::BackendKind::Local => {
                // Fail fast on the UI thread when the local backend wasn't compiled
                // in, so we don't spawn a worker just to error.
                if cfg!(not(feature = "local")) {
                    return Err(ai::AiError::NotConfigured(LOCAL_UNAVAILABLE.into()));
                }
                BackendSpec::Local(self.local_backend.clone())
            }
        })
    }

    /// Poll the in-flight AI request (non-blocking). On a result, dispatch by
    /// feature and clear the pending state. Called once per loop iteration so the
    /// result lands within the poll window.
    pub fn poll_ai(&mut self) {
        // A stuck AI step shouldn't wedge a running workflow (#227).
        self.tick_workflow_timeout();
        match self.ai_task.poll() {
            Poll::Ready(outcome) => {
                self.message = None;
                self.overlay = None; // a fresh result supersedes any open overlay
                let step_error = outcome.error().map(|e| e.to_string());
                match outcome {
                    Outcome::Answer(r) => self.apply_qa_result(r),
                    Outcome::Characters(r) => self.apply_characters(r),
                    Outcome::Continuity(r) => self.apply_continuity(r),
                    Outcome::Setting(r) => self.apply_setting(r),
                    Outcome::Scaffold(r) => self.apply_scaffold(r),
                    Outcome::Naming(r) => self.apply_naming(r),
                    Outcome::Style(r) => self.apply_style(r),
                    Outcome::Submission(r) => self.apply_submission(r),
                    Outcome::Brainstorm(r) => self.apply_brainstorm(r),
                    Outcome::AdCreative(r) => self.apply_ad_creative(r),
                    Outcome::Marketing(r) => self.apply_marketing(r),
                    Outcome::Social(r) => self.apply_social(r),
                    Outcome::Voice(r) => self.apply_voice(r),
                    Outcome::Image(r) => self.apply_images(r),
                    Outcome::Workflow(r) => self.apply_generated_workflow(r),
                    Outcome::Import(r) => self.apply_import(r),
                    Outcome::Thesaurus(r) => self.apply_thesaurus(r),
                    Outcome::Editorial(r) => self.apply_editorial(r),
                    Outcome::Error(e) => self.message = Some(format!("AI: {e}")),
                }
                // The in-flight pass is done; a clean bible apply already recorded
                // its state (#90), so drop any snapshot a failed/conflicted one left.
                self.pending_pass = None;
                // If this AI request was a workflow step, advance the workflow.
                self.advance_after_ai(step_error);
            }
            Poll::Pending => {}
            Poll::Died => {
                self.message = Some("AI: request failed".into());
                self.advance_after_ai(Some("request failed".into()));
            }
        }
        // Fire a setup wizard's deferred setting draft once the worker is free (#282).
        self.pump_wizard_drafts();
    }

    /// Assemble the chapters for which `keep(i)` holds, each under its `# Title`
    /// heading (active chapter live from its buffer, the rest from disk) — the one
    /// serialization shared by the whole-manuscript and changed-chapter contexts.
    fn render_chapters(&self, keep: impl Fn(usize) -> bool) -> String {
        let mut out = String::new();
        for i in 0..self.chapter_count() {
            if !keep(i) {
                continue;
            }
            if let Some(title) = self.project.chapter_title(i) {
                out.push_str("# ");
                out.push_str(title);
                out.push_str("\n\n");
            }
            out.push_str(self.chapter_text(i).trim_end());
            out.push_str("\n\n");
        }
        out
    }

    /// The whole manuscript as plain text for an AI request.
    pub(crate) fn manuscript_text(&self) -> String {
        self.render_chapters(|_| true)
    }

    /// The known entity list for naming checks: the setting gazetteer plus the
    /// character names from the story bible. Empty when no bible has been built.
    pub(crate) fn known_entities(&self) -> Vec<String> {
        let root = &self.project.root;
        let mut out = bible::gazetteer(root);
        out.extend(bible::character_names(root));
        out.sort();
        out.dedup();
        out
    }
}
