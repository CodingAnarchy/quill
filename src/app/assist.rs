//! AI-feature orchestration for [`App`]: the worker-thread request lifecycle
//! (`spawn_ai`/`poll_ai`), each tracker's analyze/apply pair, the shared
//! story-bible write+conflict+findings flow, and the bible-review / findings key
//! handlers. Split out of `app/mod.rs` to keep that module focused on core
//! editor state and key dispatch (a child module, so it keeps access to `App`'s
//! private fields). See `docs/ai-features.md`.

use super::*;
use super::bible_review::Staged;
use super::worker::bible_tracker;


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


/// A one-line status message for one or more failed writes (#153).
pub(super) fn write_failure_message(failures: &[String]) -> String {
    match failures.split_first() {
        Some((first, [])) => format!("Couldn't write {first}"),
        Some((first, rest)) => format!("Couldn't write {} files — first: {first}", rest.len() + 1),
        None => String::new(),
    }
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
            Outcome::Workflow(backend.complete(&req).and_then(|r| workflow_gen::parse(&r.text)))
        });
    }

    /// Handle a generated-workflow reply (#232): report a request nothing mapped
    /// to, validate the proposal against the registry, then stash the steps and
    /// open the name-confirm prompt (prefilled with the proposed name) so the
    /// author confirms, renames, or cancels before anything is written.
    pub(crate) fn apply_generated_workflow(&mut self, result: Result<GeneratedWorkflow, ai::AiError>) {
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
        candidate.push(crate::workflow::Workflow { name: gen.name.clone(), steps: gen.steps.clone() });
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

    /// Accumulate a pass's findings into the session revision-triage list,
    /// de-duplicated.
    pub(super) fn accumulate_triage(&mut self, findings: &[Finding]) {
        for f in findings {
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
        self.spawn_ai("⟳ Sending to Claude…", move |backend, manuscript, model| {
            Outcome::Answer(backend.complete(&ai::build_qa_request(manuscript, &question, &model)))
        });
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
    /// (#340) drives it per book. Routes to each tracker's request builders.
    pub(crate) fn run_book_bible_pass(&mut self, action: AiAction, book: usize, force: bool) {
        let Some(t) = bible_tracker(action) else { return };
        match action {
            AiAction::Characters => self.run_bible_tracker(t, book, force, character::build_request, character::build_incremental_request, |r| {
                Outcome::Characters(r.and_then(|resp| character::parse(&resp.text)))
            }),
            AiAction::Continuity => self.run_bible_tracker(t, book, force, continuity::build_request, continuity::build_incremental_request, |r| {
                Outcome::Continuity(r.and_then(|resp| continuity::parse(&resp.text)))
            }),
            AiAction::Setting => self.run_bible_tracker(t, book, force, setting::build_request, setting::build_incremental_request, |r| {
                Outcome::Setting(r.and_then(|resp| setting::parse(&resp.text)))
            }),
            // `bible_tracker` returned `Some`, so `action` is one of the three above.
            _ => unreachable!("run_book_bible_pass: non-bible action passed the tracker gate"),
        }
    }

    /// Start a scaffolding pass; the beat outline + gap findings are applied by
    /// [`App::poll_ai`].
    pub(crate) fn analyze_scaffold(&mut self) {
        self.spawn_ai("⟳ Outlining…", |backend, manuscript, model| {
            let req = scaffold::build_request(manuscript, &model);
            Outcome::Scaffold(backend.complete(&req).and_then(|r| scaffold::parse(&r.text)))
        });
    }

    /// Start a naming-consistency pass; variant findings are applied by
    /// [`App::poll_ai`]. Always runs on the Haiku-tier model (a cheap, high-volume
    /// scan), checked against the known entity list.
    pub(crate) fn analyze_naming(&mut self) {
        let entities = self.known_entities();
        self.spawn_ai("⟳ Checking names…", move |backend, manuscript, _model| {
            let req = naming::build_request(manuscript, &entities);
            Outcome::Naming(backend.complete(&req).and_then(|r| naming::parse(&r.text)))
        });
    }

    /// Start a prose & style pass (POV/tense/voice — semantic, analysis only);
    /// findings are applied by [`App::poll_ai`].
    pub(crate) fn analyze_style(&mut self) {
        self.spawn_ai("⟳ Reviewing prose & style…", |backend, manuscript, model| {
            let req = style::build_request(manuscript, &model);
            Outcome::Style(backend.complete(&req).and_then(|r| style::parse(&r.text)))
        });
    }

    /// Start an editorial read (#293): flag structural / continuity / line issues
    /// across the manuscript — observations only, never fixes — applied by
    /// [`App::poll_ai`] into the revision-triage list.
    pub(crate) fn analyze_editorial(&mut self) {
        self.spawn_ai("⟳ Editorial read…", |backend, manuscript, model| {
            let req = ai::editorial::build_request(manuscript, &model);
            Outcome::Editorial(backend.complete(&req).and_then(|r| ai::editorial::parse(&r.text)))
        });
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
        self.spawn_ai("⟳ Checking character voice…", move |backend, manuscript, model| {
            let req = voice::build_request(manuscript, &notes, &model);
            Outcome::Voice(backend.complete(&req).and_then(|r| voice::parse(&r.text)))
        });
    }

    /// Generate submission material (synopsis / blurb / query); written to
    /// `submission/` by [`App::poll_ai`].
    pub(crate) fn analyze_submission(&mut self) {
        self.spawn_ai("⟳ Drafting submission material…", |backend, manuscript, model| {
            let req = submission::build_request(manuscript, &model);
            Outcome::Submission(backend.complete(&req).and_then(|r| submission::parse(&r.text)))
        });
    }

    /// Brainstorm story ideas from a premise (#255) — the first generative pass.
    /// Sends only the premise (no manuscript), so it works on an empty project;
    /// the ideas are written to `bible/brainstorm.md` by [`App::poll_ai`].
    pub(crate) fn brainstorm(&mut self, premise: &str) {
        self.spawn_generative(premise, "⟳ Brainstorming…", |backend, premise, model| {
            let req = brainstorm::build_request(premise, &model);
            Outcome::Brainstorm(backend.complete(&req).and_then(|r| brainstorm::parse(&r.text)))
        });
    }

    /// Draft a starter character cast from a concept (#256) — generative, no
    /// manuscript. Reuses the extractive pass's `Outcome::Characters` so the
    /// profiles are written to `bible/characters/` by [`apply_characters`].
    pub(crate) fn draft_characters(&mut self, concept: &str) {
        self.spawn_generative(concept, "⟳ Drafting characters…", |backend, concept, model| {
            let req = character::build_concept_request(concept, &model);
            Outcome::Characters(backend.complete(&req).and_then(|r| character::parse(&r.text)))
        });
    }

    /// Draft a starter setting bible from a concept (#257) — generative, no
    /// manuscript. Reuses the extractive pass's `Outcome::Setting` so the bible is
    /// written to `bible/setting.md` by [`apply_setting`].
    pub(crate) fn draft_setting(&mut self, concept: &str) {
        self.spawn_generative(concept, "⟳ Drafting setting…", |backend, concept, model| {
            let req = setting::build_concept_request(concept, &model);
            Outcome::Setting(backend.complete(&req).and_then(|r| setting::parse(&r.text)))
        });
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
        self.apply_bible_with_findings(path, "brainstorm", |notes| brainstorm::render(&ideas, notes, &today), Vec::new());
    }

    /// Generate marketing copy (#236) — blurb variants & taglines; written to
    /// `marketing/` by [`App::poll_ai`].
    pub(crate) fn analyze_marketing(&mut self) {
        self.spawn_ai("⟳ Drafting marketing copy…", |backend, manuscript, model| {
            let req = marketing::build_request(manuscript, &model);
            Outcome::Marketing(backend.complete(&req).and_then(|r| marketing::parse(&r.text)))
        });
    }

    /// Generate social posts (#237) — per-platform promo copy; written to
    /// `marketing/social/` by [`App::poll_ai`].
    pub(crate) fn analyze_social(&mut self) {
        self.spawn_ai("⟳ Drafting social posts…", |backend, manuscript, model| {
            let req = social::build_request(manuscript, &model);
            Outcome::Social(backend.complete(&req).and_then(|r| social::parse(&r.text)))
        });
    }

    /// Generate ad creative & copy (#251) — per-platform ad fields written to
    /// `marketing/ads/` by [`App::poll_ai`]. Folds in any existing marketing copy
    /// (#236/#237) so the ads stay on-brand.
    pub(crate) fn analyze_ad_creative(&mut self) {
        let marketing = self.marketing_copy_context();
        self.spawn_ai("⟳ Drafting ad creative…", move |backend, manuscript, model| {
            let req = ad_creative::build_request(manuscript, &marketing, &model);
            Outcome::AdCreative(backend.complete(&req).and_then(|r| ad_creative::parse(&r.text)))
        });
    }

    /// The author's existing marketing copy (#236/#237) as on-brand context for
    /// the ad pass — the blurbs/taglines and social posts already written, bodies
    /// only (front matter stripped). Empty when none exist yet.
    pub(crate) fn marketing_copy_context(&self) -> String {
        let root = self.project.root.join("marketing");
        let files = ["blurbs.md", "taglines.md", "social/x.md", "social/facebook.md", "social/instagram.md"];
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
        match result {
            Ok(creative) => self.write_off_manuscript("Ad creative", "marketing/ads", creative.artifacts()),
            Err(e) => self.message = Some(format!("AI: {e}")),
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
    fn ok_or_report<T>(&mut self, result: Result<T, ai::AiError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                None
            }
        }
    }

    /// Apply a completed Q&A result: open the answer overlay on success, or set a
    /// status message on error. Shared by the async poll path and tests (which
    /// drive a mock backend synchronously).
    pub(crate) fn apply_qa_result(&mut self, result: Result<ai::Response, ai::AiError>) {
        match result {
            Ok(resp) => self.show_ai(ai::answer_to_lines(&resp)),
            Err(e) => self.message = Some(format!("AI: {e}")),
        }
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
        let mut written = 0;
        let mut drift = 0;
        let mut conflicts = Vec::new();
        let mut failures = Vec::new();
        for c in &chars {
            drift += c.drift.len();
            let path = dir.join(format!("{}.md", bible::slug(&c.name)));
            match self.stage_bible(path, &c.name, |notes| character::render_profile(c, notes, &today)) {
                Staged::Wrote => written += 1,
                Staged::Conflict(conflict) => conflicts.push(conflict),
                Staged::Failed(msg) => failures.push(msg),
            }
        }

        if !failures.is_empty() {
            self.message = Some(write_failure_message(&failures));
        } else if conflicts.is_empty() {
            self.record_pass_state(); // the bible now reflects this manuscript state (#90)
            self.show_ai(character_summary_lines(chars.len(), written, drift));
        } else {
            let pending = conflicts.len();
            self.mode = Mode::BibleReview(Box::new(BibleReview { conflicts, editing: None }));
            self.message = Some(format!("Wrote {written} · {pending} need review"));
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
        let findings = cont
            .findings
            .iter()
            .map(|f| Finding {
                summary: format!("{}: {}", f.kind, f.detail),
                chapters: f.chapters.clone(),
                book,
            })
            .collect();
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("timeline.md");
        self.apply_bible_with_findings(path, "timeline", |notes| {
            continuity::render_timeline(&cont, notes, &today)
        }, findings);
    }

    /// Apply a setting pass: write `bible/setting.md` and surface rule-violation
    /// findings.
    pub(crate) fn apply_setting(&mut self, result: Result<Setting, ai::AiError>) {
        let Some(setting) = self.ok_or_report(result) else {
            return;
        };
        let book = self.current_series_bible_book();
        let findings = setting
            .findings
            .iter()
            .map(|f| Finding {
                summary: format!("{}: {}", f.rule, f.violation),
                chapters: f.chapters.clone(),
                book,
            })
            .collect();
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("setting.md");
        self.apply_bible_with_findings(path, "setting", |notes| {
            setting::render(&setting, notes, &today)
        }, findings);
    }

    // --- Notes import (#283) ----------------------------------------------

    /// Prompt for a notes file (or a folder of them) to import into the story
    /// bible. Available with AI off too — the source is preserved regardless.
    pub(crate) fn start_import(&mut self) {
        self.open_prompt(Pending::ImportNotes, "Import notes — path to a .md file or a folder of them:", "", None);
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
            Outcome::Thesaurus(backend.complete(&req).and_then(|r| ai::thesaurus::parse(&r.text)))
        });
    }

    /// Show the word-search results in the AI overlay.
    pub(crate) fn apply_thesaurus(&mut self, result: Result<Vec<ai::thesaurus::Word>, ai::AiError>) {
        match result {
            Ok(words) => self.show_ai(thesaurus_lines(&words)),
            Err(e) => self.message = Some(format!("AI: {e}")),
        }
    }

    // --- New project from a folder of notes (#286) ------------------------

    /// Prompt for a folder of notes to start a new project from.
    pub(crate) fn start_new_from_notes(&mut self) {
        self.open_prompt(Pending::NewFromNotesSource, "New project from notes — folder to import:", "", None);
    }

    /// Validate `folder` holds importable notes, then ask where to create the new
    /// project. Shared by the folder prompt and the directory browser's `N` key.
    pub(crate) fn begin_new_from_notes(&mut self, folder: &str) {
        let folder = folder.trim();
        if collect_import_files(std::path::Path::new(folder)).is_empty() {
            self.message = Some(format!("No Markdown notes to import at {folder}"));
            self.mode = self.editing_mode();
            return;
        }
        self.pending_notes_folder = Some(std::path::PathBuf::from(folder));
        let prefill = format!("{}/", self.new_project_base().display());
        self.open_prompt(Pending::NewFromNotesDest, "New project — path:", &prefill, None);
    }

    /// Create the new project at `dest` and import the folder chosen earlier.
    pub(crate) fn finish_new_from_notes(&mut self, dest: &str) {
        let Some(folder) = self.pending_notes_folder.take() else {
            self.mode = self.editing_mode();
            return;
        };
        if self.create_and_open(std::path::Path::new(dest.trim())) {
            // The new project is now active; import the notes into its bible.
            self.import_notes(&folder.to_string_lossy());
        }
    }

    /// Import Markdown notes into the story bible: preserve every source file
    /// under `bible/sources/`, then (when AI is on) run one extractive pass over
    /// their combined text to generate/update the bible. A folder imports all the
    /// `.md` files directly inside it, collectively.
    pub(crate) fn import_notes(&mut self, path: &str) {
        let path = std::path::PathBuf::from(path.trim());
        let files = collect_import_files(&path);
        if files.is_empty() {
            self.message = Some(format!("No Markdown files to import at {}", path.display()));
            return;
        }

        // Preserve each source and gather its text, labelled so the AI can tell
        // the files apart in the combined input.
        let dest_dir = self.project.root.join("bible").join("sources");
        let mut combined = String::new();
        let mut preserved = 0;
        let mut failures = Vec::new();
        for f in &files {
            let text = match std::fs::read_to_string(f) {
                Ok(t) => t,
                Err(e) => {
                    failures.push(format!("{}: {e}", f.display()));
                    continue;
                }
            };
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "notes.md".into());
            if let Err(msg) = self.write_file(&dest_dir.join(&name), &text) {
                failures.push(msg);
                continue;
            }
            preserved += 1;
            combined.push_str(&format!("## {name}\n\n{text}\n\n"));
        }
        if preserved == 0 {
            self.message = Some(write_failure_message(&failures));
            return;
        }

        let label = match &files[..] {
            [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "notes".into()),
            _ => format!("{preserved} files"),
        };
        // With AI off, the source is safely preserved but the bible can't be built.
        if self.ai.check_ready().is_err() {
            self.message = Some(format!("Saved {label} to bible/sources — enable AI to build the story bible from it."));
            return;
        }
        self.import_source = Some(label);
        self.spawn_generative(&combined, "⟳ Importing notes…", |backend, notes, model| {
            let req = ai::import::build_request(notes.to_string(), &model);
            Outcome::Import(backend.complete(&req).and_then(|r| ai::import::parse(&r.text)))
        });
    }

    /// Apply an import pass: stage the generated character and setting files (and
    /// surface any hand-edit conflicts together in one review), exactly like the
    /// character/setting passes.
    pub(crate) fn apply_import(&mut self, result: Result<ai::import::Imported, ai::AiError>) {
        self.overlay = None;
        let source = self.import_source.take().unwrap_or_else(|| "notes".into());
        let Some(imported) = self.ok_or_report(result) else {
            return;
        };

        let mut chars = imported.characters;
        for c in &mut chars {
            self.enrich_mentions(c);
        }
        let setting = imported.setting;
        let timeline = imported.timeline;
        let outline = imported.outline;
        let setting_has = !setting.categories.is_empty() || !setting.gazetteer.is_empty();
        let timeline_has = !timeline.events.is_empty();
        let outline_has = !outline.outline.is_empty();
        if chars.is_empty() && !setting_has && !timeline_has && !outline_has {
            self.message = Some(format!("Imported {source}: no bible content found in the notes."));
            return;
        }

        let today = date::iso(date::today());
        let bible = self.project.root.join("bible");
        // Stage every produced file; collect all hand-edit conflicts into one
        // review so a single import never opens several in a row.
        let mut staged: Vec<Staged> = Vec::new();
        for c in &chars {
            let path = bible.join("characters").join(format!("{}.md", bible::slug(&c.name)));
            staged.push(self.stage_bible(path, &c.name, |notes| character::render_profile(c, notes, &today)));
        }
        if setting_has {
            staged.push(self.stage_bible(bible.join("setting.md"), "setting", |notes| setting::render(&setting, notes, &today)));
        }
        if timeline_has {
            staged.push(self.stage_bible(bible.join("timeline.md"), "timeline", |notes| continuity::render_timeline(&timeline, notes, &today)));
        }
        if outline_has {
            staged.push(self.stage_bible(bible.join("outline.md"), "outline", |notes| scaffold::render_outline(&outline, notes, &today)));
        }

        let mut conflicts = Vec::new();
        let mut failures = Vec::new();
        let mut written = 0;
        for result in staged {
            match result {
                Staged::Wrote => written += 1,
                Staged::Conflict(x) => conflicts.push(x),
                Staged::Failed(m) => failures.push(m),
            }
        }

        if !failures.is_empty() {
            self.message = Some(write_failure_message(&failures));
        } else if conflicts.is_empty() {
            self.show_ai(import_summary_lines(&source, chars.len(), setting_has, timeline_has, outline_has, written));
        } else {
            let pending = conflicts.len();
            self.mode = Mode::BibleReview(Box::new(BibleReview { conflicts, editing: None }));
            self.message = Some(format!("Imported {source}: wrote {written} · {pending} need review"));
        }
    }

    /// Apply a scaffolding pass: write the beat outline to `bible/outline.md` and
    /// surface the structural gaps as findings.
    pub(crate) fn apply_scaffold(&mut self, result: Result<Scaffold, ai::AiError>) {
        let Some(scaffold) = self.ok_or_report(result) else {
            return;
        };
        let findings = scaffold
            .gaps
            .iter()
            .map(|g| Finding {
                summary: format!("gap: {}", g.detail),
                chapters: vec![g.chapter.clone()],
                book: None,
            })
            .collect();
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("outline.md");
        self.apply_bible_with_findings(path, "outline", |notes| {
            scaffold::render_outline(&scaffold, notes, &today)
        }, findings);
    }

    /// Apply a naming pass: surface the variant clusters as findings. Pure
    /// analysis — nothing is written (not even a bible file).
    pub(crate) fn apply_naming(&mut self, result: Result<Naming, ai::AiError>) {
        let Some(naming) = self.ok_or_report(result) else {
            return;
        };
        let findings = naming
            .findings
            .iter()
            .map(|f| Finding {
                summary: if f.variants.is_empty() {
                    f.name.clone()
                } else {
                    format!("{} — also: {}", f.name, f.variants.join(", "))
                },
                chapters: f.chapters.clone(),
                book: None,
            })
            .collect();
        self.show_findings(findings);
    }

    /// Apply a prose/style pass: surface the observations as findings. Pure
    /// analysis — nothing is written.
    pub(crate) fn apply_style(&mut self, result: Result<Style, ai::AiError>) {
        let Some(style) = self.ok_or_report(result) else {
            return;
        };
        let findings = style
            .findings
            .iter()
            .map(|f| Finding {
                summary: format!("{}: {}", f.kind, f.detail),
                chapters: f.chapters.clone(),
                book: None,
            })
            .collect();
        self.show_findings(findings);
    }

    /// Apply an editorial read (#293): turn each fix-free note into a triage
    /// finding (`focus: concern`, plus a quoted span for line-level issues) and
    /// surface them in the findings list.
    pub(crate) fn apply_editorial(&mut self, result: Result<Vec<ai::editorial::Note>, ai::AiError>) {
        let Some(notes) = self.ok_or_report(result) else {
            return;
        };
        let findings = notes
            .into_iter()
            .map(|n| {
                let at = n
                    .location
                    .filter(|s| !s.is_empty())
                    .map(|q| format!(" \u{2014} \u{201c}{q}\u{201d}"))
                    .unwrap_or_default();
                Finding { summary: format!("{}: {}{at}", n.focus, n.concern), chapters: n.chapters, book: None }
            })
            .collect();
        self.show_findings(findings);
    }

    /// Apply a character-voice pass (#183): surface the observations as findings,
    /// naming the character each concerns. Pure analysis — nothing is written.
    pub(crate) fn apply_voice(&mut self, result: Result<voice::Voice, ai::AiError>) {
        let Some(voice) = self.ok_or_report(result) else {
            return;
        };
        let findings = voice
            .findings
            .iter()
            .map(|f| Finding {
                summary: if f.character.is_empty() {
                    format!("{}: {}", f.kind, f.detail)
                } else {
                    format!("{} ({}): {}", f.kind, f.character, f.detail)
                },
                chapters: f.chapters.clone(),
                book: None,
            })
            .collect();
        self.show_findings(findings);
    }

    /// Apply a submission pass: write the synopsis / blurb / query to
    /// `submission/` (conflict-reviewed like any generated file) and summarize.
    pub(crate) fn apply_submission(&mut self, result: Result<Submission, ai::AiError>) {
        match result {
            Ok(sub) => {
                let artifacts =
                    sub.artifacts().into_iter().map(|(k, f, t)| (k, f, t.to_string())).collect();
                self.write_off_manuscript("Submission material", "submission", artifacts);
            }
            Err(e) => self.message = Some(format!("AI: {e}")),
        }
    }

    /// Apply a marketing-copy pass (#236): write the blurb variants and taglines
    /// to `marketing/`, conflict-reviewed and off-manuscript like the submission
    /// material.
    pub(crate) fn apply_marketing(&mut self, result: Result<Marketing, ai::AiError>) {
        match result {
            Ok(copy) => self.write_off_manuscript("Marketing copy", "marketing", copy.artifacts()),
            Err(e) => self.message = Some(format!("AI: {e}")),
        }
    }

    /// Apply a social-posts pass (#237): write the per-platform posts under
    /// `marketing/social/`, conflict-reviewed and off-manuscript.
    pub(crate) fn apply_social(&mut self, result: Result<Social, ai::AiError>) {
        match result {
            Ok(posts) => self.write_off_manuscript("Social posts", "marketing/social", posts.artifacts()),
            Err(e) => self.message = Some(format!("AI: {e}")),
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
        let mut written = Vec::new();
        let mut conflicts = Vec::new();
        let mut failures = Vec::new();
        for (kind, file, text) in artifacts {
            let path = self.project.root.join(subdir).join(file);
            let title = format!("title: {kind}\n");
            match self.stage_bible(path, kind, |notes| bible::render(&title, &text, notes, &today)) {
                Staged::Wrote => written.push(format!("{subdir}/{file}")),
                Staged::Conflict(c) => conflicts.push(c),
                Staged::Failed(msg) => failures.push(msg),
            }
        }

        if !failures.is_empty() {
            self.message = Some(write_failure_message(&failures));
        } else if written.is_empty() && conflicts.is_empty() {
            self.message = Some(format!("{label}: nothing generated"));
        } else if conflicts.is_empty() {
            let mut lines = vec![Line::raw(format!("{label} written:")), Line::raw("")];
            lines.extend(written.into_iter().map(|w| Line::raw(format!("  {w}"))));
            lines.push(Line::raw(""));
            lines.push(Line::raw("Off-manuscript drafts — edit freely; never part of the novel."));
            self.show_ai(lines);
        } else {
            let pending = conflicts.len();
            let n = written.len();
            self.mode = Mode::BibleReview(Box::new(BibleReview { conflicts, editing: None }));
            self.message = Some(format!("Wrote {n} · {pending} need review"));
        }
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


/// The summary overlay shown after a clean character pass (no conflicts).
/// The Markdown files to import from `path` (#283): the file itself if it's a
/// `.md` file, or every `.md` directly inside it if it's a folder (sorted), so a
/// folder imports its notes collectively. Empty when the path is neither.
fn collect_import_files(path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let is_md = |p: &std::path::Path| {
        p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("md"))
    };
    if path.is_dir() {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_md(p))
            .collect();
        files.sort();
        files
    } else if path.is_file() && is_md(path) {
        vec![path.to_path_buf()]
    } else {
        Vec::new()
    }
}

/// Summary lines for a completed notes import (#283/#288).
fn import_summary_lines(
    source: &str,
    characters: usize,
    setting: bool,
    timeline: bool,
    outline: bool,
    written: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(format!("Imported {source} into the story bible.")),
        Line::raw(""),
        Line::raw(format!("Characters: {characters}")),
    ];
    let yes = |on: bool| if on { "updated" } else { "—" };
    lines.push(Line::raw(format!("Setting:    {}", yes(setting))));
    lines.push(Line::raw(format!("Timeline:   {}", yes(timeline))));
    lines.push(Line::raw(format!("Outline:    {}", yes(outline))));
    lines.push(Line::raw(format!("Files written: {written}  (bible/)")));
    lines.push(Line::raw(""));
    lines.push(Line::raw("The original notes are preserved under bible/sources/."));
    lines
}

/// Render word-search candidates for the AI overlay (#290): one block per
/// candidate — the word/phrase and its definition, with any syllable/origin/
/// rarity tags, and a usage note on its own dimmed line.
fn thesaurus_lines(words: &[ai::thesaurus::Word]) -> Vec<Line<'static>> {
    if words.is_empty() {
        return vec![Line::raw("No matches — try loosening the constraints.")];
    }
    let mut lines = vec![Line::raw(format!("{} candidates:", words.len())), Line::raw("")];
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
        let tail = if tags.is_empty() { String::new() } else { format!("  ({})", tags.join(" \u{b7} ")) };
        lines.push(Line::raw(format!("{} \u{2014} {}{tail}", w.text, w.definition)));
        if let Some(note) = w.note.as_deref().filter(|s| !s.is_empty()) {
            let dim = ratatui::style::Style::default().fg(crate::theme::current().dim);
            lines.push(Line::from(Span::styled(format!("    {note}"), dim)));
        }
    }
    lines
}

fn character_summary_lines(total: usize, written: usize, drift: usize) -> Vec<Line<'static>> {
    vec![
        Line::raw("Character analysis complete."),
        Line::raw(""),
        Line::raw(format!("Characters analyzed: {total}")),
        Line::raw(format!("Profiles written:    {written}  (bible/characters/)")),
        Line::raw(format!("Drift flags:         {drift}")),
        Line::raw(""),
        Line::raw("Open the bible files to review; the Notes section is yours to edit."),
    ]
}
