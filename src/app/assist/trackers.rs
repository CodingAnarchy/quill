//! The story-bible analyze/apply passes: each tracker's `analyze_*` starts a
//! worker request and its `apply_*` writes the bible artifact (conflict-reviewed)
//! and/or surfaces the findings. Covers the persistent-bible passes (characters,
//! continuity, setting, plot) and the analysis-only passes (scaffold, naming,
//! style, editorial, voice), plus the local character mention enrichment and the
//! derived cross-cast relationship index (#707).

use super::super::bible_review::StagedTally;
use super::super::*;

use super::write_failure_message;
use super::{
    describe_continuity, describe_editorial, describe_naming, describe_plot, describe_scaffold,
    describe_setting, describe_style, describe_theme, describe_voice, findings_from,
};
use crate::ai::naming::{self, Naming};
use crate::ai::scaffold::{self, Scaffold};
use crate::ai::style::{self, Style};
use crate::ai::voice;
use crate::mode::AiAction;
use crate::text::count_matches;

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
    /// timeline + findings are applied by [`App::poll_ai`].
    pub(crate) fn analyze_continuity(&mut self, force: bool) {
        self.run_book_bible_pass(AiAction::Continuity, self.project.active_book(), force);
    }

    /// Start a setting & worldbuilding pass (incremental when possible, #90); the
    /// setting bible + rule findings are applied by [`App::poll_ai`].
    pub(crate) fn analyze_setting(&mut self, force: bool) {
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
    /// [`App::poll_ai`].
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
        self.spawn_manuscript_pass(
            "⟳ Checking names…",
            move |manuscript, _model| naming::build_request(manuscript, &entities),
            naming::parse,
            Outcome::Naming,
        );
    }

    /// Start a prose & style pass (POV/tense/voice — semantic, analysis only);
    /// findings are applied by [`App::poll_ai`].
    pub(crate) fn analyze_style(&mut self) {
        self.spawn_manuscript_pass(
            "⟳ Reviewing prose & style…",
            style::build_request,
            style::parse,
            Outcome::Style,
        );
    }

    /// Start an editorial read (#293): flag structural / continuity / line issues
    /// across the manuscript — observations only, never fixes — applied by
    /// [`App::poll_ai`] into the revision-triage list.
    pub(crate) fn analyze_editorial(&mut self) {
        self.spawn_manuscript_pass(
            "⟳ Editorial read…",
            ai::editorial::build_request,
            ai::editorial::parse,
            Outcome::Editorial,
        );
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
        self.spawn_manuscript_pass(
            "⟳ Checking character voice…",
            move |manuscript, model| voice::build_request(manuscript, &notes, model),
            voice::parse,
            Outcome::Voice,
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
        let book = self.current_series_bible_book();
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
        let book = self.current_series_bible_book();
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
        let book = self.current_series_bible_book();
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
        let book = self.current_series_bible_book();
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
    pub(crate) fn apply_naming(&mut self, result: Result<Naming, ai::AiError>) {
        let Some(naming) = self.ok_or_report(result) else {
            return;
        };
        let findings = findings_from(&naming.findings, None, AiAction::Naming, describe_naming);
        self.show_findings(findings);
    }

    /// Apply a prose/style pass: surface the observations as findings. Pure
    /// analysis — nothing is written.
    pub(crate) fn apply_style(&mut self, result: Result<Style, ai::AiError>) {
        let Some(style) = self.ok_or_report(result) else {
            return;
        };
        let findings = findings_from(&style.findings, None, AiAction::Style, describe_style);
        self.show_findings(findings);
    }

    /// Apply an editorial read (#293): turn each fix-free note into a triage
    /// finding (`focus: concern`, plus a quoted span for line-level issues) and
    /// surface them in the findings list.
    pub(crate) fn apply_editorial(
        &mut self,
        result: Result<Vec<ai::editorial::Note>, ai::AiError>,
    ) {
        let Some(notes) = self.ok_or_report(result) else {
            return;
        };
        let findings = findings_from(&notes, None, AiAction::Editorial, describe_editorial);
        self.show_findings(findings);
    }

    /// Apply a character-voice pass (#183): surface the observations as findings,
    /// naming the character each concerns. Pure analysis — nothing is written.
    pub(crate) fn apply_voice(&mut self, result: Result<voice::Voice, ai::AiError>) {
        let Some(voice) = self.ok_or_report(result) else {
            return;
        };
        let findings = findings_from(&voice.findings, None, AiAction::Voice, describe_voice);
        self.show_findings(findings);
    }
}
