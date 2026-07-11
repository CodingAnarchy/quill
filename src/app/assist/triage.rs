//! Revision-triage accumulation (#591): the standing list of findings the
//! analysis passes surface, de-duplicated against what the author already
//! dismissed and persisted to the durable store.

use super::super::*;

impl App {
    /// Open the accumulated revision-triage findings (local; no request).
    pub(crate) fn open_triage(&mut self) {
        self.overlay = None;
        if self.triage.is_empty() {
            self.message = Some("No findings yet — run a continuity / setting / style pass".into());
        } else {
            // Collapse cross-pass agreement (#921) so a span several passes flag is
            // one row weighted by consensus; identity is unchanged (corroborators
            // aren't part of it), so re-check / dismiss / jump still match by content.
            self.mode = Mode::Findings(Box::new(Findings {
                items: super::super::health::consensus(&self.triage),
                ..Default::default()
            }));
        }
    }

    /// Accumulate a pass's findings into the revision-triage list, de-duplicated,
    /// and persist each new one to the durable store (#591). A finding the author
    /// already dismissed is skipped — a re-run that re-produces it must not
    /// re-surface it.
    ///
    /// When the pass wrote a new finding to the tracked store and the project is
    /// under version control, the store is auto-committed on its own (#1060) so the
    /// write doesn't leave the tree "dirty" and block Sync/Pull/switch-draft — and
    /// so each pass's findings are captured in history for the findings diff (#1051).
    pub(in crate::app) fn accumulate_triage(&mut self, findings: &[Finding]) {
        let root = self.project.root.clone();
        let mut wrote = false;
        for f in findings {
            if self.findings_store.is_dismissed(f) {
                continue;
            }
            wrote |= self.findings_store.remember(&root, f);
            if !self.triage.contains(f) {
                self.triage.push(f.clone());
            }
        }
        // A dedicated, path-scoped commit of only `bible/findings.toml` — it never
        // sweeps in the author's uncommitted prose. Best effort: a commit failure
        // (or no repo) just leaves the store on disk as before, never blocking the
        // pass. `commit_findings` is itself a no-op when the store didn't change.
        if wrote && self.under_vc {
            let _ = crate::vcs::commit_findings(&root, "Update findings");
        }
        // Baseline the chapters these findings speak for (#1053), so "verify on idle"
        // can tell later whether an edit has touched them since. A no-op unless the
        // opt-in is on; bounded to just these findings' chapters, never the book.
        self.record_verify_baselines(findings);
    }
}
