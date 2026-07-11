//! The story-bible analyze/apply passes: each tracker's `analyze_*` starts a
//! worker request and its `apply_*` writes the bible artifact (conflict-reviewed)
//! and/or surfaces the findings. Covers the persistent-bible passes (characters,
//! continuity, setting, plot) and the analysis-only passes (scaffold, naming,
//! style, editorial, voice), plus the local character mention enrichment and the
//! derived cross-cast relationship index (#707).

use super::super::bible_review::StagedTally;
use super::super::*;

use super::write_failure_message;
// The describe_* used by the live bible/scaffold apply handlers. The analysis-only
// passes' describe_* + typed result structs are used only by the test-only typed
// `apply_*` entry points, so they're imported inside the `#[cfg(test)]` block below.
use super::findings_from;
use super::{
    describe_continuity, describe_plot, describe_scaffold, describe_setting, describe_theme,
};
use crate::ai::accuracy;
use crate::ai::factcheck;
use crate::ai::foreshadow;
use crate::ai::naming;
use crate::ai::pacing;
use crate::ai::relationship;
use crate::ai::scaffold::{self, Scaffold};
use crate::ai::style;
use crate::ai::voice;
use crate::mode::AiAction;
use crate::text::count_matches;

// Structs + describe_* the test-only typed `apply_*` handlers unwrap and map — kept
// in a test-gated `use` so a non-test build doesn't flag them unused (the worker path
// reaches the same findings via `findings_parser`, never these typed structs).
#[cfg(test)]
use super::{
    describe_accuracy, describe_editorial, describe_factcheck, describe_foreshadow,
    describe_naming, describe_pacing, describe_relationship, describe_style, describe_voice,
};
#[cfg(test)]
use crate::ai::accuracy::Accuracy;
#[cfg(test)]
use crate::ai::factcheck::FactCheck;
#[cfg(test)]
use crate::ai::foreshadow::Foreshadow;
#[cfg(test)]
use crate::ai::naming::Naming;
#[cfg(test)]
use crate::ai::pacing::Pacing;
#[cfg(test)]
use crate::ai::relationship::Trajectory;
#[cfg(test)]
use crate::ai::style::Style;

/// The summary overlay shown after a clean character pass (no conflicts).
fn character_summary_lines(total: usize, written: usize, drift: usize) -> Vec<Line<'static>> {
    vec![
        Line::raw("Character analysis complete."),
        Line::raw(""),
        Line::raw(format!("Characters analyzed: {total}")),
        Line::raw(format!(
            "Profiles written:    {written}  (bible/characters/)"
        )),
        Line::raw(format!("Drift flags:         {drift}")),
        Line::raw(""),
        Line::raw("Open the bible files to review; the Notes section is yours to edit."),
    ]
}

impl App {
    /// Start a character-extraction pass — incremental from the changed chapters
    /// when the bible already exists, else a full pass (#90). Profiles are written
    /// / conflicts queued by [`App::poll_ai`].
    pub(crate) fn analyze_characters(&mut self, force: bool) {
        self.run_book_bible_pass(AiAction::Characters, self.project.active_book(), force);
    }

    /// Start a timeline & continuity pass (incremental when possible, #90); the
    /// timeline + findings are applied by [`App::poll_ai`]. When the author invoked
    /// it over an editor selection / chapter range (#1054), scope it to just those
    /// chapters instead of the whole book — cheaper, focused, same finding kind.
    pub(crate) fn analyze_continuity(&mut self, force: bool) {
        if self.pass_scope.is_some() {
            self.analyze_scoped(AiAction::Continuity);
            return;
        }
        self.run_book_bible_pass(AiAction::Continuity, self.project.active_book(), force);
    }

    /// Start a setting & worldbuilding pass (incremental when possible, #90); the
    /// setting bible + rule findings are applied by [`App::poll_ai`]. When the
    /// author invoked it over an editor selection / chapter range (#1054), scope it
    /// to just those chapters instead of the whole book.
    pub(crate) fn analyze_setting(&mut self, force: bool) {
        if self.pass_scope.is_some() {
            self.analyze_scoped(AiAction::Setting);
            return;
        }
        self.run_book_bible_pass(AiAction::Setting, self.project.active_book(), force);
    }

    /// Start a plot-thread / arc pass (incremental when possible, #90); the plot
    /// tracker + dangling-thread findings are applied by [`App::poll_ai`].
    pub(crate) fn analyze_plot(&mut self, force: bool) {
        self.run_book_bible_pass(AiAction::Plot, self.project.active_book(), force);
    }

    /// Start a theme / motif pass (incremental when possible, #90); the theme
    /// tracker + dangling-motif findings are applied by [`App::poll_ai`].
    pub(crate) fn analyze_theme(&mut self, force: bool) {
        self.run_book_bible_pass(AiAction::Theme, self.project.active_book(), force);
    }

    /// Start a scaffolding pass; the beat outline + gap findings are applied by
    /// [`App::poll_ai`]. Unlike the analysis-only passes it writes `bible/outline.md`,
    /// so it keeps its own [`Outcome::Scaffold`] + [`apply_scaffold`](Self::apply_scaffold).
    pub(crate) fn analyze_scaffold(&mut self) {
        self.spawn_manuscript_pass(
            "⟳ Outlining…",
            scaffold::build_request,
            scaffold::parse,
            Outcome::Scaffold,
        );
    }

    /// Start a naming-consistency pass; variant findings are applied by
    /// [`App::poll_ai`]. Always runs on the Haiku-tier model (a cheap, high-volume
    /// scan), checked against the known entity list.
    pub(crate) fn analyze_naming(&mut self) {
        let entities = self.known_entities();
        self.spawn_findings_pass(
            AiAction::Naming,
            "⟳ Checking names…",
            move |manuscript, _model| naming::build_request(manuscript, &entities),
        );
    }

    /// Start a prose & style pass (POV/tense/voice — semantic, analysis only);
    /// findings are applied by [`App::poll_ai`].
    pub(crate) fn analyze_style(&mut self) {
        self.spawn_findings_pass(
            AiAction::Style,
            "⟳ Reviewing prose & style…",
            style::build_request,
        );
    }

    /// Start an editorial read (#293): flag structural / continuity / line issues
    /// across the manuscript — observations only, never fixes — applied by
    /// [`App::poll_ai`] into the revision-triage list.
    pub(crate) fn analyze_editorial(&mut self) {
        self.spawn_findings_pass(
            AiAction::Editorial,
            "⟳ Editorial read…",
            ai::editorial::build_request,
        );
    }

    /// Start a cultural & historical accuracy pass (#973) for the concern the author
    /// named (`accuracy_concern`, set by the prompt) — flag passages that read as
    /// factually / period / culturally inaccurate for that named concern, applied by
    /// [`App::poll_ai`] into the revision-triage list. Findings only; author-invoked;
    /// accuracy, NOT a sensitivity / representation review. The concern is *taken*
    /// here (one pass per submission), so a later run re-prompts rather than reusing a
    /// stale concern; a missing concern is a no-op (the prompt always sets one first).
    pub(crate) fn analyze_accuracy(&mut self) {
        let Some(concern) = self.accuracy_concern.take() else {
            return;
        };
        let concern = concern.trim().to_string();
        if concern.is_empty() {
            return;
        }
        self.spawn_findings_pass(
            AiAction::Accuracy,
            "⟳ Checking accuracy…",
            move |manuscript, model| accuracy::build_request(manuscript, &concern, model),
        );
    }

    /// Start a relationship-trajectory pass (#1046) for the pair the author chose
    /// (`relationship_pair`, set by the prompt) — track the pair's state per chapter
    /// and flag inconsistencies (contradictions / off-page reversals), applied by
    /// [`App::poll_ai`] into the revision-triage list. Findings only; author-invoked;
    /// it observes the drift, never rewrites the arc. The pair is *taken* here (one
    /// pass per submission), so a later run re-prompts rather than reusing a stale
    /// pair; a missing / blank pair is a no-op (the prompt always sets one first).
    pub(crate) fn analyze_relationship(&mut self) {
        let Some(pair) = self.relationship_pair.take() else {
            return;
        };
        let pair = pair.trim().to_string();
        if pair.is_empty() {
            return;
        }
        self.spawn_findings_pass(
            AiAction::Relationship,
            "⟳ Tracking relationship…",
            move |manuscript, model| relationship::build_request(manuscript, &pair, model),
        );
    }

    /// Start a foreshadow / payoff scan (#1047): pair each concrete seeded detail
    /// (an object, a named threat, a promised meeting, a planted clue) with its
    /// resolution and flag the UNPAID ones, applied by [`App::poll_ai`] into the
    /// revision-triage list. Findings only; author-invoked; never drafts the payoff.
    /// Distinct from — and narrower than — the plot pass's thematic-thread tracking.
    pub(crate) fn analyze_foreshadow(&mut self) {
        self.spawn_findings_pass(
            AiAction::Foreshadow,
            "⟳ Scanning setups & payoffs…",
            foreshadow::build_request,
        );
    }

    /// Start a research-notebook fact-check (#1049): read the author's own research
    /// notebook (`bible/research.md`, #932) and flag manuscript passages that
    /// contradict a note the author wrote — applied by [`App::poll_ai`] into the
    /// revision-triage list. Findings only; author-invoked; grounded *solely* in the
    /// author's notes (no external facts), distinct from the accuracy pass. When the
    /// notebook is absent or holds no authored notes (only the seed template), there
    /// is nothing to check against, so the pass is a no-op with a nudge rather than a
    /// costed call with no basis.
    pub(crate) fn analyze_factcheck(&mut self) {
        let research =
            std::fs::read_to_string(bible::research_path(&self.project.root)).unwrap_or_default();
        if bible::research_notes(&self.project.root).is_empty() {
            self.message =
                Some("Add notes to your research notebook (bible/research.md) first".into());
            return;
        }
        self.spawn_findings_pass(
            AiAction::FactCheck,
            "⟳ Fact-checking against your research notes…",
            move |manuscript, model| factcheck::build_request(manuscript, &research, model),
        );
    }

    /// Start a pacing & tension reading (#1048): read the manuscript's scene-level
    /// tension/quiet RHYTHM and report it as located, plain-language observations —
    /// applied by [`App::poll_ai`] into the revision-triage list. Findings only;
    /// author-invoked; observation only — it names the rhythm and where it is, never
    /// suggests a cut, an addition, a reorder, or reworded prose (it sits closest to
    /// the co-writer line). A deterministic per-chapter pacing signal (word count,
    /// sentence cadence, dialogue share) computed *locally* is fed alongside the prose
    /// as read-only grounding so the located observations stay honest about where the
    /// rhythm shifts; the reading itself is still observation-only.
    pub(crate) fn analyze_pacing(&mut self) {
        let signal = self.pacing_signal_context();
        self.spawn_findings_pass(
            AiAction::Pacing,
            "⟳ Reading pacing & tension…",
            move |manuscript, model| pacing::build_request(manuscript, &signal, model),
        );
    }

    /// The deterministic per-chapter pacing signal for the active book (#1048),
    /// rendered as the read-only grounding block [`analyze_pacing`](Self::analyze_pacing)
    /// feeds the reading. Measured locally from each chapter's prose (word count,
    /// sentence cadence, dialogue share) — a grounding input, not a finding, and it
    /// never leaves the observation-only contract.
    fn pacing_signal_context(&self) -> String {
        let active = self.project.active_index();
        let signals: Vec<pacing::ChapterSignal> = (0..self.chapter_count())
            .map(|i| {
                let title = self.project.chapter_title(i).unwrap_or("");
                let prose = self.chapter_text_with_active(i, active);
                let sig = pacing::chapter_signal(title, &prose);
                // The signal is `Copy` and its title borrows the manifest (outlives
                // this render), so the transient `prose` string is free to drop here.
                sig
            })
            .collect();
        pacing::signal_context(&signals)
    }

    /// Start a character-voice pass (#183): check the manuscript against the
    /// character bible's voice profiles, surfacing dialogue / narration
    /// consistency findings via [`App::poll_ai`]. Needs the character bible for
    /// grounding, so it asks for the character pass first when there isn't one.
    pub(crate) fn analyze_voice(&mut self) {
        let notes = bible::raw_dir(&self.project.root, "characters");
        // The precondition is voice *profiles* to ground on, not merely a
        // character bible — a pre-#183 bible (or one whose characters have no
        // voice) has no `## Voice` section, so it can't ground this pass.
        if !notes.contains("## Voice") {
            self.message = Some("Run the character pass first to capture voice profiles".into());
            return;
        }
        self.spawn_findings_pass(
            AiAction::Voice,
            "⟳ Checking character voice…",
            move |manuscript, model| voice::build_request(manuscript, &notes, model),
        );
    }

    /// Apply extracted characters: enrich with local mention stats, write each
    /// profile to `bible/characters/`, and either summarize (no conflicts) or
    /// queue hand-edited files for review.
    pub(crate) fn apply_characters(&mut self, result: Result<Vec<Character>, ai::AiError>) {
        self.overlay = None; // a new result supersedes any prior overlay
        let Some(mut chars) = self.ok_or_report(result) else {
            return;
        };
        if chars.is_empty() {
            // Shared by the extractive pass (#74) and the generative draft (#256),
            // so phrase it for both: nothing extracted / nothing drafted.
            self.message = Some("No character profiles produced.".into());
            return;
        }
        for c in &mut chars {
            self.enrich_mentions(c);
        }

        let dir = self.project.root.join("bible").join("characters");
        let today = date::iso(date::today());
        let drift: usize = chars.iter().map(|c| c.drift.len()).sum();
        let tally = StagedTally::from(chars.iter().map(|c| {
            let path = dir.join(format!("{}.md", bible::slug(&c.name)));
            self.stage_bible(path, &c.name, |notes| {
                character::render_profile(c, notes, &today)
            })
        }));
        // Refresh the cross-cast relationship index (#707) from the profiles just
        // written — pure aggregation of the extracted edges, no new AI call.
        self.write_relationship_index(&chars, &today);

        let count = chars.len();
        self.resolve_staged(
            tally,
            |written, pending| format!("Wrote {written} · {pending} need review"),
            |app, written| {
                app.record_pass_state(); // the bible now reflects this manuscript state (#90)
                app.show_ai(character_summary_lines(count, written, drift));
            },
        );
    }

    /// Write the aggregated relationship index to `bible/relationships.md` (#707):
    /// a cross-cast rollup of the `relationships` the character profiles already
    /// carry. Fully derived from the profiles (no author Notes block to preserve),
    /// so it's regenerated wholesale each time profiles are written rather than
    /// staged for conflict review. A write failure is surfaced but doesn't abort
    /// the pass — the per-character profiles are the primary artifact.
    pub(crate) fn write_relationship_index(&mut self, chars: &[Character], today: &str) {
        let path = self.project.root.join("bible").join("relationships.md");
        let index = character::render_index(chars, today);
        if let Err(msg) = self.write_file(&path, &index) {
            self.message = Some(write_failure_message(&[msg]));
        }
    }

    /// Count a character's name/alias mentions across the manuscript and record
    /// the last chapter it appears in — local "screen time" / "last seen", free.
    pub(crate) fn enrich_mentions(&self, c: &mut Character) {
        let needles: Vec<&str> = std::iter::once(&c.name)
            .chain(&c.aliases)
            .map(String::as_str)
            .filter(|n| !n.is_empty())
            .collect();
        let mut total = 0;
        let mut last = None;
        for i in 0..self.chapter_count() {
            let text = self.chapter_text(i);
            let count: usize = needles.iter().map(|n| count_matches(&text, n)).sum();
            if count > 0 {
                total += count;
                last = self.project.chapter_title(i).map(str::to_string);
            }
        }
        c.mentions = total;
        c.last_chapter = last;
    }

    /// Apply a continuity pass: write the ordered timeline to `bible/timeline.md`
    /// (conflict-reviewed like any bible file) and open the findings overlay. If
    /// the timeline was hand-edited, resolve that first; the findings follow.
    pub(crate) fn apply_continuity(&mut self, result: Result<Continuity, ai::AiError>) {
        let Some(cont) = self.ok_or_report(result) else {
            return;
        };
        let book = self.current_pass_book();
        let findings = findings_from(
            &cont.findings,
            book,
            AiAction::Continuity,
            describe_continuity,
        );
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("timeline.md");
        self.apply_bible_with_findings(
            path,
            "timeline",
            |notes| continuity::render_timeline(&cont, notes, &today),
            findings,
        );
    }

    /// Apply a setting pass: write `bible/setting.md` and surface rule-violation
    /// findings.
    pub(crate) fn apply_setting(&mut self, result: Result<Setting, ai::AiError>) {
        let Some(setting) = self.ok_or_report(result) else {
            return;
        };
        let book = self.current_pass_book();
        let findings = findings_from(&setting.findings, book, AiAction::Setting, describe_setting);
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("setting.md");
        self.apply_bible_with_findings(
            path,
            "setting",
            |notes| setting::render(&setting, notes, &today),
            findings,
        );
    }

    /// Apply a plot pass (#606): write the plot-thread tracker to `bible/plot.md`
    /// (conflict-reviewed like any bible file) and surface the dangling-thread
    /// findings through the existing triage.
    pub(crate) fn apply_plot(&mut self, result: Result<Plot, ai::AiError>) {
        let Some(plot) = self.ok_or_report(result) else {
            return;
        };
        let book = self.current_pass_book();
        let findings = findings_from(&plot.findings, book, AiAction::Plot, describe_plot);
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("plot.md");
        self.apply_bible_with_findings(
            path,
            "plot",
            |notes| plot::render(&plot, notes, &today),
            findings,
        );
    }

    /// Apply a theme pass (#828): write the theme / motif tracker to
    /// `bible/theme.md` (conflict-reviewed like any bible file) and surface the
    /// dangling-motif findings through the existing triage.
    pub(crate) fn apply_theme(&mut self, result: Result<ThemeBible, ai::AiError>) {
        let Some(theme) = self.ok_or_report(result) else {
            return;
        };
        let book = self.current_pass_book();
        let findings = findings_from(&theme.findings, book, AiAction::Theme, describe_theme);
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("theme.md");
        self.apply_bible_with_findings(
            path,
            "theme",
            |notes| ai_theme::render(&theme, notes, &today),
            findings,
        );
    }

    /// Surface an analysis-only pass's typed result as findings: unwrap (reporting an
    /// error the shared way), map each item with the pass's `describe` to a UI finding
    /// attributed to `action`, and open the findings list. The single body the typed
    /// `apply_*` handlers share.
    ///
    /// These typed `apply_*` entry points are test-only (`#[cfg(test)]`): in production
    /// every analysis-only pass reaches the *same* findings through the generic worker
    /// path ([`spawn_findings_pass`](App::spawn_findings_pass) →
    /// [`findings_parser`](super::findings_parser) → [`apply_findings`](App::apply_findings)),
    /// so nothing but the tests unwraps a typed pass result. They stay because they let a
    /// test drive a pass from its parsed struct without standing up a mock backend, and
    /// they exercise the `describe_*` → summary mapping directly. Active-book attribution
    /// (`book = None`), like every findings pass.
    #[cfg(test)]
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

    /// Apply a scaffolding pass: write the beat outline to `bible/outline.md` and
    /// surface the structural gaps as findings.
    pub(crate) fn apply_scaffold(&mut self, result: Result<Scaffold, ai::AiError>) {
        let Some(scaffold) = self.ok_or_report(result) else {
            return;
        };
        let findings = findings_from(&scaffold.gaps, None, AiAction::Scaffold, describe_scaffold);
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("outline.md");
        self.apply_bible_with_findings(
            path,
            "outline",
            |notes| scaffold::render_outline(&scaffold, notes, &today),
            findings,
        );
    }

    /// Apply a naming pass: surface the variant clusters as findings. Pure
    /// analysis — nothing is written (not even a bible file).
    #[cfg(test)]
    pub(crate) fn apply_naming(&mut self, result: Result<Naming, ai::AiError>) {
        self.surface_pass_findings(
            result.map(|n| n.findings),
            AiAction::Naming,
            describe_naming,
        );
    }

    /// Apply a prose/style pass: surface the observations as findings. Pure
    /// analysis — nothing is written.
    #[cfg(test)]
    pub(crate) fn apply_style(&mut self, result: Result<Style, ai::AiError>) {
        self.surface_pass_findings(result.map(|s| s.findings), AiAction::Style, describe_style);
    }

    /// Apply an editorial read (#293): turn each fix-free note into a triage
    /// finding (`focus: concern`, plus a quoted span for line-level issues) and
    /// surface them in the findings list.
    #[cfg(test)]
    pub(crate) fn apply_editorial(
        &mut self,
        result: Result<Vec<ai::editorial::Note>, ai::AiError>,
    ) {
        self.surface_pass_findings(result, AiAction::Editorial, describe_editorial);
    }

    /// Apply a cultural & historical accuracy pass (#973): surface each inaccuracy
    /// observation as a triage finding (the flagged passage + why it may be off for
    /// the named concern). Pure analysis — nothing is written, no prose is produced.
    #[cfg(test)]
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
    #[cfg(test)]
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
    #[cfg(test)]
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
    #[cfg(test)]
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
    #[cfg(test)]
    pub(crate) fn apply_pacing(&mut self, result: Result<Pacing, ai::AiError>) {
        self.surface_pass_findings(
            result.map(|p| p.observations),
            AiAction::Pacing,
            describe_pacing,
        );
    }

    /// Apply a character-voice pass (#183): surface the observations as findings,
    /// naming the character each concerns. Pure analysis — nothing is written.
    #[cfg(test)]
    pub(crate) fn apply_voice(&mut self, result: Result<voice::Voice, ai::AiError>) {
        self.surface_pass_findings(result.map(|v| v.findings), AiAction::Voice, describe_voice);
    }
}
