//! AI-feature orchestration for [`App`]: the worker-thread request lifecycle
//! (`spawn_ai`/`poll_ai`), each tracker's analyze/apply pair, the shared
//! story-bible write+conflict+findings flow, and the bible-review / findings key
//! handlers. Split out of `app/mod.rs` to keep that module focused on core
//! editor state and key dispatch (a child module, so it keeps access to `App`'s
//! private fields). See `docs/ai-features.md`.

use super::bible_review::{Staged, StagedTally};
use super::worker::bible_tracker;
use super::*;
use crate::theme;

use crate::ai::ad_creative::{self, AdCreative};
use crate::ai::brainstorm::{self, Brainstorm};
use crate::ai::marketing::{self, Marketing};
use crate::ai::naming::{self, Naming};
use crate::ai::scaffold::{self, Scaffold};
use crate::ai::social::{self, Social};
use crate::ai::style::{self, Style};
use crate::ai::submission::{self, Submission};
use crate::ai::voice;
use crate::ai::workflow_gen::{self, GeneratedWorkflow};
use crate::mode::AiAction;
use crate::text::count_matches;

/// One entry in the session "Ask the manuscript" history (#609): the question,
/// its grounded answer (prose + cited chapter titles), and the token usage of the
/// request. Session-only recall — never persisted.
#[derive(Clone)]
pub(crate) struct QaEntry {
    /// The question as asked. Empty when the answer arrived without a recorded
    /// question (e.g. a test that drives `apply_qa_result` directly).
    pub(crate) question: String,
    /// The grounded answer: the prose plus the chapters it cited.
    pub(crate) answer: ai::Answer,
}

/// Turn a Q&A answer's cited chapter titles into jumpable [`Finding`]s (#609) —
/// one per citation, each targeting that chapter so Enter jumps there via the
/// shared findings handler. De-duplicated (a model may cite a chapter twice),
/// order preserved. Empty titles are dropped. `book: None` — Q&A runs over the
/// active book's manuscript.
fn qa_citation_findings(answer: &ai::Answer) -> Vec<Finding> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for title in &answer.citations {
        let title = title.trim();
        if title.is_empty() || seen.contains(&title) {
            continue;
        }
        seen.push(title);
        out.push(Finding {
            summary: format!("cited: {title}"),
            chapters: vec![title.to_string()],
            book: None,
        });
    }
    out
}

/// The answer prose as leading, non-jumpable rows for the citations list (#609):
/// each non-empty line of the answer becomes a `Finding` with no chapters (so
/// Enter on it is a no-op), followed by a blank spacer row. Empty answer → no rows.
fn answer_context_rows(answer: &ai::Answer) -> Vec<Finding> {
    let mut rows: Vec<Finding> = answer
        .answer
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| Finding {
            summary: l.to_string(),
            chapters: Vec::new(),
            book: None,
        })
        .collect();
    if !rows.is_empty() {
        rows.push(Finding {
            summary: String::new(),
            chapters: Vec::new(),
            book: None,
        });
    }
    rows
}

/// Build the jumpable Q&A-history findings (#609): every logged ask, newest
/// first, one row per cited chapter (`Q: … → <chapter>`, jumpable), plus a single
/// non-jumpable row for an ask that cited nothing. Lets the whole session's
/// lookups be browsed and re-jumped from one list.
fn qa_history_findings(log: &[QaEntry]) -> Vec<Finding> {
    let mut out = Vec::new();
    for entry in log.iter().rev() {
        let q = entry.question.trim();
        let label = if q.is_empty() {
            "Q".to_string()
        } else {
            format!("Q: {q}")
        };
        let cited: Vec<&String> = entry
            .answer
            .citations
            .iter()
            .filter(|c| !c.trim().is_empty())
            .collect();
        if cited.is_empty() {
            out.push(Finding {
                summary: format!("{label} — (no chapter cited)"),
                chapters: Vec::new(),
                book: None,
            });
        } else {
            for title in cited {
                let title = title.trim();
                out.push(Finding {
                    summary: format!("{label} → {title}"),
                    chapters: vec![title.to_string()],
                    book: None,
                });
            }
        }
    }
    out
}

/// A one-line status message for one or more failed writes (#153).
pub(super) fn write_failure_message(failures: &[String]) -> String {
    match failures.split_first() {
        Some((first, [])) => format!("Couldn't write {first}"),
        Some((first, rest)) => format!("Couldn't write {} files — first: {first}", rest.len() + 1),
        None => String::new(),
    }
}

/// Map a pass's typed findings into UI [`Finding`]s, attributing each to `book`
/// (`None` for an active-book pass). `describe` yields the one-line summary and
/// the chapters for one item — the single place the analysis applies share, so a
/// change to `Finding` touches here, not seven handlers (#425).
fn findings_from<T>(
    items: &[T],
    book: Option<usize>,
    describe: impl Fn(&T) -> (String, Vec<String>),
) -> Vec<Finding> {
    items
        .iter()
        .map(|it| {
            let (summary, chapters) = describe(it);
            Finding {
                summary,
                chapters,
                book,
            }
        })
        .collect()
}

impl App {
    /// Begin building a workflow from a description (#232): only when AI is ready,
    /// then open the description prompt. The estimate is trivial (no manuscript is
    /// sent), so there's no cost confirm.
    pub(crate) fn start_describe_workflow(&mut self) {
        match self.ai.check_ready() {
            Ok(_) => self.open_prompt(Pending::DescribeWorkflow, "Describe a workflow: ", "", None),
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    /// Generate a workflow from a plain-language description (#232): send the action
    /// vocabulary plus the description to the model (no manuscript — cheap) and
    /// propose the result via [`poll_ai`](App::poll_ai). The schema constrains the
    /// steps to valid tokens, so the model can't invent an action.
    pub(crate) fn generate_workflow(&mut self, description: &str) {
        let description = description.trim().to_string();
        if description.is_empty() {
            return;
        }
        let vocab = self.workflow_vocab();
        let model = self.ai.model.clone();
        self.spawn_outcome("⟳ Building a workflow…", move |backend| {
            let req = workflow_gen::build_request(&description, &vocab, &model);
            Outcome::Workflow(
                backend
                    .complete(&req)
                    .and_then(|r| workflow_gen::parse(&r.text)),
            )
        });
    }

    /// Handle a generated-workflow reply (#232): report a request nothing mapped
    /// to, validate the proposal against the registry, then stash the steps and
    /// open the name-confirm prompt (prefilled with the proposed name) so the
    /// author confirms, renames, or cancels before anything is written.
    pub(crate) fn apply_generated_workflow(
        &mut self,
        result: Result<GeneratedWorkflow, ai::AiError>,
    ) {
        let Some(gen) = self.ok_or_report(result) else {
            return;
        };
        if gen.steps.is_empty() {
            self.message = Some(if gen.unmapped.is_empty() {
                "Couldn't build a workflow from that".into()
            } else {
                format!("Couldn't map: {}", gen.unmapped.join(", "))
            });
            return;
        }
        // Defense beyond the schema enum: reject an unknown/interactive step or a
        // cycle before offering to save (resolve treats the proposal as a new
        // workflow alongside the existing ones).
        let mut candidate = self.workflows.clone();
        candidate.push(crate::workflow::Workflow {
            name: gen.name.clone(),
            steps: gen.steps.clone(),
        });
        if let Err(e) = crate::workflow::resolve(&candidate, &gen.name) {
            self.message = Some(format!("Proposed workflow is invalid: {e}"));
            return;
        }
        let note = if gen.unmapped.is_empty() {
            String::new()
        } else {
            format!("(couldn't map: {}) ", gen.unmapped.join(", "))
        };
        let label = format!("{note}Save workflow [{}] as: ", gen.steps.join(" → "));
        self.pending_workflow = Some(gen.steps);
        self.open_prompt(Pending::NameNewWorkflow, label, &gen.name, None);
    }

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
    pub(super) fn accumulate_triage(&mut self, findings: &[Finding]) {
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
    // --- AI: "Ask the manuscript" -----------------------------------------

    /// Start an "Ask the manuscript" request on a worker thread. The manuscript
    /// is assembled here (on the main thread) and moved into the thread with the
    /// question and a backend; the result returns via `ai_rx`, polled each tick.
    /// A no-op with a status message when AI isn't ready or a request is already
    /// in flight.
    pub(crate) fn ask_ai(&mut self, question: &str) {
        let question = question.trim().to_string();
        if question.is_empty() {
            return;
        }
        // Capture the question so the answer can be filed under it in the session
        // Q&A log when it returns (#609). Set before the spawn so it's recorded
        // even if the request is a no-op (the spawn clears it on a not-ready gate).
        self.pending_qa_question = Some(question.clone());
        self.spawn_ai(
            "⟳ Sending to Claude…",
            move |backend, manuscript, model| {
                Outcome::Answer(
                    backend.complete(&ai::build_qa_request(manuscript, &question, &model)),
                )
            },
        );
    }

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

    /// Run one persistent-bible pass (#339) for `action` scoped to `book` — the
    /// active-book passes call it with the active book; the series-bible refresh
    /// (#340) drives it per book. The tracker carries its own request builders and
    /// parse, so the single `bible_tracker` lookup is both the gate and the
    /// dispatch (#510).
    pub(crate) fn run_book_bible_pass(&mut self, action: AiAction, book: usize, force: bool) {
        if let Some(t) = bible_tracker(action) {
            self.run_bible_tracker(t, book, force);
        }
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

    /// Generate submission material (synopsis / blurb / query); written to
    /// `submission/` by [`App::poll_ai`].
    pub(crate) fn analyze_submission(&mut self) {
        self.spawn_manuscript_pass(
            "⟳ Drafting submission material…",
            submission::build_request,
            submission::parse,
            Outcome::Submission,
        );
    }

    /// Brainstorm story ideas from a premise (#255) — the first generative pass.
    /// Sends only the premise (no manuscript), so it works on an empty project;
    /// the ideas are written to `bible/brainstorm.md` by [`App::poll_ai`].
    pub(crate) fn brainstorm(&mut self, premise: &str) {
        self.spawn_generative(
            premise,
            "⟳ Brainstorming…",
            |backend, premise, model| {
                let req = brainstorm::build_request(premise, &model);
                Outcome::Brainstorm(
                    backend
                        .complete(&req)
                        .and_then(|r| brainstorm::parse(&r.text)),
                )
            },
        );
    }

    /// Draft a starter character cast from a concept (#256) — generative, no
    /// manuscript. Reuses the extractive pass's `Outcome::Characters` so the
    /// profiles are written to `bible/characters/` by [`apply_characters`].
    pub(crate) fn draft_characters(&mut self, concept: &str) {
        self.spawn_generative(
            concept,
            "⟳ Drafting characters…",
            |backend, concept, model| {
                let req = character::build_concept_request(concept, &model);
                Outcome::Characters(
                    backend
                        .complete(&req)
                        .and_then(|r| character::parse(&r.text)),
                )
            },
        );
    }

    /// Draft a starter setting bible from a concept (#257) — generative, no
    /// manuscript. Reuses the extractive pass's `Outcome::Setting` so the bible is
    /// written to `bible/setting.md` by [`apply_setting`].
    pub(crate) fn draft_setting(&mut self, concept: &str) {
        self.spawn_generative(
            concept,
            "⟳ Drafting setting…",
            |backend, concept, model| {
                let req = setting::build_concept_request(concept, &model);
                Outcome::Setting(backend.complete(&req).and_then(|r| setting::parse(&r.text)))
            },
        );
    }

    /// Apply a brainstorm pass (#255): write the idea notes to `bible/brainstorm.md`,
    /// conflict-reviewed and off-manuscript (never a chapter file, never prose).
    pub(crate) fn apply_brainstorm(&mut self, result: Result<Brainstorm, ai::AiError>) {
        self.overlay = None;
        let Some(ideas) = self.ok_or_report(result) else {
            return;
        };
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("brainstorm.md");
        self.apply_bible_with_findings(
            path,
            "brainstorm",
            |notes| brainstorm::render(&ideas, notes, &today),
            Vec::new(),
        );
    }

    /// Generate marketing copy (#236) — blurb variants & taglines; written to
    /// `marketing/` by [`App::poll_ai`].
    pub(crate) fn analyze_marketing(&mut self) {
        self.spawn_manuscript_pass(
            "⟳ Drafting marketing copy…",
            marketing::build_request,
            marketing::parse,
            Outcome::Marketing,
        );
    }

    /// Generate social posts (#237) — per-platform promo copy; written to
    /// `marketing/social/` by [`App::poll_ai`].
    pub(crate) fn analyze_social(&mut self) {
        self.spawn_manuscript_pass(
            "⟳ Drafting social posts…",
            social::build_request,
            social::parse,
            Outcome::Social,
        );
    }

    /// Generate ad creative & copy (#251) — per-platform ad fields written to
    /// `marketing/ads/` by [`App::poll_ai`]. Folds in any existing marketing copy
    /// (#236/#237) so the ads stay on-brand.
    pub(crate) fn analyze_ad_creative(&mut self) {
        let marketing = self.marketing_copy_context();
        self.spawn_manuscript_pass(
            "⟳ Drafting ad creative…",
            move |manuscript, model| ad_creative::build_request(manuscript, &marketing, model),
            ad_creative::parse,
            Outcome::AdCreative,
        );
    }

    /// The author's existing marketing copy (#236/#237) as on-brand context for
    /// the ad pass — the blurbs/taglines and social posts already written, bodies
    /// only (front matter stripped). Empty when none exist yet.
    pub(crate) fn marketing_copy_context(&self) -> String {
        let root = self.project.root.join("marketing");
        let files = [
            "blurbs.md",
            "taglines.md",
            "social/x.md",
            "social/facebook.md",
            "social/instagram.md",
        ];
        let mut out = String::new();
        for file in files {
            if let Ok(text) = std::fs::read_to_string(root.join(file)) {
                out.push_str(crate::frontmatter::split(&text).1.trim());
                out.push_str("\n\n");
            }
        }
        out
    }

    /// Apply an ad-creative pass (#251): write the per-platform ad copy to
    /// `marketing/ads/`, conflict-reviewed and off-manuscript.
    pub(crate) fn apply_ad_creative(&mut self, result: Result<AdCreative, ai::AiError>) {
        if let Some(creative) = self.ok_or_report(result) {
            self.write_off_manuscript("Ad creative", "marketing/ads", creative.artifacts());
        }
    }

    /// Show an AI result — a Q&A answer or a pass summary — as the full-screen
    /// overlay, scrolled to the top. The single place an AI result becomes an
    /// overlay.
    pub(super) fn show_ai(&mut self, lines: Vec<Line<'static>>) {
        self.overlay = Some(Overlay::ai(lines));
    }

    /// Unwrap an AI result, or set the standard `AI: <error>` status and return
    /// `None` — the shared error arm of the `apply_*`/`analyze_*` handlers (#315).
    pub(super) fn ok_or_report<T>(&mut self, result: Result<T, ai::AiError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                None
            }
        }
    }

    /// Apply a completed Q&A result (#609): parse the grounded answer, record it in
    /// the session Q&A log (under the question it answered, for later recall), then
    /// surface it — as a jumpable citations list when it cites chapters (reusing the
    /// [`Finding`] jump), or as the plain answer overlay when it cites none. Errors
    /// set a status message. Shared by the async poll path and tests (which drive a
    /// mock backend synchronously).
    pub(crate) fn apply_qa_result(&mut self, result: Result<ai::Response, ai::AiError>) {
        let question = self.pending_qa_question.take().unwrap_or_default();
        let Some(resp) = self.ok_or_report(result) else {
            return;
        };
        let answer = ai::parse_answer(&resp.text);
        // Record the ask before rendering, so the log always reflects it even if the
        // author dismisses the overlay immediately (#609).
        self.qa_log.push(QaEntry {
            question,
            answer: answer.clone(),
        });
        self.present_qa(&answer, &resp.usage);
    }

    /// Surface a grounded Q&A answer (#609). When it cites chapters, open a single
    /// jumpable [`Mode::Findings`] list — the answer prose as leading, non-jumpable
    /// context rows, then one jumpable entry per citation, so Enter jumps to the
    /// cited chapter exactly like a triage finding. When it cites none, fall back to
    /// the plain answer overlay. The citations never enter durable triage: they're
    /// retrieval for this ask, not standing revision findings.
    pub(super) fn present_qa(&mut self, answer: &ai::Answer, usage: &ai::Usage) {
        self.overlay = None;
        let citations = qa_citation_findings(answer);
        if citations.is_empty() {
            self.show_ai(ai::answer_to_lines(answer, usage));
            return;
        }
        // Lead with the answer prose as context rows (empty `chapters` → not a jump
        // target, a no-op on Enter), then the jumpable citations.
        let mut items: Vec<Finding> = answer_context_rows(answer);
        items.extend(citations);
        // Select the first jumpable citation so Enter jumps straight there.
        let selected = items
            .iter()
            .position(|f| !f.chapters.is_empty())
            .unwrap_or(0);
        self.mode = Mode::Findings(Box::new(Findings { items, selected }));
    }

    /// Browse the session's "Ask the manuscript" history (#609): every past ask's
    /// citations, newest first, as one jumpable [`Mode::Findings`] list — each row
    /// labels the question and the chapter it cited, so the author can re-jump any
    /// earlier lookup without re-asking (and without a fresh request). An ask that
    /// cited no chapter still shows, as a non-jumpable row. Empty log → a status.
    pub(crate) fn open_qa_history(&mut self) {
        self.overlay = None;
        if self.qa_log.is_empty() {
            self.message = Some("No questions asked yet — run \"Ask the manuscript\"".into());
            return;
        }
        let items = qa_history_findings(&self.qa_log);
        self.mode = Mode::Findings(Box::new(Findings { items, selected: 0 }));
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
        let findings = findings_from(&cont.findings, book, |f| {
            (format!("{}: {}", f.kind, f.detail), f.chapters.clone())
        });
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
        let findings = findings_from(&setting.findings, book, |f| {
            (format!("{}: {}", f.rule, f.violation), f.chapters.clone())
        });
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("setting.md");
        self.apply_bible_with_findings(
            path,
            "setting",
            |notes| setting::render(&setting, notes, &today),
            findings,
        );
    }

    // --- In-context thesaurus / word search (#290) ------------------------

    /// Open the word-search prompt, prefilled with the word under the cursor so the
    /// common case is "land on a word, invoke, hit enter (or add filters)".
    pub(crate) fn start_thesaurus(&mut self) {
        let buffer = self.edit_buffer();
        let prefill = buffer
            .text_object(TextObject::Word, false)
            .map(|(lo, hi)| buffer.byte_slice(lo, hi))
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .unwrap_or_default();
        self.open_prompt(
            Pending::Thesaurus,
            "Word search — word/phrase, then any filters (e.g. \u{201c}sad; 2 syllables, Anglo-Saxon; 8\u{201d}): ",
            &prefill,
            None,
        );
    }

    /// Run the word search: the typed request plus the current line as context
    /// (for fit only). Off the editor thread; the candidates arrive in an overlay.
    pub(crate) fn thesaurus(&mut self, request: &str) {
        let request = request.trim().to_string();
        if request.is_empty() {
            self.message = Some("Word search: nothing to look up.".into());
            return;
        }
        let buffer = self.edit_buffer();
        let context = buffer.line(buffer.cursor.line);
        let model = self.ai.model.clone();
        self.spawn_outcome("\u{27f3} Searching words\u{2026}", move |backend| {
            let req = ai::thesaurus::build_request(&request, &context, &model);
            Outcome::Thesaurus(
                backend
                    .complete(&req)
                    .and_then(|r| ai::thesaurus::parse(&r.text)),
            )
        });
    }

    /// Show the word-search results in the AI overlay.
    pub(crate) fn apply_thesaurus(
        &mut self,
        result: Result<Vec<ai::thesaurus::Word>, ai::AiError>,
    ) {
        if let Some(words) = self.ok_or_report(result) {
            self.show_ai(thesaurus_lines(&words));
        }
    }

    // --- Name generator (#610) --------------------------------------------

    /// Open the name-generator kind picker: character, place, or invented term.
    /// Choosing one opens the constraints prompt — the deliberate, two-step lookup
    /// the AI boundary allows (an option list the author picks from, never prose).
    pub(crate) fn start_namegen(&mut self) {
        let items = ai::namegen::Kind::ALL
            .iter()
            .map(|&k| {
                PaletteItem::new(
                    kind_title(k),
                    format!("Generate {} names by constraints", k.label()),
                    Command::PickNameKind(k),
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Name generator — what needs a name?",
            items,
            selected: 0,
            collapsed: Default::default(),
        }));
    }

    /// After the kind is chosen, prompt for the free-text constraints (origin,
    /// syllables, tone, era) before running the lookup.
    pub(crate) fn prompt_namegen(&mut self, kind: ai::namegen::Kind) {
        self.open_prompt(
            Pending::NameGen(kind),
            format!(
                "Name a {} \u{2014} constraints (e.g. \u{201c}Norse, 2 syllables, harsh, medieval\u{201d}): ",
                kind.label()
            ),
            "",
            None,
        );
    }

    /// Run the name generator: the chosen kind plus the typed constraints. Off the
    /// editor thread; the candidates arrive as a pickable list overlay. No
    /// manuscript is attached — it's a small, off-manuscript option-list lookup.
    pub(crate) fn namegen(&mut self, kind: ai::namegen::Kind, constraints: &str) {
        let constraints = constraints.trim().to_string();
        let model = self.ai.model.clone();
        self.spawn_outcome("\u{27f3} Generating names\u{2026}", move |backend| {
            let req = ai::namegen::build_request(kind, &constraints, &model);
            Outcome::NameGen(
                backend
                    .complete(&req)
                    .and_then(|r| ai::namegen::parse(&r.text)),
            )
        });
    }

    /// Show the name-generator candidates as a pickable list (#610): each row is a
    /// candidate name with its note/tags; Enter places the chosen name at the
    /// cursor (mirroring the snippet picker). The author picks and Quill places —
    /// no prose is written, only the one name the author chose. Empty result → a
    /// status telling the author to loosen the constraints.
    pub(crate) fn apply_namegen(&mut self, result: Result<Vec<ai::namegen::Name>, ai::AiError>) {
        let Some(names) = self.ok_or_report(result) else {
            return;
        };
        let names: Vec<ai::namegen::Name> = names
            .into_iter()
            .filter(|n| !n.text.trim().is_empty())
            .collect();
        if names.is_empty() {
            self.message = Some("No names — try loosening the constraints.".into());
            return;
        }
        let items = names
            .iter()
            .map(|n| {
                PaletteItem::new(
                    n.text.clone(),
                    namegen_desc(n),
                    Command::InsertName(n.text.clone()),
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Pick a name to place",
            items,
            selected: 0,
            collapsed: Default::default(),
        }));
    }

    /// Place the chosen candidate name at the cursor (#610), through the normal
    /// edit path (so it works in both views) as one undo group. Verbatim: smart
    /// typography is bypassed, the name lands exactly as chosen.
    pub(crate) fn insert_name(&mut self, name: &str) {
        self.edit_buffer_mut().seal_undo_group();
        for c in name.chars() {
            self.edit_insert_raw(c);
        }
        self.edit_buffer_mut().seal_undo_group();
        self.message = Some(format!("Placed \u{201c}{name}\u{201d}."));
    }

    /// Apply a scaffolding pass: write the beat outline to `bible/outline.md` and
    /// surface the structural gaps as findings.
    pub(crate) fn apply_scaffold(&mut self, result: Result<Scaffold, ai::AiError>) {
        let Some(scaffold) = self.ok_or_report(result) else {
            return;
        };
        let findings = findings_from(&scaffold.gaps, None, |g| {
            (format!("gap: {}", g.detail), vec![g.chapter.clone()])
        });
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
        let findings = findings_from(&naming.findings, None, |f| {
            let summary = if f.variants.is_empty() {
                f.name.clone()
            } else {
                format!("{} — also: {}", f.name, f.variants.join(", "))
            };
            (summary, f.chapters.clone())
        });
        self.show_findings(findings);
    }

    /// Apply a prose/style pass: surface the observations as findings. Pure
    /// analysis — nothing is written.
    pub(crate) fn apply_style(&mut self, result: Result<Style, ai::AiError>) {
        let Some(style) = self.ok_or_report(result) else {
            return;
        };
        let findings = findings_from(&style.findings, None, |f| {
            (format!("{}: {}", f.kind, f.detail), f.chapters.clone())
        });
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
        let findings = findings_from(&notes, None, |n| {
            let at = n
                .location
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|q| format!(" \u{2014} \u{201c}{q}\u{201d}"))
                .unwrap_or_default();
            (
                format!("{}: {}{at}", n.focus, n.concern),
                n.chapters.clone(),
            )
        });
        self.show_findings(findings);
    }

    /// Apply a character-voice pass (#183): surface the observations as findings,
    /// naming the character each concerns. Pure analysis — nothing is written.
    pub(crate) fn apply_voice(&mut self, result: Result<voice::Voice, ai::AiError>) {
        let Some(voice) = self.ok_or_report(result) else {
            return;
        };
        let findings = findings_from(&voice.findings, None, |f| {
            let summary = if f.character.is_empty() {
                format!("{}: {}", f.kind, f.detail)
            } else {
                format!("{} ({}): {}", f.kind, f.character, f.detail)
            };
            (summary, f.chapters.clone())
        });
        self.show_findings(findings);
    }

    /// Apply a submission pass: write the synopsis / blurb / query to
    /// `submission/` (conflict-reviewed like any generated file) and summarize.
    pub(crate) fn apply_submission(&mut self, result: Result<Submission, ai::AiError>) {
        if let Some(sub) = self.ok_or_report(result) {
            let artifacts = sub
                .artifacts()
                .into_iter()
                .map(|(k, f, t)| (k, f, t.to_string()))
                .collect();
            self.write_off_manuscript("Submission material", "submission", artifacts);
        }
    }

    /// Apply a marketing-copy pass (#236): write the blurb variants and taglines
    /// to `marketing/`, conflict-reviewed and off-manuscript like the submission
    /// material.
    pub(crate) fn apply_marketing(&mut self, result: Result<Marketing, ai::AiError>) {
        if let Some(copy) = self.ok_or_report(result) {
            self.write_off_manuscript("Marketing copy", "marketing", copy.artifacts());
        }
    }

    /// Apply a social-posts pass (#237): write the per-platform posts under
    /// `marketing/social/`, conflict-reviewed and off-manuscript.
    pub(crate) fn apply_social(&mut self, result: Result<Social, ai::AiError>) {
        if let Some(posts) = self.ok_or_report(result) {
            self.write_off_manuscript("Social posts", "marketing/social", posts.artifacts());
        }
    }

    /// Write a set of off-manuscript artifacts (#236) — `(title, filename, body)`
    /// — under `<subdir>/`, conflict-reviewed via [`stage_bible`](App::stage_bible)
    /// and summarized. The one writer shared by the submission and marketing passes
    /// (and future off-manuscript copy): they differ only in `label`, `subdir`, and
    /// what their `artifacts()` renders.
    fn write_off_manuscript(
        &mut self,
        label: &str,
        subdir: &str,
        artifacts: Vec<(&'static str, &'static str, String)>,
    ) {
        self.overlay = None;
        let today = date::iso(date::today());
        // `Staged::Wrote` carries no path, so capture each successful write's
        // label as we stage, for the clean-success listing below.
        let mut written_paths = Vec::new();
        let tally = StagedTally::from(artifacts.into_iter().map(|(kind, file, text)| {
            let path = self.project.root.join(subdir).join(file);
            let title = format!("title: {kind}\n");
            let staged = self.stage_bible(path, kind, |notes| {
                bible::render(&title, &text, notes, &today)
            });
            if matches!(staged, Staged::Wrote(_)) {
                written_paths.push(format!("{subdir}/{file}"));
            }
            staged
        }));

        let label = label.to_string();
        // Every `Staged::Wrote` pushed one label, so `written` (the tally count)
        // equals `written_paths.len()`; use it so only the clean closure owns the
        // list, leaving the conflict closure free of a competing borrow.
        self.resolve_staged(
            tally,
            |written, pending| format!("Wrote {written} · {pending} need review"),
            |app, _written| {
                if written_paths.is_empty() {
                    app.message = Some(format!("{label}: nothing generated"));
                    return;
                }
                let mut lines = vec![Line::raw(format!("{label} written:")), Line::raw("")];
                lines.extend(
                    written_paths
                        .into_iter()
                        .map(|w| Line::raw(format!("  {w}"))),
                );
                lines.push(Line::raw(""));
                lines.push(Line::raw(
                    "Off-manuscript drafts — edit freely; never part of the novel.",
                ));
                app.show_ai(lines);
            },
        );
    }

    /// Whether an AI request is currently in flight (for the status indicator).
    pub fn ai_busy(&self) -> bool {
        self.ai_task.is_busy()
    }

    // --- Story-bible conflict review --------------------------------------
}

/// The new selection for a navigable list given a key, or `None` if the key
/// isn't a movement key. Shared by the palette, findings, and reference-file
/// list handlers.
pub(super) fn list_move(selected: usize, len: usize, code: KeyCode) -> Option<usize> {
    match code {
        KeyCode::Up | KeyCode::Char('k') => Some(selected.saturating_sub(1)),
        KeyCode::Down | KeyCode::Char('j') => Some((selected + 1).min(len.saturating_sub(1))),
        _ => None,
    }
}

/// Render word-search candidates for the AI overlay (#290): one block per
/// candidate — the word/phrase and its definition, with any syllable/origin/
/// rarity tags, and a usage note on its own dimmed line.
fn thesaurus_lines(words: &[ai::thesaurus::Word]) -> Vec<Line<'static>> {
    if words.is_empty() {
        return vec![Line::raw("No matches — try loosening the constraints.")];
    }
    let mut lines = vec![
        Line::raw(format!("{} candidates:", words.len())),
        Line::raw(""),
    ];
    for w in words {
        let mut tags: Vec<String> = Vec::new();
        if let Some(s) = w.syllables {
            tags.push(format!("{s} syl"));
        }
        if let Some(o) = w.origin.as_deref().filter(|s| !s.is_empty()) {
            tags.push(o.to_string());
        }
        if let Some(r) = w.rarity.as_deref().filter(|s| !s.is_empty()) {
            tags.push(r.to_string());
        }
        let tail = if tags.is_empty() {
            String::new()
        } else {
            format!("  ({})", tags.join(" \u{b7} "))
        };
        lines.push(Line::raw(format!(
            "{} \u{2014} {}{tail}",
            w.text, w.definition
        )));
        if let Some(note) = w.note.as_deref().filter(|s| !s.is_empty()) {
            let dim = ratatui::style::Style::default().fg(theme::current().dim);
            lines.push(Line::from(Span::styled(format!("    {note}"), dim)));
        }
    }
    lines
}

/// The name-generator kind picker's row title (#610) — the capitalized kind.
fn kind_title(kind: ai::namegen::Kind) -> String {
    let label = kind.label();
    let mut chars = label.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// A candidate name's one-line description for the pick list (#610): its note,
/// with any syllable / origin / pronunciation tags appended.
fn namegen_desc(name: &ai::namegen::Name) -> String {
    let mut tags: Vec<String> = Vec::new();
    if let Some(s) = name.syllables {
        tags.push(format!("{s} syl"));
    }
    if let Some(o) = name
        .origin
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        tags.push(o.to_string());
    }
    if let Some(p) = name
        .pronunciation
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        tags.push(format!("/{p}/"));
    }
    let note = name.note.trim();
    match (note.is_empty(), tags.is_empty()) {
        (true, true) => String::new(),
        (false, true) => note.to_string(),
        (true, false) => tags.join(" \u{b7} "),
        (false, false) => format!("{note}  ({})", tags.join(" \u{b7} ")),
    }
}

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
