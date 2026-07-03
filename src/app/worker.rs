//! The AI worker: backend resolution, request spawning on the worker thread,
//! incremental-scope plumbing (#90), and the `poll_ai` outcome router. Extracted
//! from `assist.rs` (#315).

use super::*;

use crate::mode::AiAction;

/// Shown when the local backend is selected but wasn't compiled in. One source
/// so the fast-fail (UI thread) and the `build_local` stub can't drift.
pub(super) const LOCAL_UNAVAILABLE: &str =
    "local backend isn't in this build — rebuild Quill with `--features local`, or set `backend = \"claude\"`";

/// Whether `model`'s GGUF is already in the `hf-hub` cache (#382) — a no-download
/// lookup, so the manager can show on-disk status. Always `false` without the
/// `local` feature (nothing can be downloaded there).
#[cfg(feature = "local")]
pub(super) fn model_downloaded(model: &ai::model_select::Model) -> bool {
    hf_hub::Cache::from_env()
        .model(model.gguf_repo.to_string())
        .get(model.gguf_file)
        .is_some()
}
#[cfg(not(feature = "local"))]
pub(super) const fn model_downloaded(_model: &ai::model_select::Model) -> bool {
    false
}

/// The single hardware-recommended model this machine would load when no set is
/// configured (#725) — resolved without downloading, so the router's "is a usable
/// local model on disk?" gate can consider it. `None` when the machine is too
/// small for any model.
#[cfg(feature = "local")]
pub(super) fn recommended_model() -> Option<&'static ai::model_select::Model> {
    ai::local::LocalBackend::recommended_model()
}

/// A `hf-hub` progress sink that feeds Quill's shared [`DownloadProgress`] counter
/// (#813) instead of the disabled indicatif bar (#727). `hf-hub` calls `init` with
/// each file's byte size and `update` with every chunk read; we fold both into the
/// combined counter the UI thread renders. `finish` is a no-op — completion is
/// reported by the download task's terminal poll, not the per-file callback.
#[cfg(feature = "local")]
struct ProgressSink(std::sync::Arc<super::models::DownloadProgress>);

#[cfg(feature = "local")]
impl hf_hub::api::Progress for ProgressSink {
    fn init(&mut self, size: usize, _filename: &str) {
        self.0.add_total(size as u64);
    }
    fn update(&mut self, size: usize) {
        self.0.add_done(size as u64);
    }
    fn finish(&mut self) {}
}

/// Fetch `model`'s GGUF + tokenizer into the `hf-hub` cache (#382), without
/// loading it — the explicit "download now" the manager offers (resumable, cached
/// outside the project). Run on a worker thread. `progress` is the shared counter
/// the fetch drives so Quill can render live progress (#813).
#[cfg(feature = "local")]
pub(super) fn download_model(
    model: &ai::model_select::Model,
    progress: &std::sync::Arc<super::models::DownloadProgress>,
) -> Result<(), String> {
    // Disable hf-hub's indicatif progress bar (#727): Quill owns the alternate
    // screen, so the library's raw stderr writes would corrupt the TUI. Progress
    // is surfaced through Quill's own UI instead, via the shared counter (#813).
    let hub = hf_hub::api::sync::ApiBuilder::new()
        .with_progress(false)
        .build()
        .map_err(|e| format!("hf-hub: {e}"))?;
    fetch_with_progress(
        model.gguf_repo,
        model.gguf_file,
        &hub.model(model.gguf_repo.to_string()),
        progress,
    )
    .map_err(|e| format!("download {}: {e}", model.gguf_file))?;
    fetch_with_progress(
        model.tokenizer_repo,
        "tokenizer.json",
        &hub.model(model.tokenizer_repo.to_string()),
        progress,
    )
    .map_err(|e| format!("download tokenizer: {e}"))?;
    Ok(())
}

/// Fetch `filename` from the `repo`-named repo, reusing the cached copy if present
/// (like `ApiRepo::get`) and otherwise downloading it while driving `progress`
/// (#813). `ApiRepo::download_with_progress` always re-downloads and `ApiRepo::get`
/// gives no progress, so the cache is probed directly here: a cached file
/// short-circuits the fetch (preserving the manager's "already downloaded"
/// resumability) and still adds its size to `done` + `total`, so a partly-cached
/// multi-file download reads an honest 100%.
#[cfg(feature = "local")]
fn fetch_with_progress(
    repo_id: &str,
    filename: &str,
    api_repo: &hf_hub::api::sync::ApiRepo,
    progress: &std::sync::Arc<super::models::DownloadProgress>,
) -> Result<std::path::PathBuf, hf_hub::api::sync::ApiError> {
    if let Some(path) = hf_hub::Cache::from_env()
        .model(repo_id.to_string())
        .get(filename)
    {
        // Already cached: count its bytes so the combined percentage stays honest.
        if let Ok(len) = std::fs::metadata(&path).map(|m| m.len()) {
            progress.add_total(len);
            progress.add_done(len);
        }
        return Ok(path);
    }
    api_repo.download_with_progress(filename, ProgressSink(progress.clone()))
}

#[cfg(not(feature = "local"))]
pub(super) fn download_model(
    _model: &ai::model_select::Model,
    _progress: &std::sync::Arc<super::models::DownloadProgress>,
) -> Result<(), String> {
    Err(LOCAL_UNAVAILABLE.to_string())
}

/// Delete `model`'s GGUF from the `hf-hub` cache to reclaim disk (#382). Only the
/// large GGUF repo is removed; the tiny tokenizer is left (it may be shared).
#[cfg(feature = "local")]
pub(super) fn remove_model_files(model: &ai::model_select::Model) -> Result<(), String> {
    let cache = hf_hub::Cache::from_env();
    let dir = cache
        .path()
        .join(hf_hub::Repo::model(model.gguf_repo.to_string()).folder_name());
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("remove {}: {e}", dir.display()))?;
    }
    Ok(())
}
#[cfg(not(feature = "local"))]
pub(super) fn remove_model_files(_model: &ai::model_select::Model) -> Result<(), String> {
    Err(LOCAL_UNAVAILABLE.to_string())
}

/// A resolved backend choice, ready to build on the worker thread. Built here
/// (not in `spawn_ai`) so loading the local model can't block the UI.
pub(super) enum BackendSpec {
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
    pub(super) fn local(
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
    pub(super) fn build(self) -> Result<Arc<dyn Backend>, ai::AiError> {
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
    let mut cache = slot
        .lock()
        .map_err(|_| ai::AiError::Internal("model lock poisoned".into()))?;
    if let Some(backend) = cache.get(key) {
        return Ok(backend.clone());
    }
    let loaded = match model {
        Some(m) => ai::local::LocalBackend::for_model(m)?,
        None => ai::local::LocalBackend::recommended()?.0,
    };
    let backend: Arc<dyn Backend> = Arc::new(loaded);
    cache.insert(key.to_string(), backend.clone());
    Ok(backend)
}

#[cfg(not(feature = "local"))]
fn build_local(
    _slot: &Arc<Mutex<HashMap<String, Arc<dyn Backend>>>>,
    _model: Option<&'static ai::model_select::Model>,
) -> Result<Arc<dyn Backend>, ai::AiError> {
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
/// key, where its prior bible lives, the status message for each scope, and the
/// request builders + parse-into-[`Outcome`] that make this a *bible* pass — the
/// single source mapping a bible action to everything `run_bible_tracker` needs,
/// so the dispatch lives here, not split across a second match (#510).
pub(super) struct BibleTracker {
    key: &'static str,
    source: BibleSource,
    full_msg: &'static str,
    incr_msg: &'static str,
    /// Build the full-rebuild request from `(manuscript, model)`.
    full: fn(String, &str) -> ai::Request,
    /// Build the incremental-revise request from `(prior_bible, changed, model)`.
    incr: fn(&str, &str, &str) -> ai::Request,
    /// Parse the completed response into this tracker's [`Outcome`] variant.
    finish: fn(Result<ai::Response, ai::AiError>) -> Outcome,
}

/// The bible tracker behind a costed AI action, or `None` for findings-only and
/// interactive actions (which send the whole manuscript or nothing).
pub(super) fn bible_tracker(action: AiAction) -> Option<BibleTracker> {
    // The bible key is the descriptor's single source (#563); read it once here so
    // the incremental cache and the series-rollup label can never point at
    // different on-disk files.
    let key = action.bible_key()?;
    match action {
        AiAction::Characters => Some(BibleTracker {
            key,
            source: BibleSource::Dir("characters"),
            full_msg: "⟳ Analyzing characters…",
            incr_msg: "⟳ Updating characters from changed chapters…",
            full: character::build_request,
            incr: character::build_incremental_request,
            finish: |r| Outcome::Characters(r.and_then(|resp| character::parse(&resp.text))),
        }),
        AiAction::Continuity => Some(BibleTracker {
            key,
            source: BibleSource::File("timeline.md"),
            full_msg: "⟳ Checking continuity…",
            incr_msg: "⟳ Updating the timeline from changed chapters…",
            full: continuity::build_request,
            incr: continuity::build_incremental_request,
            finish: |r| Outcome::Continuity(r.and_then(|resp| continuity::parse(&resp.text))),
        }),
        AiAction::Setting => Some(BibleTracker {
            key,
            source: BibleSource::File("setting.md"),
            full_msg: "⟳ Building setting bible…",
            incr_msg: "⟳ Updating the setting bible from changed chapters…",
            full: setting::build_request,
            incr: setting::build_incremental_request,
            finish: |r| Outcome::Setting(r.and_then(|resp| setting::parse(&resp.text))),
        }),
        AiAction::Plot => Some(BibleTracker {
            key,
            source: BibleSource::File("plot.md"),
            full_msg: "⟳ Tracking plot threads…",
            incr_msg: "⟳ Updating the plot tracker from changed chapters…",
            full: plot::build_request,
            incr: plot::build_incremental_request,
            finish: |r| Outcome::Plot(r.and_then(|resp| plot::parse(&resp.text))),
        }),
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

    /// Spawn a manuscript analysis pass (#425): `build` makes the request from the
    /// manuscript text + model, `parse` turns the completed response into the
    /// pass's typed result, and `wrap` is the matching [`Outcome`] variant. This
    /// is the `complete`-then-`parse`-then-wrap plumbing every analysis pass
    /// shared by hand — most call it with the module's `build_request`/`parse`
    /// functions and the variant directly, one declarative line.
    pub(super) fn spawn_manuscript_pass<T>(
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
    fn plan_bible_pass(
        &self,
        t: &BibleTracker,
        book: usize,
    ) -> (ai::incremental::Scope, String, ai::incremental::Digests) {
        let prior = match t.source {
            BibleSource::Dir(dir) => bible::raw_dir(&self.project.root, dir),
            BibleSource::File(file) => bible::raw_file(&self.project.root, file),
        };
        let current = self.book_digests(book);
        let scope = if prior.trim().is_empty() {
            ai::incremental::Scope::Full
        } else {
            let state = ai::incremental::State::load(&self.project.root);
            // Scope the recorded digests to THIS book's files, so the other books'
            // chapters never register as "removed" and force a needless rebuild
            // (#339). Ownership is "not owned by another book" rather than "still in
            // the current book", so a chapter the author deleted from THIS book stays
            // in the scoped record and `decide` sees the removal (#645) instead of it
            // silently vanishing from the diff. For a standalone novel there are no
            // other books, so this is the same record `decide` saw before.
            let others = self.other_books_files(book);
            let prior_book: Option<ai::incremental::Digests> = state.built_from(t.key).map(|d| {
                d.iter()
                    .filter(|(f, _)| !others.contains(*f))
                    .map(|(f, g)| (f.clone(), g.clone()))
                    .collect()
            });
            ai::incremental::decide(prior_book.as_ref(), &current, self.ai.refresh_after_changes)
        };
        (scope, prior, current)
    }

    /// Test hook for [`plan_bible_pass`] (#645): lets the series-bible tests assert
    /// on the scope a book's pass would take without spawning the request.
    #[cfg(test)]
    pub(super) fn plan_bible_pass_for_test(
        &self,
        t: &BibleTracker,
        book: usize,
    ) -> (ai::incremental::Scope, String, ai::incremental::Digests) {
        self.plan_bible_pass(t, book)
    }

    /// Run a persistent-bible tracker with incremental scoping (#90): revise the
    /// bible from only the changed chapters, rebuild it from the whole manuscript,
    /// or skip (already current). The tracker carries its own request builders and
    /// parse (#510): `t.full`/`t.incr` build the request for each case, `t.finish`
    /// turns the response into the matching [`Outcome`].
    #[allow(clippy::needless_pass_by_value)] // consumes the tracker: t.finish is moved into spawn_request
    pub(super) fn run_bible_tracker(&mut self, t: BibleTracker, book: usize, force: bool) {
        let (scope, prior, current) = self.plan_bible_pass(&t, book);
        // The AI palette's `R` forces a full rebuild regardless of the delta (#176).
        let scope = if force {
            ai::incremental::Scope::Full
        } else {
            scope
        };
        // In a multi-book series the bible spans every book, so a "full" pass for
        // one book must *revise* the shared bible with that book's chapters, not
        // rebuild from scratch and discard the other books (#339). It also folds
        // its digests into the state rather than replacing it. A standalone novel
        // has one book, so this is exactly the prior single-book behavior.
        let series = self.project.book_count() > 1;
        // Full rebuilds run through the Batch API (half price, higher latency) on
        // the Claude backend; the local backend has no batch path.
        let claude = self.ai.backend() == ai::BackendKind::Claude;
        let model = self.ai.model.clone();
        let (req, base_msg, batch) = match scope {
            ai::incremental::Scope::UpToDate => {
                self.message = Some(format!(
                    "{}: bible is current — no chapters changed since the last pass.",
                    t.key
                ));
                return;
            }
            ai::incremental::Scope::Full if prior.trim().is_empty() || !series => {
                // First build (no bible yet), or a standalone novel: rebuild from
                // the whole book. `book_render(book, None)` is the active book's
                // whole manuscript for a single-book project.
                (
                    (t.full)(self.book_render(book, None), &model),
                    t.full_msg,
                    claude,
                )
            }
            ai::incremental::Scope::Full => {
                // Series, bible exists: revise it with all of this book's chapters.
                (
                    (t.incr)(&prior, &self.book_render(book, None), &model),
                    t.full_msg,
                    false,
                )
            }
            ai::incremental::Scope::Incremental(changed) => (
                (t.incr)(&prior, &self.book_render(book, Some(&changed)), &model),
                t.incr_msg,
                false,
            ),
        };
        // Snapshot the manuscript state now; recorded once the pass applies cleanly.
        // A series merges the book's digests into the state (carrying the other
        // books' files to preserve, so this book's deleted chapters get pruned —
        // #645); a standalone replaces. Snapshot the preserve set now too, since the
        // manifest can change before the async pass lands.
        let merge = series.then(|| self.other_books_files(book));
        self.pending_pass = Some((t.key.to_string(), current, merge));
        let message = if batch {
            format!("{base_msg} (batch — may take a few minutes)")
        } else {
            base_msg.to_string()
        };
        self.spawn_request(&message, req, batch, t.finish);
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
            let result = if batch {
                backend.complete_batched(&req)
            } else {
                backend.complete(&req)
            };
            finish(result)
        });
    }

    /// The chapter `file → content-digest` map for the active book (#90). Used by
    /// the incremental-scoping tests to seed and inspect state.
    #[cfg(test)]
    pub(crate) fn chapter_digests(&self) -> ai::incremental::Digests {
        self.book_digests(self.project.active_book())
    }

    /// The text of the active book's chapters whose files are in `files`, each
    /// under its title heading — the changed-chapter context for a pass (#90). A
    /// thin active-book wrapper over [`book_render_files`], which the whole-series
    /// bible estimate (#655) uses per book.
    #[cfg(test)]
    pub(crate) fn chapters_text(&self, files: &[String]) -> String {
        self.book_render_files(self.project.active_book(), files)
    }

    /// The set of chapter files owned by every book *other* than `b` (#645). Used
    /// to scope a book's slice of the shared incremental state: an entry that no
    /// other book owns belongs to book `b` — including a chapter it just deleted, so
    /// the removal is detectable and prunable rather than lost.
    pub(crate) fn other_books_files(&self, b: usize) -> std::collections::BTreeSet<String> {
        (0..self.project.book_count())
            .filter(|&i| i != b)
            .flat_map(|i| self.project.book_chapters(i))
            .filter(|c| !c.file.is_empty())
            .map(|c| c.file.clone())
            .collect()
    }

    /// Book `b`'s `file → content-digest` map (#339): the active book's active
    /// chapter is read live from the buffer, every other chapter from disk.
    pub(crate) fn book_digests(&self, b: usize) -> ai::incremental::Digests {
        self.project
            .book_chapters(b)
            .iter()
            .filter(|c| !c.file.is_empty())
            .map(|c| {
                (
                    c.file.clone(),
                    ai::incremental::digest(&self.book_chapter_text(b, &c.file)),
                )
            })
            .collect()
    }

    /// One chapter's prose for book `b`, by file (#339): live from the buffer for
    /// the active book's active chapter, from disk otherwise — never holding more
    /// than the editing window in memory.
    fn book_chapter_text(&self, b: usize, file: &str) -> String {
        if b == self.project.active_book() {
            self.project
                .index_of_file(file)
                .map_or_else(String::new, |i| self.chapter_text(i))
        } else {
            self.project.read_chapter_parts(file).0
        }
    }

    /// Book `b`'s chapters serialized under title headings — all of them, or just
    /// those whose file is in `keep` — the request context for a book pass (#339).
    fn book_render(&self, b: usize, keep: Option<&[String]>) -> String {
        let mut out = String::new();
        for c in self.project.book_chapters(b) {
            if keep.is_some_and(|files| !files.iter().any(|f| f == &c.file)) {
                continue;
            }
            out.push_str("# ");
            out.push_str(&c.title);
            out.push_str("\n\n");
            out.push_str(self.book_chapter_text(b, &c.file).trim_end());
            out.push_str("\n\n");
        }
        out
    }

    /// Book `b`'s chapters named by `files`, serialized under title headings — the
    /// scoped context for a single-finding re-check (#608). A thin public wrapper
    /// over [`book_render`], so the re-check reuses the same serializer the bible
    /// passes send rather than re-deriving it.
    pub(super) fn book_render_files(&self, b: usize, files: &[String]) -> String {
        self.book_render(b, Some(files))
    }

    /// Record what the just-applied bible pass was built from (#90), so the next
    /// run only re-analyzes what changed. Best effort — a state-write failure
    /// doesn't undo the pass that already succeeded. A no-op for non-bible passes
    /// (which leave `pending_pass` unset).
    pub(super) fn record_pass_state(&mut self) {
        if let Some((tracker, digests, merge)) = self.pending_pass.take() {
            let today = date::iso(date::today());
            let root = &self.project.root;
            let _ = if let Some(preserve) = merge {
                ai::incremental::State::record_merge(root, &tracker, &today, digests, &preserve)
            } else {
                ai::incremental::State::record(root, &tracker, &today, digests)
            };
        }
    }

    /// The byte length of the context a costed pass will actually send — for the
    /// cost estimate (#90). The whole manuscript for most passes, but only the
    /// bible + changed chapters for an incremental tracker pass, and zero when its
    /// bible is already current (so an up-to-date pass never prompts a confirm).
    pub(crate) fn pass_context_len(&self, action: AiAction, force: bool) -> usize {
        self.book_pass_context_len(action, self.project.active_book(), force)
    }

    /// The byte length of the context a costed pass will send for a specific
    /// `book` — the per-book form of [`pass_context_len`], so a whole-series bible
    /// estimate (#655) can sum every book's scoped size without assuming the active
    /// book. A non-bible pass ignores `book` (it always sends the active
    /// manuscript). A bible pass is scoped exactly as it will run: the whole book on
    /// a full rebuild (or forced), the bible + changed chapters incrementally, or
    /// zero when that book's bible is already current.
    pub(crate) fn book_pass_context_len(
        &self,
        action: AiAction,
        book: usize,
        force: bool,
    ) -> usize {
        let Some(t) = bible_tracker(action) else {
            return self.manuscript_text().len();
        };
        if force {
            return self.book_render(book, None).len(); // a forced rebuild sends the whole book
        }
        let (scope, prior, _) = self.plan_bible_pass(&t, book);
        match scope {
            ai::incremental::Scope::UpToDate => 0,
            ai::incremental::Scope::Full => self.book_render(book, None).len(),
            ai::incremental::Scope::Incremental(changed) => {
                prior.len() + self.book_render(book, Some(&changed)).len()
            }
        }
    }

    /// Whether a costed pass for `book` will actually run via the Batch API (half
    /// price) — so its cost estimate can quote the discounted figure the confirm
    /// gates (#691). This must track the run-time decision in [`run_bible_tracker`]
    /// exactly: a full rebuild batches, but only on the Claude backend and only
    /// when it rebuilds the whole book from scratch (a first build, or a standalone
    /// / single-book project). A series that revises its shared bible, an
    /// incremental pass, an up-to-date pass, and every non-bible pass all run at the
    /// standard rate.
    pub(crate) fn book_pass_batches(&self, action: AiAction, book: usize, force: bool) -> bool {
        let Some(t) = bible_tracker(action) else {
            return false; // non-bible passes never batch
        };
        if self.ai.backend() != ai::BackendKind::Claude {
            return false; // only the Claude backend has a batch path
        }
        let (scope, prior, _) = self.plan_bible_pass(&t, book);
        // `force` (the palette's `R`) rebuilds in full regardless of the delta.
        let full = force || matches!(scope, ai::incremental::Scope::Full);
        // A series with an existing bible *revises* rather than rebuilds, so its
        // "full" pass runs at standard rate; only a from-scratch rebuild batches.
        let from_scratch = prior.trim().is_empty() || self.project.book_count() <= 1;
        full && from_scratch
    }

    /// Whether the active book's costed pass will run batched — the active-book
    /// form of [`book_pass_batches`], mirroring [`pass_context_len`].
    pub(crate) fn pass_batches(&self, action: AiAction, force: bool) -> bool {
        self.book_pass_batches(action, self.project.active_book(), force)
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
    pub(super) fn backend_spec(&self) -> Result<BackendSpec, ai::AiError> {
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
    pub(super) fn assistant_backend(&self) -> Result<BackendSpec, ai::AiError> {
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
                match outcome {
                    Outcome::Answer(r) => self.apply_qa_result(r),
                    Outcome::Characters(r) => self.apply_characters(r),
                    Outcome::Continuity(r) => self.apply_continuity(r),
                    Outcome::Setting(r) => self.apply_setting(r),
                    Outcome::Plot(r) => self.apply_plot(r),
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
                    Outcome::Config(r) => self.apply_generated_config(r),
                    Outcome::Import(r) => self.apply_import(r),
                    Outcome::Thesaurus(r) => self.apply_thesaurus(r),
                    Outcome::NameGen(r) => self.apply_namegen(r),
                    Outcome::Editorial(r) => self.apply_editorial(r),
                    Outcome::Recheck(r) => self.apply_recheck(r),
                    Outcome::Error(e) => self.message = Some(format!("AI: {e}")),
                }
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
            Poll::Died => {
                // Resume the assistant if it was waiting on this (now dead) pass (#392).
                if let Some(summary) =
                    self.assistant_pass_summary(&Outcome::Error(ai::AiError::Network {
                        status: None,
                        message: "request failed".into(),
                    }))
                {
                    self.resume_assistant_pass(summary);
                }
                self.message = Some("AI: request failed".into());
                self.advance_run(Some("request failed".into()));
            }
        }
        // Fire a setup wizard's deferred setting draft once the worker is free (#282).
        self.pump_wizard_drafts();
    }

    /// The whole manuscript as plain text for an AI request — the active book's
    /// chapters under title headings. The same serializer the bible passes use
    /// (#432): `book_render(active_book, None)` over `book_chapters(active_book)`
    /// (the live, band-ordered manifest list) resolves each chapter's body through
    /// `chapter_text`, so the active chapter's unsaved edits are included exactly
    /// as before.
    pub(crate) fn manuscript_text(&self) -> String {
        self.book_render(self.project.active_book(), None)
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

#[cfg(all(test, feature = "local"))]
mod local_model_files_tests {
    //! Vets the download / status / remove round-trip (#382) against the real
    //! `hf-hub` cache. Ignored: needs network + disk. Uses the smallest model
    //! (~400 MB). Run with:
    //! `cargo test --features local download_status_remove -- --ignored --nocapture`
    use std::sync::Arc;

    use super::super::models::DownloadProgress;
    use crate::ai::model_select::MODELS;

    #[test]
    #[ignore = "integration: needs network / a downloaded model; not hermetic"]
    fn download_status_remove_round_trip() {
        let model = &MODELS[0]; // 0.5B — keep the download small
        super::remove_model_files(model).expect("clean start"); // ignore prior state
        assert!(!super::model_downloaded(model), "absent before download");

        // The real fetch drives the shared progress counter; a live download ends
        // with `done == total` (both files fully fetched).
        let progress = Arc::new(DownloadProgress::new());
        super::download_model(model, &progress).expect("download");
        let (done, total) = progress.snapshot();
        assert!(
            total > 0 && done == total,
            "progress reached 100%: {done}/{total}"
        );
        assert!(super::model_downloaded(model), "present after download");

        super::remove_model_files(model).expect("remove");
        assert!(!super::model_downloaded(model), "absent after remove");
    }
}
