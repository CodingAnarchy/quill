//! Single-finding re-check (#608): re-run one finding's originating pass scoped
//! to just its chapters, then clear the finding if the fix held or keep it if
//! it's still flagged — so verifying one fix doesn't cost a whole-manuscript
//! sweep. Stays within the AI boundary: it re-runs analysis on a subset and
//! updates a flag; it writes no prose and no bible file.

use super::super::*;

use super::{recheck_parser, RecheckRequest};
use crate::ai::naming;
use crate::ai::scaffold;
use crate::ai::style;
use crate::ai::voice;
use crate::mode::AiAction;

/// One scoped re-check to run in a re-check-all batch (#830): the built request
/// plus the pass and book its response is parsed under (so the worker can map the
/// response back to UI findings that share the original's content key).
type GroupedRecheck = (RecheckRequest, AiAction, Option<usize>);

impl App {
    /// The on-demand pass scope from the current editor selection (#1054): the
    /// chapter files the Visual selection covers, or the cursor's chapter when
    /// nothing is highlighted (a caret in a scene means "this scene"). `None` when
    /// there's nothing to scope to — no active book chapters, or a selection that
    /// somehow lands outside every loaded chapter — so the pass falls back to the
    /// whole book. Reads the *live* selection, so it must be called before a mode
    /// change (e.g. opening the palette) clears it.
    pub(crate) fn selection_pass_scope(&self) -> Option<Vec<String>> {
        // The selection byte range in the buffer the author is editing: the Visual
        // range when selecting, else the caret (a zero-width point).
        let (start, end) = if let Mode::Visual { anchor, linewise } = self.mode {
            self.selection_bytes(anchor, linewise)
        } else {
            let c = self.edit_buffer().cursor_byte();
            (c, c)
        };
        // The loaded chapters' byte spans, in the same buffer coordinates the
        // selection uses. In the continuous view that's every windowed chapter; in
        // the single-chapter view it's the one active chapter spanning the buffer.
        let spans: Vec<(String, usize, usize)> = self.manuscript.as_ref().map_or_else(
            || {
                vec![(
                    self.project.active_file().to_string(),
                    0,
                    self.edit_buffer().len_bytes(),
                )]
            },
            |m| {
                m.segment_spans()
                    .into_iter()
                    .map(|(idx, s, e)| (self.project.chapter_file(idx).to_string(), s, e))
                    .filter(|(f, _, _)| !f.is_empty())
                    .collect()
            },
        );
        let refs: Vec<ai::incremental::ChapterSpan> = spans
            .iter()
            .map(|(f, s, e)| ai::incremental::ChapterSpan {
                file: f,
                start: *s,
                end: *e,
            })
            .collect();
        let files = ai::incremental::chapters_in_span(&refs, start, end);
        (!files.is_empty()).then_some(files)
    }

    /// Run a continuity/setting pass scoped on demand to the captured selection /
    /// chapter range (#1054): re-verify just those chapters and surface the same
    /// kind of findings the full pass produces. Narrows the input only — the output
    /// contract is unchanged and nothing is written (no bible file, no prose). A
    /// no-op with a status for an action that can't be scoped this way, or when
    /// there's nothing scoped. The scope is *taken* here, so a later run without a
    /// selection reverts to the whole book.
    pub(crate) fn analyze_scoped(&mut self, action: AiAction) {
        if self.ai_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        let Some(files) = self.pass_scope.take() else {
            return;
        };
        let book = Some(self.project.active_book());
        let Some(req) = self.recheck_request(action, book, &files) else {
            self.message = Some("This pass can't be scoped to a selection".into());
            return;
        };
        self.spawn_outcome("⟳ Checking the selected scene…", move |backend| {
            Outcome::ScopedPass(
                backend
                    .complete(&req.request)
                    .map(|r| (req.to_findings)(&r.text, action, book)),
            )
        });
    }

    /// The scoped request the captured on-demand scope (#1054) would send for
    /// `action`, plus the parser that turns its response into UI findings — for
    /// tests to assert the request carries only the scoped chapters and to drive
    /// `apply_scoped_pass` end-to-end through a mock backend, without a worker
    /// thread or a live backend. `None` when no scope is captured or the action
    /// can't be scoped this way. Returns the request and parser as a tuple (not the
    /// crate-private `RecheckRequest`) so the test module outside `assist` can use it.
    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    pub(crate) fn scoped_pass_request(
        &self,
        action: AiAction,
    ) -> Option<(
        ai::Request,
        fn(&str, AiAction, Option<usize>) -> Vec<Finding>,
    )> {
        let files = self.pass_scope.as_ref()?;
        let book = Some(self.project.active_book());
        self.recheck_request(action, book, files)
            .map(|r| (r.request, r.to_findings))
    }

    /// Apply a completed on-demand scoped pass (#1054): surface the fresh findings
    /// the scoped run produced through the shared triage / findings flow — the same
    /// kind of findings a full pass yields, limited to the scoped input. Unlike a
    /// re-check (#608) this isn't reconciled against one flagged finding; it's a
    /// fresh, narrowed analysis, so its findings list like any pass's.
    pub(crate) fn apply_scoped_pass(&mut self, result: Result<Vec<Finding>, ai::AiError>) {
        let Some(findings) = self.ok_or_report(result) else {
            return;
        };
        self.show_findings(findings);
    }

    /// Re-check the finding at `index` in the open findings overlay (#608): re-run
    /// its originating pass scoped to just the finding's chapters, then clear it if
    /// it's gone or keep it if it's still flagged — so verifying one fix doesn't
    /// cost a whole-manuscript sweep. A no-op (with a status) for a finding that
    /// carries no re-runnable pass (a Q&A citation, or one restored from disk) or
    /// no chapters to scope to. Stays within the AI boundary: it re-runs analysis
    /// on a subset and updates a flag; it writes no prose and no bible file.
    pub(crate) fn recheck_finding(&mut self, index: usize) {
        if self.ai_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        let Some(finding) = self.finding_at(index).cloned() else {
            return;
        };
        let Some(action) = finding.action else {
            self.message = Some("Nothing to re-check for this finding".into());
            return;
        };
        if finding.chapters.is_empty() {
            self.message = Some("This finding has no chapter to re-check".into());
            return;
        }
        let files = self.finding_chapter_files(&finding);
        if files.is_empty() {
            self.message = Some("Couldn't locate this finding's chapters".into());
            return;
        }
        let Some(req) = self.recheck_request(action, finding.book, &files) else {
            self.message = Some("Nothing to re-check for this finding".into());
            return;
        };
        // The book a re-produced finding is attributed to, so its content key
        // matches the finding being re-checked (titles can repeat across books).
        let book = finding.book;
        self.spawn_outcome("⟳ Re-checking this finding…", move |backend| {
            Outcome::Recheck(
                backend
                    .complete(&req.request)
                    .map(|r| (req.to_findings)(&r.text, action, book)),
            )
        });
        // Track which finding is in flight so [`apply_recheck`] can update just it.
        // Set after the spawn so a not-ready gate (which returns early inside
        // `spawn_outcome`) doesn't leave a stale pending recheck.
        if self.ai_busy() {
            self.pending_recheck = Some(finding);
        }
    }

    /// Re-check *every* open finding in one pass (#830): re-run each open
    /// finding's originating pass scoped to its chapters, then reconcile — clear
    /// the findings the fresh runs no longer produce and keep the rest. Groups the
    /// open findings by pass + chapters so a pass covering several findings in the
    /// same chapters runs once, not per finding, and drives all the grouped
    /// requests through a single worker batch. A finding with no re-runnable pass
    /// (`action: None` — a Q&A citation, or one restored from the durable store,
    /// which records content, not the originating pass) is skipped gracefully: it
    /// stays in triage untouched, since nothing scoped can be re-run for it. Stays
    /// within the AI boundary: it re-runs analysis on subsets and updates flags; it
    /// writes no prose and no bible file.
    pub(crate) fn recheck_all(&mut self) {
        if self.ai_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        if !matches!(self.mode, Mode::Findings(_)) {
            return;
        }
        let (requests, verifying) = self.recheck_all_plan();
        if verifying.is_empty() {
            self.message = Some("No open findings to re-check".into());
            return;
        }
        if requests.is_empty() {
            self.message = Some("Nothing re-runnable to re-check".into());
            return;
        }
        // Run every grouped request on the worker thread and gather the combined
        // fresh findings; [`apply_recheck_all`] reconciles each verifying finding
        // against that set (present → still flagged, absent → cleared).
        self.spawn_outcome("⟳ Re-checking all open findings…", move |backend| {
            let mut fresh = Vec::new();
            for (req, action, book) in requests {
                match backend.complete(&req.request) {
                    Ok(r) => fresh.extend((req.to_findings)(&r.text, action, book)),
                    Err(e) => return Outcome::RecheckAll(Err(e)),
                }
            }
            Outcome::RecheckAll(Ok(fresh))
        });
        // Track the findings in flight so [`apply_recheck_all`] reconciles exactly
        // them. Set after the spawn so a not-ready gate (which returns early inside
        // `spawn_outcome`) doesn't leave a stale pending re-check.
        if self.ai_busy() {
            self.pending_recheck_all = Some(verifying);
        }
    }

    /// Plan a re-check-all over the open findings overlay (#830): the deduplicated
    /// scoped requests to run, and the findings they verify. One request per
    /// distinct (pass, book, chapters) group, so several findings sharing a pass +
    /// chapters cost a single re-run. `verifying` is every open finding that
    /// carries a re-runnable pass and locatable chapters — a `None`-action or
    /// unlocatable finding is skipped (left in triage, never re-run). Split out so
    /// the spawn path and the test helper share the exact grouping.
    fn recheck_all_plan(&self) -> (Vec<GroupedRecheck>, Vec<Finding>) {
        let open: Vec<Finding> = match &self.mode {
            Mode::Findings(f) => f.items.clone(),
            _ => return (Vec::new(), Vec::new()),
        };
        let mut requests = Vec::new();
        let mut seen: Vec<(AiAction, Option<usize>, Vec<String>)> = Vec::new();
        let mut verifying = Vec::new();
        for finding in open {
            let Some(action) = finding.action else {
                continue; // restored / non-pass finding: nothing to re-run
            };
            let files = self.finding_chapter_files(&finding);
            if files.is_empty() {
                continue; // no locatable chapter to scope to
            }
            verifying.push(finding.clone());
            let group = (action, finding.book, files.clone());
            if seen.contains(&group) {
                continue; // this pass + chapters is already queued
            }
            if let Some(req) = self.recheck_request(action, finding.book, &files) {
                requests.push((req, action, finding.book));
                seen.push(group);
            }
        }
        (requests, verifying)
    }

    /// The scoped requests a re-check-all would send over the open overlay (#830),
    /// for tests to assert each is scoped to its finding's chapters and that the
    /// grouping dedups — without spawning a worker thread or a live backend.
    #[cfg(test)]
    pub(crate) fn recheck_all_requests(&self) -> Vec<ai::Request> {
        self.recheck_all_plan()
            .0
            .into_iter()
            .map(|(r, _, _)| r.request)
            .collect()
    }

    /// Apply a completed re-check-all (#830). `result` is the combined fresh
    /// findings from re-running every open finding's pass over its chapters. For
    /// each finding we were verifying: if the fresh runs no longer produce it, the
    /// fix held — clear it from triage and the overlay; if it's still among them,
    /// keep it. Newly-surfaced findings enter durable triage silently (this is a
    /// verification sweep, not a fresh full pass). Reports how many cleared.
    pub(crate) fn apply_recheck_all(&mut self, result: Result<Vec<Finding>, ai::AiError>) {
        let Some(verifying) = self.pending_recheck_all.take() else {
            return;
        };
        let Some(fresh) = self.ok_or_report(result) else {
            return;
        };
        // Any newly-surfaced finding in these chapters enters durable triage.
        self.accumulate_triage(&fresh);
        let mut cleared = 0;
        for finding in &verifying {
            if !fresh.contains(finding) {
                self.resolve_finding(finding);
                self.remove_finding_from_overlay(finding);
                cleared += 1;
            }
        }
        let kept = verifying.len() - cleared;
        self.message = Some(format!(
            "Re-checked {} — {cleared} cleared, {kept} still flagged",
            verifying.len()
        ));
    }

    /// The chapter *files* (project-relative) for a finding's chapter titles,
    /// within the finding's book (#608). Titles can repeat across books, so the
    /// lookup is scoped to the finding's own book. Unknown titles are dropped.
    fn finding_chapter_files(&self, finding: &Finding) -> Vec<String> {
        let book = self.finding_book(finding);
        let chapters = self.project.book_chapters(book);
        finding
            .chapters
            .iter()
            .filter_map(|title| {
                chapters
                    .iter()
                    .find(|c| &c.title == title)
                    .map(|c| c.file.clone())
            })
            .collect()
    }

    /// The book index a finding belongs to — its own book, or the active book for a
    /// finding not attributed to one (#608).
    fn finding_book(&self, finding: &Finding) -> usize {
        finding.book.unwrap_or_else(|| self.project.active_book())
    }

    /// Build the scoped re-check request for `action` over `files` (the finding's
    /// chapters), plus a parser turning the response into the UI findings that pass
    /// now produces (#608). Bible passes revise from the existing bible + the
    /// changed chapters (the incremental path); findings-only passes analyze just
    /// those chapters. `None` for an action that produces no re-runnable findings.
    fn recheck_request(
        &self,
        action: AiAction,
        book: Option<usize>,
        files: &[String],
    ) -> Option<RecheckRequest> {
        let b = book.unwrap_or_else(|| self.project.active_book());
        let scoped = self.book_render_files(b, files);
        let model = self.ai.model.clone();
        let root = &self.project.root;
        let request = match action {
            AiAction::Continuity => {
                let prior = bible::raw_file(root, "timeline.md");
                continuity::build_incremental_request(&prior, &scoped, &model)
            }
            AiAction::Setting => {
                let prior = bible::raw_file(root, "setting.md");
                setting::build_incremental_request(&prior, &scoped, &model)
            }
            AiAction::Characters => {
                let prior = bible::raw_dir(root, "characters");
                character::build_incremental_request(&prior, &scoped, &model)
            }
            AiAction::Plot => {
                let prior = bible::raw_file(root, "plot.md");
                plot::build_incremental_request(&prior, &scoped, &model)
            }
            AiAction::Theme => {
                let prior = bible::raw_file(root, "theme.md");
                ai_theme::build_incremental_request(&prior, &scoped, &model)
            }
            AiAction::Style => style::build_request(scoped, &model),
            AiAction::Scaffold => scaffold::build_request(scoped, &model),
            AiAction::Editorial => ai::editorial::build_request(scoped, &model),
            AiAction::Foreshadow => ai::foreshadow::build_request(scoped, &model),
            AiAction::Naming => {
                let entities = self.known_entities();
                naming::build_request(scoped, &entities)
            }
            AiAction::Voice => {
                let notes = bible::raw_dir(root, "characters");
                voice::build_request(scoped, &notes, &model)
            }
            _ => return None,
        };
        Some(RecheckRequest {
            request,
            to_findings: recheck_parser(action),
        })
    }

    /// The scoped re-check request a finding would send (#608) — for tests to
    /// assert the request carries only the finding's chapters, without spawning a
    /// worker thread or a live backend.
    #[cfg(test)]
    pub(crate) fn recheck_request_for(&self, finding: &Finding) -> Option<ai::Request> {
        let action = finding.action?;
        let files = self.finding_chapter_files(finding);
        self.recheck_request(action, finding.book, &files)
            .map(|r| r.request)
    }

    /// Parse a mock re-check `response` into the UI findings the finding's pass
    /// produces (#608) — the real parse+map the worker runs, for tests to drive
    /// `apply_recheck` end-to-end without a live backend or a thread.
    #[cfg(test)]
    #[allow(clippy::unused_self)] // test helper; kept a method for symmetry with recheck_request_for
    pub(crate) fn recheck_findings_from(&self, finding: &Finding, response: &str) -> Vec<Finding> {
        let Some(action) = finding.action else {
            return Vec::new();
        };
        recheck_parser(action)(response, action, finding.book)
    }

    /// Apply a completed single-finding re-check (#608). The response is the
    /// finding's pass re-run over its chapters, parsed to the findings that pass
    /// now produces. If the re-checked finding is no longer among them, the fix
    /// held — clear it from triage and the overlay; if it's still there, keep it
    /// and say so. New findings the scoped run surfaced are accumulated into triage
    /// but don't pop the overlay (this is a verification, not a fresh sweep).
    pub(crate) fn apply_recheck(&mut self, result: Result<Vec<Finding>, ai::AiError>) {
        let Some(rechecked) = self.pending_recheck.take() else {
            return;
        };
        let Some(fresh) = self.ok_or_report(result) else {
            return;
        };
        // Any newly-surfaced finding in these chapters enters durable triage, but
        // silently — the author asked to verify one fix, not to run a full sweep.
        self.accumulate_triage(&fresh);
        if fresh.contains(&rechecked) {
            // Still flagged: the fix didn't clear it. Leave it in place.
            self.message = Some("Still flagged — re-check found the issue again".into());
        } else {
            // Cleared: the scoped re-run no longer produces it. Drop its record so a
            // later full pass may surface it afresh if it recurs, and remove it from
            // the overlay + triage list.
            self.resolve_finding(&rechecked);
            self.remove_finding_from_overlay(&rechecked);
            self.message = Some("Cleared — the fix resolved this finding".into());
        }
    }
}
