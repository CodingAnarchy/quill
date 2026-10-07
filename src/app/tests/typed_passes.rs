//! Test-only typed `apply_*` entry points for the analysis-only AI passes (naming,
//! style, editorial, accuracy, relationship, foreshadow, fact-check, pacing,
//! voice). Production reaches the same findings through the generic worker path
//! (`spawn_findings_pass` → `findings_parser` → `apply_findings`); these let a test
//! drive a pass from its parsed struct without a mock backend. Moved out of
//! `assist/trackers.rs` so the production file holds only production code.

use super::super::assist::{
    describe_accuracy, describe_editorial, describe_factcheck, describe_foreshadow,
    describe_naming, describe_pacing, describe_relationship, describe_style, describe_voice,
    findings_from,
};
use super::super::*;
use crate::ai::accuracy::Accuracy;
use crate::ai::factcheck::FactCheck;
use crate::ai::foreshadow::Foreshadow;
use crate::ai::naming::Naming;
use crate::ai::pacing::Pacing;
use crate::ai::relationship::Trajectory;
use crate::ai::style::Style;
use crate::ai::voice::Voice;
use crate::mode::AiAction;

impl App {
    /// Surface an analysis-only pass's typed result as findings: unwrap (reporting an
    /// error the shared way), map each item with the pass's `describe` to a UI finding
    /// attributed to `action`, and open the findings list. The single body the typed
    /// `apply_*` handlers share.
    ///
    /// These typed `apply_*` entry points are test-only: in production
    /// every analysis-only pass reaches the *same* findings through the generic worker
    /// path ([`spawn_findings_pass`](App::spawn_findings_pass) →
    /// [`findings_parser`](super::findings_parser) → [`apply_findings`](App::apply_findings)),
    /// so nothing but the tests unwraps a typed pass result. They stay because they let a
    /// test drive a pass from its parsed struct without standing up a mock backend, and
    /// they exercise the `describe_*` → summary mapping directly. Active-book attribution
    /// (`book = None`), like every findings pass.
    fn surface_pass_findings<T>(
        &mut self,
        result: Result<Vec<T>, ai::AiError>,
        action: AiAction,
        describe: impl Fn(&T) -> (String, Vec<String>),
    ) {
        let Some(items) = self.ok_or_report(result) else {
            return;
        };
        let findings = findings_from(&items, None, action, describe);
        self.show_findings(findings);
    }

    /// Apply a naming pass: surface the variant clusters as findings. Pure
    /// analysis — nothing is written (not even a bible file).
    pub(crate) fn apply_naming(&mut self, result: Result<Naming, ai::AiError>) {
        self.surface_pass_findings(
            result.map(|n| n.findings),
            AiAction::Naming,
            describe_naming,
        );
    }

    /// Apply a prose/style pass: surface the observations as findings. Pure
    /// analysis — nothing is written.
    pub(crate) fn apply_style(&mut self, result: Result<Style, ai::AiError>) {
        self.surface_pass_findings(result.map(|s| s.findings), AiAction::Style, describe_style);
    }

    /// Apply an editorial read (#293): turn each fix-free note into a triage
    /// finding (`focus: concern`, plus a quoted span for line-level issues) and
    /// surface them in the findings list.
    pub(crate) fn apply_editorial(
        &mut self,
        result: Result<Vec<ai::editorial::Note>, ai::AiError>,
    ) {
        self.surface_pass_findings(result, AiAction::Editorial, describe_editorial);
    }

    /// Apply a cultural & historical accuracy pass (#973): surface each inaccuracy
    /// observation as a triage finding (the flagged passage + why it may be off for
    /// the named concern). Pure analysis — nothing is written, no prose is produced.
    pub(crate) fn apply_accuracy(&mut self, result: Result<Accuracy, ai::AiError>) {
        self.surface_pass_findings(
            result.map(|a| a.findings),
            AiAction::Accuracy,
            describe_accuracy,
        );
    }

    /// Apply a relationship-trajectory pass (#1046): surface each inconsistency in
    /// the chosen pair's arc as a triage finding (the drift + the chapters it spans).
    /// Pure analysis — nothing is written, no prose is produced, and the trajectory
    /// itself is observation only; the author reads the arc and decides.
    pub(crate) fn apply_relationship(&mut self, result: Result<Trajectory, ai::AiError>) {
        self.surface_pass_findings(
            result.map(|t| t.findings),
            AiAction::Relationship,
            describe_relationship,
        );
    }

    /// Apply a foreshadow / payoff scan (#1047): surface each unpaid concrete setup
    /// as a triage finding (the flagged setup passage + why it reads as unpaid, its
    /// chapter). Pure analysis — nothing is written, no payoff is drafted.
    pub(crate) fn apply_foreshadow(&mut self, result: Result<Foreshadow, ai::AiError>) {
        self.surface_pass_findings(
            result.map(|f| f.findings),
            AiAction::Foreshadow,
            describe_foreshadow,
        );
    }

    /// Apply a research-notebook fact-check (#1049): surface each contradiction of an
    /// author's note as a triage finding (the cited note, how the passage disagrees,
    /// and the flagged passage). Pure analysis — nothing is written, no prose is
    /// produced, and every finding is grounded in a note the author wrote.
    pub(crate) fn apply_factcheck(&mut self, result: Result<FactCheck, ai::AiError>) {
        self.surface_pass_findings(
            result.map(|f| f.findings),
            AiAction::FactCheck,
            describe_factcheck,
        );
    }

    /// Apply a pacing & tension reading (#1048): surface each located rhythm
    /// observation as a triage finding (what the stretch reads like + the chapters it
    /// spans). Pure analysis — nothing is written, and no suggested change or prose is
    /// produced; the finding names the rhythm and where it is, never a fix.
    pub(crate) fn apply_pacing(&mut self, result: Result<Pacing, ai::AiError>) {
        self.surface_pass_findings(
            result.map(|p| p.observations),
            AiAction::Pacing,
            describe_pacing,
        );
    }

    /// Apply a character-voice pass (#183): surface the observations as findings,
    /// naming the character each concerns. Pure analysis — nothing is written.
    pub(crate) fn apply_voice(&mut self, result: Result<Voice, ai::AiError>) {
        self.surface_pass_findings(result.map(|v| v.findings), AiAction::Voice, describe_voice);
    }
}
