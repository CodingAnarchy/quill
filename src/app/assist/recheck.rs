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

impl App {
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
            AiAction::Style => style::build_request(scoped, &model),
            AiAction::Scaffold => scaffold::build_request(scoped, &model),
            AiAction::Editorial => ai::editorial::build_request(scoped, &model),
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
