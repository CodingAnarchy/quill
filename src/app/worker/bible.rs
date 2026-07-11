//! Persistent-bible tracker passes: the [`BibleTracker`] descriptor mapping a
//! costed AI action to its request builders + parse, incremental-scope planning
//! (#90), and the per-book manuscript serialization + digests the passes send.
//! Split out of `worker.rs` (#839). A pure move — no behavior change.

use super::super::*;

use crate::mode::AiAction;

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
pub(in crate::app) struct BibleTracker {
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
pub(in crate::app) fn bible_tracker(action: AiAction) -> Option<BibleTracker> {
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
        AiAction::Theme => Some(BibleTracker {
            key,
            source: BibleSource::File("theme.md"),
            full_msg: "⟳ Tracking themes & motifs…",
            incr_msg: "⟳ Updating the theme tracker from changed chapters…",
            full: ai_theme::build_request,
            incr: ai_theme::build_incremental_request,
            finish: |r| Outcome::Theme(r.and_then(|resp| ai_theme::parse(&resp.text))),
        }),
        _ => None,
    }
}

impl App {
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
    pub(in crate::app) fn plan_bible_pass_for_test(
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
    pub(in crate::app) fn run_bible_tracker(&mut self, t: BibleTracker, book: usize, force: bool) {
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
        let model = self.ai.model.clone();
        // Offline-oversized full rebuild (#849): if this pass is pinned `offline`,
        // is a from-scratch full rebuild, and doesn't fit the on-device window,
        // rebuild it by *folding* — walk the chapters in order, revising the bible
        // one window-sized chunk at a time. Schema-equivalent to a one-shot rebuild;
        // the model never sees more than a window. A hard limit (the running bible
        // itself outgrows the window) falls through to the graceful refusal.
        let full_from_scratch =
            matches!(scope, ai::incremental::Scope::Full) && (prior.trim().is_empty() || !series);
        if full_from_scratch && self.offline_oversized(book) {
            self.run_folded_rebuild(&t, book, &model, current, series);
            return;
        }
        let (req, base_msg) = match scope {
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
                ((t.full)(self.book_render(book, None), &model), t.full_msg)
            }
            ai::incremental::Scope::Full => {
                // Series, bible exists: revise it with all of this book's chapters.
                (
                    (t.incr)(&prior, &self.book_render(book, None), &model),
                    t.full_msg,
                )
            }
            ai::incremental::Scope::Incremental(changed) => {
                // Retrieval-backed incremental pass (#929): when a usable embedding
                // model is on disk, fold the prior passages the changed chapters bear
                // on into the revise context (off the worker thread, since embedding
                // is CPU-bound), so the pass sees the scenes that matter — where a
                // character/place last appeared — instead of being blind to
                // everything outside the diff. Falls back to the plain
                // changed-chapters-only context when embeddings aren't available.
                self.spawn_incremental_pass(&t, book, &prior, &model, changed, current, series);
                return;
            }
        };
        // Snapshot the manuscript state now; recorded once the pass applies cleanly.
        // A series merges the book's digests into the state (carrying the other
        // books' files to preserve, so this book's deleted chapters get pruned —
        // #645); a standalone replaces. Snapshot the preserve set now too, since the
        // manifest can change before the async pass lands.
        let merge = series.then(|| self.other_books_files(book));
        self.pending_pass = Some((t.key.to_string(), current, merge));
        // A bible refresh is a request the author kicks off and watches, so every
        // pass — full rebuild included — runs synchronously through the standard
        // Messages API: bounded latency, seconds-to-minutes even over a whole
        // manuscript. Full rebuilds used to run through the Message Batches API for
        // its 50% saving (#176), but that endpoint is asynchronous with no latency
        // SLA (a processing window of up to 24 hours), so an interactive pass would
        // either false-time-out under the no-hang guardrail or block for minutes on
        // a batch still legitimately processing. The batch machinery
        // (`Backend::complete_batched`, the `cost::estimate` discount) is retained
        // for a future opt-in background "update" that can tolerate the latency (#950).
        self.spawn_request(base_msg, req, false, t.finish);
    }

    /// Build the full-manuscript rebuild request for a background batch update
    /// (#950), plus the manuscript digests it's built against (the staleness
    /// baseline) and whether this is a series book (so the reconcile merges rather
    /// than replaces the recorded state). Returns `None` when there's no tracker for
    /// the action or the book has nothing to send (an empty manuscript). Always a
    /// full rebuild: the batch path is the "full-from-scratch rebuild on the Claude
    /// backend" the removed `book_pass_batches` eligibility covered — the interactive
    /// incremental path (#655/#955) stays synchronous and unbatched.
    ///
    /// In a series with an existing shared bible, a book's "full" pass *revises* the
    /// shared bible with that book's chapters (never discarding the other books),
    /// exactly as `run_bible_tracker`'s `Scope::Full` arm does; a first build or a
    /// standalone novel rebuilds from the whole book.
    pub(in crate::app) fn batch_full_request(
        &self,
        action: AiAction,
        book: usize,
    ) -> Option<(ai::Request, ai::incremental::Digests, bool)> {
        let t = bible_tracker(action)?;
        let manuscript = self.book_render(book, None);
        if manuscript.trim().is_empty() {
            return None; // an empty book has nothing to rebuild
        }
        let current = self.book_digests(book);
        let series = self.project.book_count() > 1;
        let model = self.ai.model.clone();
        let prior = match t.source {
            BibleSource::Dir(dir) => bible::raw_dir(&self.project.root, dir),
            BibleSource::File(file) => bible::raw_file(&self.project.root, file),
        };
        // Series with an existing bible → revise it with the whole book; else a
        // from-scratch rebuild of the whole book (mirrors the `Scope::Full` arms).
        let req = if series && !prior.trim().is_empty() {
            (t.incr)(&prior, &manuscript, &model)
        } else {
            (t.full)(manuscript, &model)
        };
        Some((req, current, series))
    }

    /// The tracker `finish` that turns a completed batch response into the matching
    /// [`Outcome`] (#950), so the reconcile applies a batch rebuild through the same
    /// per-feature apply path the interactive pass uses. `None` for a non-bible
    /// action (which never reaches the batch update).
    pub(in crate::app) fn bible_finish(
        action: AiAction,
    ) -> Option<fn(Result<ai::Response, ai::AiError>) -> Outcome> {
        bible_tracker(action).map(|t| t.finish)
    }

    /// Spawn a retrieval-backed incremental bible pass (#929). Assembles everything
    /// off-thread retrieval needs on the UI thread — the changed chapters' rendered
    /// text (the revise material and the retrieval query), the whole-book index
    /// input, the index dir, and the embedding model (`None` when none is on disk) —
    /// then, off the worker thread, retrieves the prior passages the changed chapters
    /// bear on and folds them into the revise context via
    /// [`incremental::with_prior_passages`](ai::incremental::with_prior_passages)
    /// before building + completing the request. When retrieval isn't available or
    /// turns up nothing, the context is the changed chapters alone — the exact prior
    /// incremental request — so the pass degrades gracefully. The request shape,
    /// schema, and `t.finish` parse are identical either way; retrieval changes only
    /// *what prior context* the revise sees.
    #[allow(clippy::too_many_arguments)] // one call site; a struct would only obscure the flow
    fn spawn_incremental_pass(
        &mut self,
        t: &BibleTracker,
        book: usize,
        prior: &str,
        model: &str,
        changed: Vec<String>,
        current: ai::incremental::Digests,
        series: bool,
    ) {
        // The changed chapters, rendered once: the material the pass revises from and
        // the query retrieval ranks the prior passages against.
        let changed_text = self.book_render(book, Some(&changed));
        // Retrieval inputs, moved into the worker: the whole book to (re-)index, the
        // index dir, and the embedding model — `None` keeps the pass on the plain
        // incremental context (#929).
        let embed_model = self.ask_embed_model();
        let chapters = embed_model.is_some().then(|| self.ask_index_chapters());
        let root = self.project.root.clone();
        let prior = prior.to_string();
        let model = model.to_string();
        let incr = t.incr;
        let finish = t.finish;

        let merge = series.then(|| self.other_books_files(book));
        self.pending_pass = Some((t.key.to_string(), current, merge));
        let message = t.incr_msg.to_string();
        self.spawn_outcome(&message, move |backend| {
            // Retrieve the relevant prior passages (best effort); fall back to the
            // changed chapters alone when embeddings aren't available or find nothing.
            let retrieved = match (embed_model, chapters) {
                (Some(m), Some(chapters)) => {
                    super::super::assist::retrieve::retrieve_prior_context(
                        &root,
                        &chapters,
                        m,
                        &changed,
                        &changed_text,
                    )
                    .unwrap_or_default()
                }
                _ => String::new(),
            };
            let context = ai::incremental::with_prior_passages(&retrieved, &changed_text);
            let req = incr(&prior, &context, &model);
            finish(backend.complete(&req))
        });
    }

    /// Whether a full rebuild of `action` for `book` is pinned `offline` yet too
    /// big for the on-device window (#849) — the condition that turns a from-scratch
    /// rebuild into a fold. Uses the same `context_len / 4` estimate and
    /// `local_fits` window math the router consults, so the fold triggers exactly
    /// when the router would otherwise refuse.
    fn offline_oversized(&self, book: usize) -> bool {
        if !self.ai.offline {
            return false;
        }
        let est = (self.book_render(book, None).len() / 4) as u32;
        !self.local_fits(est)
    }

    /// The window a folded rebuild targets (#849) — the largest on-device model's
    /// context, the same bound `local_fits` uses.
    fn fold_window(&self) -> u32 {
        ai::model_select::local_window(&self.ai.local_models)
    }

    /// The active-book chapters in manuscript order with their estimated token
    /// sizes (#849), for the fold planner. Each chapter is rendered exactly as the
    /// full pass renders it (title heading + body), so a fold's chunks reproduce the
    /// one-shot serialization on chapter seams.
    fn fold_chapters(&self, book: usize) -> Vec<(String, u32)> {
        self.project
            .book_chapters(book)
            .iter()
            .filter(|c| !c.file.is_empty())
            .map(|c| {
                let rendered = self.book_render_files(book, std::slice::from_ref(&c.file));
                let tokens = (rendered.len() / 4) as u32;
                (rendered, tokens)
            })
            .collect()
    }

    /// Rebuild a bible for `book` by folding (#849): seed an empty bible, then walk
    /// the chapters in manuscript order, each step revising the running bible with
    /// as many whole chapters as fit the window. The running bible is carried as the
    /// prior JSON between steps, so the final response is schema-equivalent to a
    /// one-shot rebuild and `t.finish` parses it unchanged. The whole fold runs in
    /// one worker spawn (each step is a `complete` on the held backend); a hard limit
    /// — the running bible outgrows the window — surfaces the graceful refusal.
    #[allow(clippy::needless_pass_by_value)] // consumes current: moved into pending_pass
    fn run_folded_rebuild(
        &mut self,
        t: &BibleTracker,
        book: usize,
        model: &str,
        current: ai::incremental::Digests,
        series: bool,
    ) {
        let chapters = self.fold_chapters(book);
        let window = self.fold_window();
        let model = model.to_string();
        let incr = t.incr;
        let finish = t.finish;
        // `est(system + running bible + MAX_OUTPUT_TOKENS)`: the request overhead
        // besides the chapter chunk. The system prompt is bounded and small; the
        // running bible grows, so it's measured per step inside the loop.
        let system_tokens = fold_request_system_tokens(incr, &model);
        let out_tokens = ai::MAX_OUTPUT_TOKENS;

        let merge = series.then(|| self.other_books_files(book));
        self.pending_pass = Some((t.key.to_string(), current, merge));
        let message = format!("{} (folding on-device — chapter by chapter)", t.full_msg);
        // Report per-pass chapter progress (#923): the fold walks the chapters one
        // chunk at a time, so it can publish "chapter n/total" into the shared cell
        // the pass-queue run's status reads. Reset before the pass, cleared after.
        let progress = self.pass_progress.clone();
        let total = chapters.len();
        self.set_pass_progress(Some((0, total)));
        self.spawn_outcome(&message, move |backend| {
            let out = fold_rebuild(
                backend,
                &chapters,
                window,
                system_tokens,
                out_tokens,
                &model,
                incr,
                finish,
                &progress,
                total,
            );
            if let Ok(mut slot) = progress.lock() {
                *slot = None; // the pass is done; drop the per-chapter counter
            }
            out
        });
    }

    /// Set (or clear) the in-flight pass's per-chapter progress counter (#923).
    pub(in crate::app) fn set_pass_progress(&self, progress: Option<(usize, usize)>) {
        if let Ok(mut slot) = self.pass_progress.lock() {
            *slot = progress;
        }
    }

    /// The in-flight pass's per-chapter progress `(done, total)` (#923), or `None`
    /// between fold steps and for one-shot passes — the pass-queue run's status
    /// reads it to show "continuity: chapter 5/12".
    pub(in crate::app) fn pass_progress(&self) -> Option<(usize, usize)> {
        self.pass_progress.lock().ok().and_then(|slot| *slot)
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
    pub(in crate::app) fn book_render_files(&self, b: usize, files: &[String]) -> String {
        self.book_render(b, Some(files))
    }

    /// Record what the just-applied bible pass was built from (#90), so the next
    /// run only re-analyzes what changed. Best effort — a state-write failure
    /// doesn't undo the pass that already succeeded. A no-op for non-bible passes
    /// (which leave `pending_pass` unset).
    pub(in crate::app) fn record_pass_state(&mut self) {
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
    ///
    /// An on-demand scope-to-selection pass (#1054) sends only its prior bible plus
    /// the scoped chapters, so the estimate reflects the reduced scope — smaller
    /// context, lower cost — matching exactly what `analyze_scoped` will send.
    pub(crate) fn pass_context_len(&self, action: AiAction, force: bool) -> usize {
        if let Some(files) = self.scoped_pass_context(action) {
            return files;
        }
        self.book_pass_context_len(action, self.project.active_book(), force)
    }

    /// The context length a captured on-demand scoped pass (#1054) will send for
    /// `action`, or `None` when no selection scope is active or the action isn't a
    /// scopeable bible pass (so the caller falls back to the whole-book estimate).
    /// Mirrors `recheck_request`'s scoped context — the prior bible plus the scoped
    /// chapters — so the pre-flight estimate can't drift from the real request.
    fn scoped_pass_context(&self, action: AiAction) -> Option<usize> {
        let files = self.pass_scope.as_ref()?;
        let t = bible_tracker(action)?;
        let book = self.project.active_book();
        let prior = match t.source {
            BibleSource::Dir(dir) => bible::raw_dir(&self.project.root, dir),
            BibleSource::File(file) => bible::raw_file(&self.project.root, file),
        };
        Some(prior.len() + self.book_render_files(book, files).len())
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

/// Estimate the fixed request overhead a revise pass carries besides the chapter
/// chunk and the running bible (#849): the system prompt, roughly, via the same
/// `bytes / 4` estimate the router uses. Built with an empty prior + empty chunk so
/// only the framing is measured; the running bible is added per step.
fn fold_request_system_tokens(incr: fn(&str, &str, &str) -> ai::Request, model: &str) -> u32 {
    let framing = incr("", "", model);
    let bytes = framing.system.as_deref().map_or(0, str::len) + framing.user.len();
    (bytes / 4) as u32
}

/// Drive a full-by-folding rebuild on the worker thread (#849): seed an empty
/// bible, then fold the chapters in order, revising the running bible with each
/// window-sized chunk. The running bible is carried as the prior JSON between
/// steps; the final response is returned through `finish` (schema-equivalent to a
/// one-shot rebuild). A hard limit — even one chapter can't fit beside the running
/// bible — becomes a graceful `NotConfigured` error the caller surfaces.
#[allow(clippy::too_many_arguments)] // one call site; grouping into a struct would only obscure it
fn fold_rebuild(
    backend: &dyn Backend,
    chapters: &[(String, u32)],
    window: u32,
    system_tokens: u32,
    out_tokens: u32,
    model: &str,
    incr: fn(&str, &str, &str) -> ai::Request,
    finish: fn(Result<ai::Response, ai::AiError>) -> Outcome,
    progress: &Arc<Mutex<Option<(usize, usize)>>>,
    total: usize,
) -> Outcome {
    // Position-indexed sizes so the planner packs whole chapters on seams; the
    // "file" id is just the index, since we drain by count.
    let ids: Vec<String> = (0..chapters.len()).map(|i| i.to_string()).collect();
    let mut remaining: Vec<ai::incremental::ChapterSize> = ids
        .iter()
        .zip(chapters)
        .map(|(id, (_, tokens))| ai::incremental::ChapterSize {
            file: id.as_str(),
            tokens: *tokens,
        })
        .collect();

    let mut running_bible = String::new();
    let mut next = 0usize; // index of the next chapter to fold
    let mut last: Option<ai::Response> = None;
    while !remaining.is_empty() {
        let bible_tokens = (running_bible.len() / 4) as u32;
        let overhead = system_tokens
            .saturating_add(bible_tokens)
            .saturating_add(out_tokens);
        let count = match ai::incremental::next_fold_chunk(&remaining, window, overhead) {
            ai::incremental::FoldStep::Chunk(files) => files.len(),
            ai::incremental::FoldStep::HardLimit => {
                return finish(Err(ai::AiError::NotConfigured(
                    ai::router::offline_too_big_message(bible_tokens),
                )));
            }
        };
        // The chunk's text: the chosen chapters, concatenated in order.
        let chunk: String = chapters[next..next + count]
            .iter()
            .map(|(text, _)| text.as_str())
            .collect();
        let resp = match backend.complete(&incr(&running_bible, &chunk, model)) {
            Ok(r) => r,
            Err(e) => return finish(Err(e)),
        };
        running_bible.clone_from(&resp.text);
        last = Some(resp);
        remaining.drain(0..count);
        next += count;
        // Publish per-pass chapter progress (#923): `next` is now how many whole
        // chapters this pass has folded in, out of `total`.
        if let Ok(mut slot) = progress.lock() {
            *slot = Some((next, total));
        }
    }
    // The last step's response is the complete folded bible (or an empty
    // manuscript, which yields no chapters and so no fold — an empty result).
    finish(last.ok_or_else(|| {
        ai::AiError::NotConfigured("nothing to analyze: the manuscript is empty".into())
    }))
}
