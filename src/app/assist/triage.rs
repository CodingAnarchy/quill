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
            self.mode = Mode::Findings(Box::new(Findings {
                items: self.triage.clone(),
                selected: 0,
            }));
        }
    }

    /// Accumulate a pass's findings into the revision-triage list, de-duplicated,
    /// and persist each new one to the durable store (#591). A finding the author
    /// already dismissed is skipped — a re-run that re-produces it must not
    /// re-surface it.
    pub(in crate::app) fn accumulate_triage(&mut self, findings: &[Finding]) {
        let root = self.project.root.clone();
        for f in findings {
            if self.findings_store.is_dismissed(f) {
                continue;
            }
            self.findings_store.remember(&root, f);
            if !self.triage.contains(f) {
                self.triage.push(f.clone());
            }
        }
    }
}
