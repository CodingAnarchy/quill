//! Backend resolution + AI-request spawning: the [`BackendSpec`] a request
//! resolves to (Claude or a lazily-built local model), the generic worker-spawn
//! lifecycle every AI pass shares, and the `poll_ai` outcome router. Split out of
//! `worker.rs` (#839). `#[cfg(feature = "local")]` gating on `build_local` kept
//! intact.

use super::super::*;

use super::download::LOCAL_UNAVAILABLE;
use crate::mode::AiAction;

/// A resolved backend choice, ready to build on the worker thread. Built here
/// (not in `spawn_ai`) so loading the local model can't block the UI.
pub(in crate::app) enum BackendSpec {
    /// The Claude HTTP backend, with its API key.
    Claude(String),
    /// The local backend, sharing `App`'s per-model cache so a loaded model is
    /// reused across requests. The `Model` (#377) is the one the router picked, or
    /// `None` to load the single hardware-recommended model. `fallback` carries the
    /// Claude API key to fall back to (#725) when the local model can't be obtained
    /// (download/init fails) — `None` when Claude isn't ready, so the local error
    /// surfaces as-is (still actionable). `notice` is the shared slot the fallback
    /// writes so the switch to Claude is never silent.
    Local {
        slot: Arc<Mutex<HashMap<String, Arc<dyn Backend>>>>,
        model: Option<&'static ai::model_select::Model>,
        fallback: Option<String>,
        notice: Arc<Mutex<Option<String>>>,
    },
}

impl BackendSpec {
    /// A local spec with no Claude fallback and a throwaway notice slot — for the
    /// callers that don't route (the assistant, image passes), where a local build
    /// error should just surface.
    pub(in crate::app) fn local(
        slot: Arc<Mutex<HashMap<String, Arc<dyn Backend>>>>,
        model: Option<&'static ai::model_select::Model>,
    ) -> Self {
        Self::Local {
            slot,
            model,
            fallback: None,
            notice: Arc::new(Mutex::new(None)),
        }
    }

    /// Build (or fetch the cached) backend. Runs on the worker thread. When a
    /// routed-local build fails and a Claude `fallback` is available, it builds
    /// Claude instead and records why in `notice` (#725), turning a routing
    /// decision into graceful degradation rather than a hard failure.
    pub(in crate::app) fn build(self) -> Result<Arc<dyn Backend>, ai::AiError> {
        match self {
            Self::Claude(key) => Ok(Arc::new(ClaudeBackend::new(key))),
            Self::Local {
                slot,
                model,
                fallback,
                notice,
            } => match build_local(&slot, model) {
                Ok(backend) => Ok(backend),
                // No fallback (Claude not ready): surface the actionable local error
                // (which names the model and how to fix it, #725).
                Err(local_err) => fallback.map_or(Err(local_err), |key| {
                    if let Ok(mut slot) = notice.lock() {
                        *slot = Some(
                            "Local model isn't available — ran on Claude instead. \
                             Download it from \"Local AI models\" (Ctrl-K) to run on-device."
                                .to_string(),
                        );
                    }
                    Ok(Arc::new(ClaudeBackend::new(key)))
                }),
            },
        }
    }
}

/// Load the chosen local model once and cache it under its id, reusing it on later
/// requests (the model resets its KV cache each request, so reuse is correct).
/// `None` loads the single hardware-recommended model (cached under `"auto"`), so
/// an author who configured no model set keeps the prior behavior (#377).
#[cfg(feature = "local")]
fn build_local(
    slot: &Arc<Mutex<HashMap<String, Arc<dyn Backend>>>>,
    model: Option<&'static ai::model_select::Model>,
) -> Result<Arc<dyn Backend>, ai::AiError> {
    let key = model.map_or("auto", ai::model_select::Model::id);
    // Fast path: hold the lock only for the cache lookup, never across the load.
    {
        let cache = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(backend) = cache.get(key) {
            return Ok(backend.clone());
        }
    }
    // Load WITHOUT the lock held: this can download and parse a multi-GB GGUF and
    // take minutes, and holding the cache mutex across it would freeze any UI-thread
    // caller (e.g. `remove_model`) for the whole load (reliability audit).
    let loaded = match model {
        Some(m) => ai::local::LocalBackend::for_model(m)?,
        None => ai::local::LocalBackend::recommended()?.0,
    };
    let backend: Arc<dyn Backend> = Arc::new(loaded);
    // Re-lock to insert. If another thread loaded the same model meanwhile, keep the
    // one already cached and drop ours, so every caller shares a single instance.
    let mut cache = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Ok(cache.entry(key.to_string()).or_insert(backend).clone())
}

#[cfg(not(feature = "local"))]
fn build_local(
    _slot: &Arc<Mutex<HashMap<String, Arc<dyn Backend>>>>,
    _model: Option<&'static ai::model_select::Model>,
) -> Result<Arc<dyn Backend>, ai::AiError> {
    Err(ai::AiError::NotConfigured(LOCAL_UNAVAILABLE.into()))
}

impl App {
    /// Spawn a generative AI pass (#255/#256/#257) from a short typed input — a
    /// premise or concept, not the manuscript. Trims and guards the input, then
    /// runs `produce` off the worker thread (the input + model are handed to it).
    /// A no-op for empty input; works on an empty project.
    pub(in crate::app) fn spawn_generative(
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
    pub(in crate::app) fn spawn_ai(
        &mut self,
        message: &str,
        run: impl FnOnce(&dyn Backend, String, String) -> Outcome + Send + 'static,
    ) {
        let model = self.pass_cloud_model();
        let manuscript = self.manuscript_text();
        self.spawn_outcome(message, move |backend| run(backend, manuscript, model));
    }

    /// The cloud model the next manuscript request should build with (#930): the
    /// one-shot the per-action router armed in `route_request` (a light, small pass
    /// stepped down to a cheaper tier), or the session `[ai] model` when nothing was
    /// armed (heavy passes, and anything that skipped the picker). Consumed here so
    /// a choice never leaks into a later request. The *local* backend ignores this
    /// (it picks its own on-device model, `routed_model`); the model only shapes the
    /// Claude request the build closure assembles.
    fn pass_cloud_model(&self) -> String {
        self.routed_cloud_model
            .take()
            .unwrap_or_else(|| self.ai.model.clone())
    }

    /// Spawn a manuscript analysis pass (#425): `build` makes the request from the
    /// manuscript text + model, `parse` turns the completed response into the
    /// pass's typed result, and `wrap` is the matching [`Outcome`] variant. This
    /// is the `complete`-then-`parse`-then-wrap plumbing every analysis pass
    /// shared by hand — most call it with the module's `build_request`/`parse`
    /// functions and the variant directly, one declarative line.
    pub(in crate::app) fn spawn_manuscript_pass<T>(
        &mut self,
        message: &str,
        build: impl FnOnce(String, &str) -> ai::Request + Send + 'static,
        parse: impl FnOnce(&str) -> Result<T, ai::AiError> + Send + 'static,
        wrap: impl FnOnce(Result<T, ai::AiError>) -> Outcome + Send + 'static,
    ) {
        self.spawn_ai(message, move |backend, manuscript, model| {
            wrap(
                backend
                    .complete(&build(manuscript, &model))
                    .and_then(|r| parse(&r.text)),
            )
        });
    }

    /// The shared worker-spawn lifecycle for an AI request: bail if one's already
    /// in flight, resolve the backend (gating readiness on the UI thread; the
    /// heavy build happens off-thread), then run `produce` to an `Outcome`.
    pub(in crate::app) fn spawn_outcome(
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

    /// Run a pre-built request on a worker thread (#90), delivering `finish`'s
    /// `Outcome` via `ai_rx`. Like [`App::spawn_ai`], but the request is assembled
    /// by the caller (so it can be a full or an incremental pass).
    pub(in crate::app) fn spawn_request(
        &mut self,
        message: &str,
        req: ai::Request,
        batch: bool,
        finish: impl FnOnce(Result<ai::Response, ai::AiError>) -> Outcome + Send + 'static,
    ) {
        self.spawn_outcome(message, move |backend| {
            let result = if batch {
                backend.complete_batched(&req)
            } else {
                backend.complete(&req)
            };
            finish(result)
        });
    }

    /// A human label for the AI backend that will actually run (#371): the active
    /// one, accounting for the `offline` privacy pin. Shown in the run-action menu
    /// and the switch's confirmation.
    pub(crate) const fn ai_backend_label(&self) -> &'static str {
        // `Settings::backend()` is the authoritative resolver — it already folds the
        // `offline` pin into Local — so the mapping lives in one place; `offline` is
        // read here only to word *why* it's local.
        match self.ai.backend() {
            ai::BackendKind::Claude => "Claude (cloud)",
            ai::BackendKind::Local if self.ai.offline => "local (offline pinned)",
            ai::BackendKind::Local => "local (on-device)",
        }
    }

    /// Switch the active AI backend between Claude and the local model from inside
    /// Quill (#371). Session-only — the next AI request re-reads the choice, the
    /// local model loads off the UI thread on first use, and a restart reverts to
    /// the `[ai] backend` in config. `offline = true` stays authoritative: it pins
    /// local, so the toggle reports the pin rather than silently contradicting it.
    pub(crate) fn toggle_ai_backend(&mut self) {
        use ai::BackendKind;
        if self.ai.offline {
            self.message = Some(
                "Offline mode pins the local backend — unset `offline` under [ai] to switch."
                    .into(),
            );
            return;
        }
        let next = match self.ai.backend {
            BackendKind::Claude => BackendKind::Local,
            BackendKind::Local => BackendKind::Claude,
        };
        // Don't appear to switch to a local backend this build can't run (#371).
        if next == BackendKind::Local && cfg!(not(feature = "local")) {
            self.message = Some(LOCAL_UNAVAILABLE.into());
            return;
        }
        self.ai.backend = next;
        self.message = Some(match next {
            BackendKind::Claude if self.ai.resolved_key().is_none() => {
                "AI backend: Claude (cloud) for this session — set an API key to use it.".into()
            }
            BackendKind::Claude => "AI backend: Claude (cloud) for this session.".into(),
            BackendKind::Local => "AI backend: local (on-device) for this session.".into(),
        });
    }

    /// Resolve the `chosen` backend into a [`BackendSpec`] after the readiness
    /// gate (#433): check `ready_for`, fail fast when local isn't compiled in,
    /// then build it — Claude with the resolved key, or local with `model`. Cheap;
    /// the heavy local-model load is deferred to [`BackendSpec::build`] on the
    /// worker thread. The one shared resolution body, behind [`backend_spec`] and
    /// [`assistant_backend`].
    fn build_spec(
        &self,
        chosen: ai::BackendKind,
        model: Option<&'static ai::model_select::Model>,
    ) -> Result<BackendSpec, ai::AiError> {
        self.ai.ready_for(chosen)?;
        // Fail fast on the UI thread when local wasn't compiled in, rather than
        // spawning a worker just to error.
        if chosen == ai::BackendKind::Local && cfg!(not(feature = "local")) {
            return Err(ai::AiError::NotConfigured(LOCAL_UNAVAILABLE.into()));
        }
        Ok(match chosen {
            ai::BackendKind::Claude => {
                // ready_for already guaranteed a key for Claude; resolve it
                // defensively rather than unwrapping.
                let key = self
                    .ai
                    .resolved_key()
                    .ok_or_else(|| ai::AiError::NotConfigured("no API key".into()))?;
                BackendSpec::Claude(key)
            }
            ai::BackendKind::Local => BackendSpec::local(self.local_backend.clone(), model),
        })
    }

    /// Choose the backend for a request from settings (after the readiness gate).
    /// The hybrid router (#372/#377) may have picked the backend and model for this
    /// one request; otherwise the session backend with no per-pass model. Both
    /// one-shots are consumed every call so a choice never leaks into a later
    /// request, and the in-flight indicator is set once a spec resolves.
    pub(in crate::app) fn backend_spec(&self) -> Result<BackendSpec, ai::AiError> {
        let routed = self.routed_backend.take();
        let chosen = routed.unwrap_or_else(|| self.ai.backend());
        let model = self.routed_model.take();
        let mut spec = self.build_spec(chosen, model)?;
        // When the *router* (not the offline pin) sent this pass on-device and
        // Claude is ready, arm a Claude fallback (#725): if the local model can't
        // be obtained at build time (not downloaded / download fails / offline),
        // the worker runs on Claude and notes the switch rather than hard-failing.
        // An `offline` pin is deliberate privacy — it never falls back.
        if let BackendSpec::Local {
            fallback, notice, ..
        } = &mut spec
        {
            if routed == Some(ai::BackendKind::Local)
                && !self.ai.offline
                && self.ai.ready_for(ai::BackendKind::Claude).is_ok()
            {
                *fallback = self.ai.resolved_key();
                *notice = self.ai_routing_notice.clone();
            }
        }
        self.ai_in_flight.set(Some(chosen)); // the status bar shows where it runs
        Ok(spec)
    }

    /// Take (and clear) any fallback notice a routed-local build left when it
    /// dropped to Claude (#725), so `poll_ai` can surface it once.
    fn take_routing_notice(&self) -> Option<String> {
        self.ai_routing_notice
            .lock()
            .ok()
            .and_then(|mut n| n.take())
    }

    /// The backend the assistant pane runs its tool-use turns on (#310): the
    /// session backend (an `offline` pin forces local), the hardware-recommended /
    /// configured model (no per-turn size routing). Unlike [`backend_spec`], it
    /// doesn't touch the pass status indicator or consume the per-pass routing
    /// one-shots — the assistant has its own task and "thinking" state.
    pub(in crate::app) fn assistant_backend(&self) -> Result<BackendSpec, ai::AiError> {
        self.build_spec(self.ai.backend(), None)
    }

    /// A label for the backend a running AI request is using (#372) — `Claude` or
    /// `local` — for the in-flight status indicator. Defaults to the session
    /// backend's label when nothing is recorded.
    pub(crate) fn ai_in_flight_label(&self) -> &'static str {
        match self.ai_in_flight.get().unwrap_or_else(|| self.ai.backend()) {
            ai::BackendKind::Claude => "Claude",
            ai::BackendKind::Local => "local",
        }
    }

    /// Run an AI action the author confirmed at the routing prompt (#372), on the
    /// chosen `backend`. Mid-workflow it runs as the suspended step; otherwise
    /// directly. Shared by the cloud / local choices of the confirm.
    pub(crate) fn run_confirmed_ai(
        &mut self,
        action: AiAction,
        force: bool,
        backend: ai::BackendKind,
    ) {
        self.route_request(backend, action, force); // backend + the picked local model (#377)
        if self.workflow_run.is_some() {
            self.start_async_step(action);
        } else {
            self.run_ai_action(action, force);
        }
    }

    /// Poll the in-flight AI request (non-blocking). On a result, dispatch by
    /// feature and clear the pending state. Called once per loop iteration so the
    /// result lands within the poll window.
    pub fn poll_ai(&mut self) {
        // A stuck AI step shouldn't wedge a running sequenced run — a workflow
        // (#227) or a series-bible refresh (#340). One tick for whichever is
        // active (#344).
        self.tick_run_timeout();
        match self.ai_task.poll() {
            Poll::Ready(outcome) => {
                self.message = None;
                self.ai_in_flight.set(None); // the request finished — clear the backend indicator
                self.overlay = None; // a fresh result supersedes any open overlay
                let step_error = outcome.error().map(std::string::ToString::to_string);
                // If the assistant pane (#392) is waiting on this pass, capture its
                // result before the outcome is consumed, then resume after applying.
                let assistant_summary = self.assistant_pass_summary(&outcome);
                self.apply_outcome(outcome);
                // If a routed-local pass fell back to Claude (#725), surface why —
                // the pass ran, but the author should know their local model wasn't
                // used (and how to enable it). Overrides the per-feature success
                // message since the fallback is the more important thing to know.
                if let Some(notice) = self.take_routing_notice() {
                    if step_error.is_none() {
                        self.message = Some(notice);
                    }
                }
                // The in-flight pass is done; a clean bible apply already recorded
                // its state (#90), so drop any snapshot a failed/conflicted one left.
                self.pending_pass = None;
                // If this AI request was a sequenced-run step (a workflow or a
                // series-bible refresh pass), advance that run (#344).
                self.advance_run(step_error);
                // Resume the assistant pane's parked batch with the pass result (#392).
                if let Some(summary) = assistant_summary {
                    self.resume_assistant_pass(summary);
                }
            }
            Poll::Pending => {}
            Poll::Died(cause) => {
                // Surface the worker's real cause (#838) rather than a bare failure.
                let reason = cause.unwrap_or_else(|| "request failed".into());
                // Resume the assistant if it was waiting on this (now dead) pass (#392).
                if let Some(summary) =
                    self.assistant_pass_summary(&Outcome::Error(ai::AiError::Network {
                        status: None,
                        message: reason.clone(),
                    }))
                {
                    self.resume_assistant_pass(summary);
                }
                self.message = Some(format!("AI: {reason}"));
                self.advance_run(Some(reason));
            }
        }
        // Fire a setup wizard's deferred setting draft once the worker is free (#282).
        self.pump_wizard_drafts();
    }

    /// Dispatch a completed [`Outcome`] to its feature's `apply_*` handler — the one
    /// place the per-feature routing lives. Called by [`poll_ai`](App::poll_ai) for
    /// the interactive worker's result, and reused by the background batch update
    /// (#950) when a reconciled batch's rebuild is applied through the same path
    /// (the tracker's `finish` turns the batch response into the matching variant).
    pub(in crate::app) fn apply_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Answer(r) => self.apply_qa_result(r),
            Outcome::Characters(r) => self.apply_characters(r),
            Outcome::Continuity(r) => self.apply_continuity(r),
            Outcome::Setting(r) => self.apply_setting(r),
            Outcome::Plot(r) => self.apply_plot(r),
            Outcome::Theme(r) => self.apply_theme(r),
            Outcome::Scaffold(r) => self.apply_scaffold(r),
            // Every analysis-only findings pass funnels through one generic apply (#563);
            // each finding is already attributed to its action by the worker.
            Outcome::Findings(r) => self.apply_findings(r),
            Outcome::Submission(r) => self.apply_submission(r),
            Outcome::Brainstorm(r) => self.apply_brainstorm(r),
            Outcome::AdCreative(r) => self.apply_ad_creative(r),
            Outcome::Marketing(r) => self.apply_marketing(r),
            Outcome::Social(r) => self.apply_social(r),
            Outcome::Image(r) => self.apply_images(r),
            Outcome::Workflow(r) => self.apply_generated_workflow(r),
            Outcome::Config(r) => self.apply_generated_config(r),
            Outcome::Import(r) => self.apply_import(r),
            Outcome::Thesaurus(r) => self.apply_thesaurus(r),
            Outcome::NameGen(r) => self.apply_namegen(r),
            Outcome::SemanticFind(r) => self.apply_semantic_find(r),
            Outcome::Echo(r) => self.apply_echo(r),
            Outcome::Recheck(r) => self.apply_recheck(r),
            Outcome::RecheckAll(r) => self.apply_recheck_all(r),
            Outcome::ScopedPass(r) => self.apply_scoped_pass(r),
            Outcome::Error(e) => self.message = Some(format!("AI: {e}")),
        }
    }
}

/// Hermetic tests for the routed-local → Claude fallback build (#725). No
/// network, no download: in the default (non-`local`) test build `build_local`
/// always errors, which is exactly the "local can't be obtained" condition the
/// fallback rides. Gated to that build so no real fetch is attempted.
#[cfg(all(test, not(feature = "local")))]
mod fallback_build_tests {
    use super::*;

    fn slot() -> Arc<Mutex<HashMap<String, Arc<dyn Backend>>>> {
        Arc::new(Mutex::new(HashMap::new()))
    }

    #[test]
    fn a_routed_local_build_falls_back_to_claude_with_a_notice() {
        let notice = Arc::new(Mutex::new(None));
        let spec = BackendSpec::Local {
            slot: slot(),
            model: None,
            fallback: Some("sk-test".into()),
            notice: notice.clone(),
        };
        // The local build can't produce a model → it builds Claude instead, and
        // records why so the switch is never silent.
        assert!(
            spec.build().is_ok(),
            "fell back to Claude, didn't hard-fail"
        );
        let msg = notice.lock().unwrap().clone().expect("a fallback notice");
        assert!(msg.contains("Claude"), "{msg}");
        assert!(
            msg.contains("Local AI models"),
            "actionable next step: {msg}"
        );
    }

    #[test]
    fn a_routed_local_build_without_a_fallback_surfaces_the_local_error() {
        let notice = Arc::new(Mutex::new(None));
        let spec = BackendSpec::Local {
            slot: slot(),
            model: None,
            fallback: None, // Claude not ready → no fallback
            notice: notice.clone(),
        };
        // With no fallback the local error surfaces (still actionable), and no
        // spurious fallback notice is left behind.
        assert!(spec.build().is_err(), "local error surfaces, not swallowed");
        assert!(notice.lock().unwrap().is_none(), "no fallback happened");
    }
}
