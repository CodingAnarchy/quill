//! AI-feature orchestration for [`App`]: the worker-thread request lifecycle
//! (`spawn_ai`/`poll_ai`), each tracker's analyze/apply pair, the shared
//! story-bible write+conflict+findings flow, and the bible-review / findings key
//! handlers. Split out of `app/mod.rs` to keep that module focused on core
//! editor state and key dispatch (a child module, so it keeps access to `App`'s
//! private fields). See `docs/ai-features.md`.

use super::*;

use std::sync::{Arc, Mutex};

use crate::ai::brainstorm::{self, Brainstorm};
use crate::ai::cost;
use crate::ai::fal::FalBackend;
use crate::ai::image::{self, ImageBackend, ImageReport, ImageRequest};
use crate::ai::marketing::{self, Marketing};
use crate::ai::naming::{self, Naming};
use crate::ai::scaffold::{self, Scaffold};
use crate::ai::scene_brief;
use crate::ai::social::{self, Social};
use crate::ai::style::{self, Style};
use crate::ai::submission::{self, Submission};
use crate::ai::voice;
use crate::ai::workflow_gen::{self, GeneratedWorkflow};
use crate::mode::{Action, AiAction, Command, Palette, PaletteItem, PortraitChoice};
use crate::text::count_matches;

/// The cost decision for starting a network AI action (#82): either run it now,
/// or confirm first (the estimate exceeds the threshold) with the confirm label.
pub(crate) enum AiStart {
    Run,
    Confirm(String),
}

/// The outcome of staging one generated file (#153) — a bible profile or a
/// submission artifact. Distinguishes a successful write from a hand-edit
/// conflict (queue for review) and from an I/O failure (surface to the author),
/// so a failed write is never miscounted as success.
enum Staged {
    /// Written to disk.
    Wrote,
    /// The generated block was hand-edited since the last pass; queued for review.
    Conflict(BibleConflict),
    /// The write (or its parent-directory creation) failed; carries an
    /// author-facing description.
    Failed(String),
}

/// Condense prose for an image prompt (#239): drop markdown heading lines,
/// collapse runs of whitespace, and cap at `max_chars` (on a char boundary), so a
/// whole chapter becomes a compact scene description the image model can use.
fn condense(text: &str, max_chars: usize) -> String {
    let joined = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    joined.chars().take(max_chars).collect()
}

/// The active chapter's input for a scene illustration (#247): identity, the full
/// text (to distill from), the setting bible, and a prose-excerpt fallback prompt.
struct SceneInput {
    slug: String,
    title: String,
    text: String,
    setting: String,
    fallback_prompt: String,
}

/// Build the scene image prompt (#247): distill a visual brief via the text
/// backend when one is available, else use the prose-excerpt fallback. Runs on the
/// worker thread, so any distill failure (or no text backend) degrades quietly to
/// the fallback — the verified #239 behavior.
fn scene_prompt_for(text_spec: Option<BackendSpec>, scene: &SceneInput, book: &str, model: &str) -> String {
    if let Some(spec) = text_spec {
        if let Ok(backend) = spec.build() {
            let req = scene_brief::build_request(scene.text.clone(), model);
            if let Ok(resp) = backend.complete(&req) {
                if !resp.text.trim().is_empty() {
                    return image::scene_prompt(book, &scene.title, &resp.text, &scene.setting);
                }
            }
        }
    }
    scene.fallback_prompt.clone()
}

/// Write image bytes to `path`, creating parent directories (#238). Runs on the
/// worker thread, so it returns a plain message rather than using `App`. Removes
/// any existing sibling with the same stem but a different extension first, so
/// regenerating in a different format replaces rather than accumulates (e.g. a
/// later `.jpg` doesn't leave a stale `.png` behind).
fn save_image(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    if let (Some(parent), Some(stem)) = (path.parent(), path.file_stem()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.file_stem() == Some(stem) && p != path {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
    }
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

/// A one-line status message for one or more failed writes (#153).
fn write_failure_message(failures: &[String]) -> String {
    match failures.split_first() {
        Some((first, [])) => format!("Couldn't write {first}"),
        Some((first, rest)) => format!("Couldn't write {} files — first: {first}", rest.len() + 1),
        None => String::new(),
    }
}

/// Shown when the local backend is selected but wasn't compiled in. One source
/// so the fast-fail (UI thread) and the `build_local` stub can't drift.
const LOCAL_UNAVAILABLE: &str =
    "local backend isn't in this build — rebuild Quill with `--features local`, or set `backend = \"claude\"`";

/// A resolved backend choice, ready to build on the worker thread. Built here
/// (not in `spawn_ai`) so loading the local model can't block the UI.
enum BackendSpec {
    /// The Claude HTTP backend, with its API key.
    Claude(String),
    /// The local backend, sharing `App`'s cache slot so the loaded model is
    /// reused across requests.
    Local(Arc<Mutex<Option<Arc<dyn Backend>>>>),
}

impl BackendSpec {
    /// Build (or fetch the cached) backend. Runs on the worker thread.
    fn build(self) -> Result<Arc<dyn Backend>, ai::AiError> {
        match self {
            BackendSpec::Claude(key) => Ok(Arc::new(ClaudeBackend::new(key))),
            BackendSpec::Local(slot) => build_local(slot),
        }
    }
}

/// Load the local model once and cache it in the shared slot, reusing it on later
/// requests (the model resets its KV cache each request, so reuse is correct).
#[cfg(feature = "local")]
fn build_local(slot: Arc<Mutex<Option<Arc<dyn Backend>>>>) -> Result<Arc<dyn Backend>, ai::AiError> {
    let mut guard = slot.lock().map_err(|_| ai::AiError::Api("model lock poisoned".into()))?;
    if let Some(backend) = guard.as_ref() {
        return Ok(backend.clone());
    }
    let (backend, _rec) = ai::local::LocalBackend::recommended()?;
    let backend: Arc<dyn Backend> = Arc::new(backend);
    *guard = Some(backend.clone());
    Ok(backend)
}

#[cfg(not(feature = "local"))]
fn build_local(_slot: Arc<Mutex<Option<Arc<dyn Backend>>>>) -> Result<Arc<dyn Backend>, ai::AiError> {
    Err(ai::AiError::NotConfigured(LOCAL_UNAVAILABLE.into()))
}

/// Shown when the local *image* backend is selected but wasn't compiled in (#240).
const LOCAL_IMAGE_UNAVAILABLE: &str =
    "local image backend isn't in this build — rebuild Quill with `--features local`, or set `backend = \"remote\"` under [image]";

/// A resolved image-backend choice, built on the worker thread (#238/#240) so a
/// heavy local model load can't block the UI — mirroring [`BackendSpec`].
enum ImageBackendSpec {
    /// The remote fal.ai backend, with its API key.
    Remote(String),
    /// The local Stable-Diffusion backend, sharing `App`'s cache slot so the
    /// loaded model is reused across requests.
    Local(Arc<Mutex<Option<Arc<dyn ImageBackend>>>>),
}

impl ImageBackendSpec {
    /// Build (or fetch the cached) image backend. Runs on the worker thread.
    fn build(self) -> Result<Arc<dyn ImageBackend>, ai::AiError> {
        match self {
            ImageBackendSpec::Remote(key) => Ok(Arc::new(FalBackend::new(key))),
            ImageBackendSpec::Local(slot) => build_local_image(slot),
        }
    }
}

/// Load the local image model once and cache it in the shared slot (#240).
#[cfg(feature = "local")]
fn build_local_image(
    slot: Arc<Mutex<Option<Arc<dyn ImageBackend>>>>,
) -> Result<Arc<dyn ImageBackend>, ai::AiError> {
    let mut guard = slot.lock().map_err(|_| ai::AiError::Api("image model lock poisoned".into()))?;
    if let Some(backend) = guard.as_ref() {
        return Ok(backend.clone());
    }
    let backend: Arc<dyn ImageBackend> = Arc::new(ai::local_image::LocalImageBackend::load()?);
    *guard = Some(backend.clone());
    Ok(backend)
}

#[cfg(not(feature = "local"))]
fn build_local_image(
    _slot: Arc<Mutex<Option<Arc<dyn ImageBackend>>>>,
) -> Result<Arc<dyn ImageBackend>, ai::AiError> {
    Err(ai::AiError::NotConfigured(LOCAL_IMAGE_UNAVAILABLE.into()))
}

/// Where a persistent-bible tracker keeps its prior bible, for the incremental
/// cache (#90).
enum BibleSource {
    /// A directory of files, e.g. `bible/characters/`.
    Dir(&'static str),
    /// A single file, e.g. `bible/setting.md`.
    File(&'static str),
}

/// A persistent-bible tracker's incremental-scoping descriptor (#90): its state
/// key, where its prior bible lives, and the status message for each scope.
struct BibleTracker {
    key: &'static str,
    source: BibleSource,
    full_msg: &'static str,
    incr_msg: &'static str,
}

/// The bible tracker behind a costed AI action, or `None` for findings-only and
/// interactive actions (which send the whole manuscript or nothing).
fn bible_tracker(action: AiAction) -> Option<BibleTracker> {
    let make = |key, source, full_msg, incr_msg| BibleTracker { key, source, full_msg, incr_msg };
    match action {
        AiAction::Characters => Some(make(
            "characters",
            BibleSource::Dir("characters"),
            "⟳ Analyzing characters…",
            "⟳ Updating characters from changed chapters…",
        )),
        AiAction::Continuity => Some(make(
            "timeline",
            BibleSource::File("timeline.md"),
            "⟳ Checking continuity…",
            "⟳ Updating the timeline from changed chapters…",
        )),
        AiAction::Setting => Some(make(
            "setting",
            BibleSource::File("setting.md"),
            "⟳ Building setting bible…",
            "⟳ Updating the setting bible from changed chapters…",
        )),
        _ => None,
    }
}

impl App {
    /// Open the AI command palette — the single entry point for the AI features.
    /// (The caller gates on [`ai::Settings::check_ready`].)
    pub(crate) fn open_ai_palette(&mut self) {
        let ai = |label: &str, desc: &str, a| PaletteItem { label: label.into(), desc: desc.into(), command: Command::Ai(a) };
        let items = vec![
            ai("Ask the manuscript", "Answer a question grounded in the draft", AiAction::Ask),
            ai("Character bible", "Profiles, arcs, and drift flags", AiAction::Characters),
            ai("Timeline & continuity", "Ordered timeline + contradiction findings", AiAction::Continuity),
            ai("Setting & worldbuilding", "Setting bible + rule-violation findings", AiAction::Setting),
            ai("Scaffold / outline", "Beat outline + structural gaps", AiAction::Scaffold),
            ai("Name & term consistency", "Spelling variants (cheap pass)", AiAction::Naming),
            ai("Prose & style", "POV / tense / voice findings (analysis only)", AiAction::Style),
            ai("Brainstorm ideas", "Premises, themes & conflicts from a spark (off-manuscript)", AiAction::Brainstorm),
            ai("Draft characters", "Starter cast from a concept (no manuscript needed)", AiAction::DraftCharacters),
            ai("Character voice", "Dialogue & POV voice vs. the character bible", AiAction::Voice),
            ai("Submission material", "Synopsis, blurb & query letter (off-manuscript)", AiAction::Submission),
            ai("Marketing copy", "Blurb variants & taglines (off-manuscript)", AiAction::Marketing),
            ai("Social posts", "X / Facebook / Instagram copy (off-manuscript)", AiAction::Social),
            ai("Character portraits", "Portrait images from the character bible (fal.ai)", AiAction::Portrait),
            ai("Scene illustration", "Illustrate the active chapter (fal.ai)", AiAction::Scene),
            ai("Revision triage", "All findings so far, in one list (local)", AiAction::Triage),
        ];
        self.mode = Mode::Palette(Box::new(Palette { title: "AI", items, selected: 0 }));
    }

    /// Open the unified run-action menu (#225): every registry [`Action`], run by
    /// name through the same dispatch its dedicated shortcut uses. Unavailable
    /// actions stay listed, annotated with why they can't run right now.
    pub(crate) fn open_action_palette(&mut self) {
        let items = Action::ALL
            .iter()
            .map(|&a| {
                let desc = match self.action_status(a) {
                    Some(reason) => format!("{} · unavailable: {reason}", a.name()),
                    None => a.name().to_string(),
                };
                PaletteItem { label: a.label().into(), desc, command: Command::Run(a) }
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette { title: "Run action", items, selected: 0 }));
    }

    /// Why `action` can't run right now, or `None` if it can — the one
    /// availability check the menu annotates with, [`run_action`](App::run_action)
    /// guards on, and the workflow runner (#226) treats as a step failure. The
    /// per-action runtime guards (AI cost confirm, etc.) still live at the dispatch
    /// layer; this only covers the up-front "can't start" cases.
    pub(crate) fn action_status(&self, action: Action) -> Option<String> {
        match action {
            // Triage is local aggregation — it works with the AI backend off.
            Action::Ai(AiAction::Triage) => None,
            // Image generation has its own opt-in / key, separate from the text AI
            // (and its own "local not compiled" fail-fast).
            Action::Ai(AiAction::Portrait | AiAction::Scene) => {
                self.image_backend_spec().err().map(|e| e.to_string())
            }
            Action::Ai(_) => self.ai.check_ready().err().map(|e| e.to_string()),
            // Export compiles per-chapter files; the continuous view holds an
            // unsaved window, so leave it first.
            Action::Export(_) if self.manuscript.is_some() => {
                Some("leave the continuous view (Ctrl-l) first".into())
            }
            _ => None,
        }
    }

    /// Run a registry [`Action`] (#225) through the existing code path for its
    /// kind, after checking [`action_status`](App::action_status). `force` rebuilds
    /// a bible from the full manuscript for the AI passes that support it.
    pub(crate) fn run_action(&mut self, action: Action, force: bool) {
        if let Some(reason) = self.action_status(action) {
            self.message = Some(format!("{}: {reason}", action.label()));
            return;
        }
        self.dispatch_action(action, force);
    }

    /// Run an action's effect, its availability already checked by
    /// [`action_status`](App::action_status). Shared by the run-action menu and the
    /// workflow runner (#226).
    pub(crate) fn dispatch_action(&mut self, action: Action, force: bool) {
        match action {
            Action::Save => self.save_edit(),
            Action::Export(f) => self.run_export(f),
            Action::ProgressReport => self.open_progress_report(),
            Action::AnalysisReport => self.open_analysis_report(),
            Action::Ai(a) => self.start_ai_action(a, force),
        }
    }

    pub(crate) fn on_key_palette(&mut self, key: KeyEvent) {
        let Mode::Palette(p) = &mut self.mode else {
            return;
        };
        if let Some(s) = list_move(p.selected, p.items.len(), key.code) {
            p.selected = s;
            return;
        }
        match key.code {
            // Enter runs the selected action (auto-scoped for bibles); `R` forces a
            // full-manuscript rebuild of a bible regardless of the delta (#176).
            KeyCode::Enter | KeyCode::Char('R') => {
                let force = matches!(key.code, KeyCode::Char('R'));
                let command = p.items.get(p.selected).map(|it| it.command);
                self.mode = self.editing_mode();
                match command {
                    Some(Command::Ai(a)) => self.start_ai_action(a, force),
                    Some(Command::Git(g)) => self.start_git_action(g),
                    Some(Command::Export(f)) => self.run_export(f),
                    Some(Command::Run(a)) => self.run_action(a, force),
                    Some(Command::RunWorkflow(i)) => self.run_workflow(i),
                    Some(Command::NewWorkflowFromPrompt) => self.start_describe_workflow(),
                    Some(Command::Portrait(c)) => self.confirm_portraits(c),
                    None => {}
                }
            }
            KeyCode::Esc => self.mode = Mode::Insert,
            _ => {}
        }
    }

    /// Begin an AI action chosen from the palette: for a network pass, show a
    /// cost confirm first when the estimate exceeds the threshold; the local /
    /// interactive actions (Ask, Triage) run immediately.
    pub(crate) fn start_ai_action(&mut self, action: AiAction, force: bool) {
        match self.ai_action_start(action, force) {
            AiStart::Run => self.run_ai_action(action, force),
            AiStart::Confirm(label) => {
                self.open_prompt(Pending::ConfirmAi(action, force), label, "", None)
            }
        }
    }

    /// The cost decision for a network AI action (#82): run it now, or confirm
    /// first because the estimate exceeds the threshold. Shared by the AI palette
    /// ([`start_ai_action`]) and the workflow runner (#227) so a workflow's AI
    /// step follows the same cost policy.
    pub(crate) fn ai_action_start(&self, action: AiAction, force: bool) -> AiStart {
        // The local backend is free, so there's nothing to confirm — run directly.
        if self.ai.backend() == ai::BackendKind::Local {
            return AiStart::Run;
        }
        let Some((model, max_tokens)) = self.cost_profile(action) else {
            // Interactive / local-only action — no whole-manuscript request to estimate.
            return AiStart::Run;
        };
        let est = cost::estimate(self.pass_context_len(action, force), max_tokens, model);
        if est.dollars > self.ai.cost_threshold {
            AiStart::Confirm(format!("{} — run? (y/N)", cost::label(&est, model)))
        } else {
            AiStart::Run
        }
    }

    /// The model and output bound a network pass will send — the inputs to its
    /// cost estimate, read from the same constants the request builders use so
    /// the estimate can't drift from the real request. `None` for the local /
    /// interactive actions (Ask, Triage), which issue no costed pass.
    fn cost_profile(&self, action: AiAction) -> Option<(&str, u32)> {
        match action {
            // No whole-manuscript estimate: Ask/Triage/Brainstorm are interactive
            // and send no manuscript (Brainstorm sends only a premise), and the
            // image passes (#238/#239) are billed per image with their own confirm.
            AiAction::Ask | AiAction::Triage | AiAction::Brainstorm | AiAction::DraftCharacters
            | AiAction::Portrait | AiAction::Scene => None,
            AiAction::Naming => Some((naming::MODEL, naming::MAX_TOKENS)),
            _ => Some((&self.ai.model, ai::MAX_OUTPUT_TOKENS)),
        }
    }

    /// Dispatch an AI action to its handler. Shared by the palette and the cost
    /// confirm.
    pub(crate) fn run_ai_action(&mut self, action: AiAction, force: bool) {
        match action {
            AiAction::Ask => self.open_prompt(Pending::AskAi, "Ask the manuscript: ", "", None),
            AiAction::Characters => self.analyze_characters(force),
            AiAction::Continuity => self.analyze_continuity(force),
            AiAction::Setting => self.analyze_setting(force),
            AiAction::Scaffold => self.analyze_scaffold(),
            AiAction::Naming => self.analyze_naming(),
            AiAction::Style => self.analyze_style(),
            AiAction::Brainstorm => {
                self.open_prompt(Pending::Brainstorm, "Premise or idea: ", "", None)
            }
            AiAction::DraftCharacters => {
                self.open_prompt(Pending::DraftCharacters, "Concept / premise: ", "", None)
            }
            AiAction::Voice => self.analyze_voice(),
            AiAction::Submission => self.analyze_submission(),
            AiAction::Marketing => self.analyze_marketing(),
            AiAction::Social => self.analyze_social(),
            AiAction::Portrait => self.start_portrait(),
            AiAction::Scene => self.start_scene(),
            AiAction::Triage => self.open_triage(),
        }
    }

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
        let gen = match result {
            Ok(g) => g,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
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
    fn accumulate_triage(&mut self, findings: &[Finding]) {
        for f in findings {
            if !self.triage.contains(f) {
                self.triage.push(f.clone());
            }
        }
    }
    // --- AI: "Ask the manuscript" -----------------------------------------

    /// Assemble the chapters for which `keep(i)` holds, each under its `# Title`
    /// heading (active chapter live from its buffer, the rest from disk) — the one
    /// serialization shared by the whole-manuscript and changed-chapter contexts.
    fn render_chapters(&self, keep: impl Fn(usize) -> bool) -> String {
        let mut out = String::new();
        for i in 0..self.chapter_count() {
            if !keep(i) {
                continue;
            }
            if let Some(title) = self.project.chapter_title(i) {
                out.push_str("# ");
                out.push_str(title);
                out.push_str("\n\n");
            }
            out.push_str(self.chapter_text(i).trim_end());
            out.push_str("\n\n");
        }
        out
    }

    /// The whole manuscript as plain text for an AI request.
    pub(crate) fn manuscript_text(&self) -> String {
        self.render_chapters(|_| true)
    }

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
        let t = bible_tracker(AiAction::Characters).expect("characters is a bible tracker");
        self.run_bible_tracker(t, force, character::build_request, character::build_incremental_request, |r| {
            Outcome::Characters(r.and_then(|resp| character::parse(&resp.text)))
        });
    }

    /// Start a timeline & continuity pass (incremental when possible, #90); the
    /// timeline + findings are applied by [`App::poll_ai`].
    pub(crate) fn analyze_continuity(&mut self, force: bool) {
        let t = bible_tracker(AiAction::Continuity).expect("continuity is a bible tracker");
        self.run_bible_tracker(t, force, continuity::build_request, continuity::build_incremental_request, |r| {
            Outcome::Continuity(r.and_then(|resp| continuity::parse(&resp.text)))
        });
    }

    /// Start a setting & worldbuilding pass (incremental when possible, #90); the
    /// setting bible + rule findings are applied by [`App::poll_ai`].
    pub(crate) fn analyze_setting(&mut self, force: bool) {
        let t = bible_tracker(AiAction::Setting).expect("setting is a bible tracker");
        self.run_bible_tracker(t, force, setting::build_request, setting::build_incremental_request, |r| {
            Outcome::Setting(r.and_then(|resp| setting::parse(&resp.text)))
        });
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
        let premise = premise.trim().to_string();
        if premise.is_empty() {
            return;
        }
        let model = self.ai.model.clone();
        self.spawn_outcome("⟳ Brainstorming…", move |backend| {
            let req = brainstorm::build_request(&premise, &model);
            Outcome::Brainstorm(backend.complete(&req).and_then(|r| brainstorm::parse(&r.text)))
        });
    }

    /// Draft a starter character cast from a concept (#256) — generative, no
    /// manuscript. Reuses the extractive pass's `Outcome::Characters` so the
    /// profiles are written to `bible/characters/` by [`apply_characters`].
    pub(crate) fn draft_characters(&mut self, concept: &str) {
        let concept = concept.trim().to_string();
        if concept.is_empty() {
            return;
        }
        let model = self.ai.model.clone();
        self.spawn_outcome("⟳ Drafting characters…", move |backend| {
            let req = character::build_concept_request(&concept, &model);
            Outcome::Characters(backend.complete(&req).and_then(|r| character::parse(&r.text)))
        });
    }

    /// Apply a brainstorm pass (#255): write the idea notes to `bible/brainstorm.md`,
    /// conflict-reviewed and off-manuscript (never a chapter file, never prose).
    pub(crate) fn apply_brainstorm(&mut self, result: Result<Brainstorm, ai::AiError>) {
        self.overlay = None;
        let ideas = match result {
            Ok(b) => b,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
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

    /// Begin a character-portrait pass (#238): gate on image readiness and the
    /// character bible, then open the picker so the author chooses which
    /// character(s) to generate (#244).
    pub(crate) fn start_portrait(&mut self) {
        if let Err(e) = self.image_backend_spec() {
            self.message = Some(e.to_string());
            return;
        }
        let targets = self.character_portrait_targets();
        if targets.is_empty() {
            self.message = Some("Run the character pass first to build the character bible".into());
            return;
        }
        // Lead with "all", then one item per character. The choice carries the
        // index; the targets are recomputed (stable order) when it's confirmed.
        let mut items = vec![PaletteItem {
            label: format!("All characters ({})", targets.len()),
            desc: "a portrait for every character".into(),
            command: Command::Portrait(PortraitChoice::All),
        }];
        items.extend(targets.iter().enumerate().map(|(i, (_, name, _))| PaletteItem {
            label: name.clone(),
            desc: "a portrait for this character".into(),
            command: Command::Portrait(PortraitChoice::One(i)),
        }));
        self.mode = Mode::Palette(Box::new(Palette { title: "Character portrait", items, selected: 0 }));
    }

    /// Confirm the picked portrait selection (#244) before spending: stash the
    /// chosen targets and open the cost confirm, whose count reflects the choice.
    pub(crate) fn confirm_portraits(&mut self, choice: PortraitChoice) {
        let targets = self.character_portrait_targets();
        let selected = match choice {
            PortraitChoice::All => targets,
            PortraitChoice::One(i) => targets.into_iter().nth(i).map(|t| vec![t]).unwrap_or_default(),
        };
        if selected.is_empty() {
            return;
        }
        let n = selected.len();
        let plural = if n == 1 { "" } else { "s" };
        let label = format!(
            "Generate {n} character portrait{plural} via fal.ai ({}) — each image is billed. Run? (y/N)",
            self.image.model,
        );
        self.pending_portraits = Some(selected);
        self.open_prompt(Pending::ConfirmPortraits, label, "", None);
    }

    /// Generate the picked character portraits off the UI thread (#238/#244),
    /// saved under `marketing/images/characters/`. The result is applied by
    /// [`poll_ai`](App::poll_ai). A no-op if the selection was cleared.
    pub(crate) fn generate_portraits(&mut self) {
        if let Some(targets) = self.pending_portraits.take() {
            self.spawn_image_job("marketing/images/characters", "⟳ Generating portraits…", targets);
        }
    }

    /// Run an image job off the UI thread (#238/#239): generate one image per
    /// `(slug, label, prompt)` target via the image backend and save each under
    /// `<subdir>/<slug>.<ext>`, reporting saved files and per-target failures. The
    /// shared worker behind the portrait and scene passes. A no-op for no targets;
    /// the busy/key guards mirror the text spawn path.
    fn spawn_image_job(&mut self, subdir: &'static str, message: &str, targets: Vec<(String, String, String)>) {
        if targets.is_empty() {
            return;
        }
        if self.ai_task.is_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        // Resolve the backend choice on the UI thread (gating + fail-fast for an
        // uncompiled local model); the heavy local load happens on the worker.
        let spec = match self.image_backend_spec() {
            Ok(spec) => spec,
            Err(e) => {
                self.message = Some(e.to_string());
                return;
            }
        };
        let model = self.image.model.clone();
        let dir = self.project.root.join(subdir);
        self.ai_task.spawn(move || {
            let backend = match spec.build() {
                Ok(b) => b,
                Err(e) => return Outcome::Image(Err(e)),
            };
            let mut report = ImageReport::default();
            for (slug, label, prompt) in targets {
                match backend.generate(&ImageRequest { prompt, model: model.clone() }) {
                    Ok(img) => {
                        let file = format!("{slug}.{}", img.ext);
                        match save_image(&dir.join(&file), &img.bytes) {
                            Ok(()) => report.saved.push(format!("{subdir}/{file}")),
                            Err(e) => report.failed.push(format!("{label}: {e}")),
                        }
                    }
                    Err(e) => report.failed.push(format!("{label}: {e}")),
                }
            }
            Outcome::Image(Ok(report))
        });
        self.message = Some(message.to_string());
    }

    /// Resolve the configured image backend (#238/#240) for building on the worker:
    /// gate on readiness, supply the remote key, or fail fast when the local model
    /// is selected but not compiled in.
    fn image_backend_spec(&self) -> Result<ImageBackendSpec, ai::AiError> {
        self.image.check_ready()?;
        Ok(match self.image.backend() {
            ai::image::ImageBackendKind::Remote => {
                let key = self
                    .image
                    .resolved_key()
                    .ok_or_else(|| ai::AiError::NotConfigured("no image API key".into()))?;
                ImageBackendSpec::Remote(key.to_string())
            }
            ai::image::ImageBackendKind::Local => {
                if cfg!(not(feature = "local")) {
                    return Err(ai::AiError::NotConfigured(LOCAL_IMAGE_UNAVAILABLE.into()));
                }
                ImageBackendSpec::Local(self.image_backend.clone())
            }
        })
    }

    /// The portrait targets — `(slug, name, prompt)` per character in the bible,
    /// the prompt built from that character's profile. Empty when no bible exists.
    fn character_portrait_targets(&self) -> Vec<(String, String, String)> {
        let dir = self.project.root.join("bible").join("characters");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut files: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
            .collect();
        files.sort();
        let mut out = Vec::new();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let slug = path.file_stem().and_then(|s| s.to_str()).unwrap_or("character").to_string();
            let name = bible::front_field(&text, "name")
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| slug.clone());
            let description = crate::frontmatter::split(&text).1;
            out.push((slug, name.clone(), image::portrait_prompt(&name, description)));
        }
        out
    }

    /// Begin a scene-illustration pass (#239): gate on image readiness and the
    /// active chapter having text, then confirm before spending.
    pub(crate) fn start_scene(&mut self) {
        if let Err(e) = self.image_backend_spec() {
            self.message = Some(e.to_string());
            return;
        }
        let Some(scene) = self.active_scene() else {
            self.message = Some("The active chapter has no text to illustrate".into());
            return;
        };
        let label = format!(
            "Illustrate \"{}\" via fal.ai ({}) — the image is billed. Run? (y/N)",
            scene.title, self.image.model,
        );
        self.open_prompt(Pending::ConfirmScene, label, "", None);
    }

    /// Generate a scene illustration of the active chapter off the UI thread
    /// (#239/#247), saved under `marketing/images/scenes/`. When the text AI is
    /// ready it first distills a visual brief of the chapter's pivotal moment
    /// (#247); otherwise it falls back to the prose-excerpt prompt. Applied by
    /// [`poll_ai`](App::poll_ai).
    pub(crate) fn generate_scene(&mut self) {
        if self.ai_task.is_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        let Some(scene) = self.active_scene() else {
            return;
        };
        let image_spec = match self.image_backend_spec() {
            Ok(spec) => spec,
            Err(e) => {
                self.message = Some(e.to_string());
                return;
            }
        };
        // Optional distill step: the text backend, if it's configured. None → the
        // worker uses the prose-excerpt fallback prompt.
        let text_spec = self.backend_spec().ok();
        let book = self.project.manifest.title.clone();
        let text_model = self.ai.model.clone();
        let image_model = self.image.model.clone();
        let subdir = "marketing/images/scenes";
        let dir = self.project.root.join(subdir);
        self.ai_task.spawn(move || {
            let backend = match image_spec.build() {
                Ok(b) => b,
                Err(e) => return Outcome::Image(Err(e)),
            };
            let prompt = scene_prompt_for(text_spec, &scene, &book, &text_model);
            let mut report = ImageReport::default();
            match backend.generate(&ImageRequest { prompt, model: image_model }) {
                Ok(img) => {
                    let file = format!("{}.{}", scene.slug, img.ext);
                    match save_image(&dir.join(&file), &img.bytes) {
                        Ok(()) => report.saved.push(format!("{subdir}/{file}")),
                        Err(e) => report.failed.push(format!("{}: {e}", scene.title)),
                    }
                }
                Err(e) => report.failed.push(format!("{}: {e}", scene.title)),
            }
            Outcome::Image(Ok(report))
        });
        self.message = Some("⟳ Illustrating the scene…".into());
    }

    /// The active chapter's scene input (#247): its slug, title, full text, the
    /// setting bible, and a prose-excerpt fallback prompt. `None` when the chapter
    /// has no prose.
    fn active_scene(&self) -> Option<SceneInput> {
        let idx = self.project.active_index();
        let text = self.chapter_text(idx);
        let excerpt = condense(&text, 600);
        if excerpt.is_empty() {
            return None;
        }
        let title = self.project.chapter_title(idx).unwrap_or("Scene").to_string();
        let slug = std::path::Path::new(self.project.chapter_file(idx))
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("scene")
            .to_string();
        let setting = condense(crate::frontmatter::split(&bible::raw_file(&self.project.root, "setting.md")).1, 400);
        let fallback_prompt = image::scene_prompt(&self.project.manifest.title, &title, &excerpt, &setting);
        Some(SceneInput { slug, title, text, setting, fallback_prompt })
    }

    /// Apply an image pass (#238/#239): summarize the saved images and any
    /// failures. Shared by the portrait and scene passes (both `Outcome::Image`).
    pub(crate) fn apply_images(&mut self, result: Result<ImageReport, ai::AiError>) {
        self.overlay = None;
        let report = match result {
            Ok(r) => r,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
        };
        if report.saved.is_empty() && report.failed.is_empty() {
            self.message = Some("No images generated".into());
            return;
        }
        let mut lines = Vec::new();
        if !report.saved.is_empty() {
            lines.push(Line::raw("Images generated:"));
            lines.push(Line::raw(""));
            lines.extend(report.saved.iter().map(|s| Line::raw(format!("  {s}"))));
            lines.push(Line::raw(""));
            lines.push(Line::raw("Off-manuscript image files — yours to use and regenerate."));
        }
        if !report.failed.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::raw(format!("{} failed:", report.failed.len())));
            lines.extend(report.failed.iter().map(|f| Line::raw(format!("  {f}"))));
        }
        self.show_ai(lines);
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

    /// Run an AI request on a worker thread: assemble the manuscript here, hand it
    /// (with a backend + model) to `run` off-thread, and deliver the `Outcome` via
    /// `ai_rx` for [`App::poll_ai`]. A no-op with a status message when AI isn't
    /// ready or a request is already in flight.
    fn spawn_ai(
        &mut self,
        message: &str,
        run: impl FnOnce(&dyn Backend, String, String) -> Outcome + Send + 'static,
    ) {
        let model = self.ai.model.clone();
        let manuscript = self.manuscript_text();
        self.spawn_outcome(message, move |backend| run(backend, manuscript, model));
    }

    /// The shared worker-spawn lifecycle for an AI request: bail if one's already
    /// in flight, resolve the backend (gating readiness on the UI thread; the
    /// heavy build happens off-thread), then run `produce` to an `Outcome`.
    fn spawn_outcome(
        &mut self,
        message: &str,
        produce: impl FnOnce(&dyn Backend) -> Outcome + Send + 'static,
    ) {
        if self.ai_task.is_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        let spec = match self.backend_spec() {
            Ok(spec) => spec,
            Err(e) => {
                self.message = Some(e.to_string());
                return;
            }
        };
        self.ai_task.spawn(move || match spec.build() {
            Ok(backend) => produce(backend.as_ref()),
            Err(e) => Outcome::Error(e),
        });
        self.message = Some(message.to_string());
    }

    /// Read a tracker's prior bible and decide its scope from the revision delta
    /// (#90) — shared by the cost estimate and the run so they always agree. A
    /// missing/empty bible can't be revised and isn't truly "up to date" (e.g. it
    /// was hand-deleted while the state file lingered), so it rebuilds in full.
    fn plan_bible_pass(&self, t: &BibleTracker) -> (ai::incremental::Scope, String, ai::incremental::Digests) {
        let prior = match t.source {
            BibleSource::Dir(dir) => bible::raw_dir(&self.project.root, dir),
            BibleSource::File(file) => bible::raw_file(&self.project.root, file),
        };
        let current = self.chapter_digests();
        let scope = if prior.trim().is_empty() {
            ai::incremental::Scope::Full
        } else {
            let state = ai::incremental::State::load(&self.project.root);
            ai::incremental::decide(state.built_from(t.key), &current, self.ai.refresh_after_changes)
        };
        (scope, prior, current)
    }

    /// Run a persistent-bible tracker with incremental scoping (#90): revise the
    /// bible from only the changed chapters, rebuild it from the whole manuscript,
    /// or skip (already current). `full`/`incr` build the request for each case.
    fn run_bible_tracker(
        &mut self,
        t: BibleTracker,
        force: bool,
        full: impl FnOnce(String, &str) -> ai::Request,
        incr: impl FnOnce(&str, &str, &str) -> ai::Request,
        finish: impl FnOnce(Result<ai::Response, ai::AiError>) -> Outcome + Send + 'static,
    ) {
        let (scope, prior, current) = self.plan_bible_pass(&t);
        // The AI palette's `R` forces a full rebuild regardless of the delta (#176).
        let scope = if force { ai::incremental::Scope::Full } else { scope };
        // Full rebuilds run through the Batch API (half price, higher latency) on
        // the Claude backend; the local backend has no batch path.
        let claude = self.ai.backend() == ai::BackendKind::Claude;
        let model = self.ai.model.clone();
        let (req, base_msg, batch) = match scope {
            ai::incremental::Scope::UpToDate => {
                self.message =
                    Some(format!("{}: bible is current — no chapters changed since the last pass.", t.key));
                return;
            }
            ai::incremental::Scope::Full => (full(self.manuscript_text(), &model), t.full_msg, claude),
            ai::incremental::Scope::Incremental(changed) => {
                (incr(&prior, &self.chapters_text(&changed), &model), t.incr_msg, false)
            }
        };
        // Snapshot the manuscript state now; recorded once the pass applies cleanly.
        self.pending_pass = Some((t.key.to_string(), current));
        let message = if batch {
            format!("{base_msg} (batch — may take a few minutes)")
        } else {
            base_msg.to_string()
        };
        self.spawn_request(&message, req, batch, finish);
    }

    /// Run a pre-built request on a worker thread (#90), delivering `finish`'s
    /// `Outcome` via `ai_rx`. Like [`App::spawn_ai`], but the request is assembled
    /// by the caller (so it can be a full or an incremental pass).
    fn spawn_request(
        &mut self,
        message: &str,
        req: ai::Request,
        batch: bool,
        finish: impl FnOnce(Result<ai::Response, ai::AiError>) -> Outcome + Send + 'static,
    ) {
        self.spawn_outcome(message, move |backend| {
            let result = if batch { backend.complete_batched(&req) } else { backend.complete(&req) };
            finish(result)
        });
    }

    /// The chapter `file → content-digest` map for the manuscript as it is now —
    /// the revision-delta baseline for incremental scoping (#90).
    pub(crate) fn chapter_digests(&self) -> ai::incremental::Digests {
        (0..self.chapter_count())
            .filter_map(|i| {
                let file = self.project.chapter_file(i);
                (!file.is_empty())
                    .then(|| (file.to_string(), ai::incremental::digest(&self.chapter_text(i))))
            })
            .collect()
    }

    /// The text of the chapters whose files are in `files`, each under its title
    /// heading — the changed-chapter context for an incremental pass (#90).
    pub(crate) fn chapters_text(&self, files: &[String]) -> String {
        self.render_chapters(|i| files.iter().any(|f| f == self.project.chapter_file(i)))
    }

    /// Record what the just-applied bible pass was built from (#90), so the next
    /// run only re-analyzes what changed. Best effort — a state-write failure
    /// doesn't undo the pass that already succeeded. A no-op for non-bible passes
    /// (which leave `pending_pass` unset).
    fn record_pass_state(&mut self) {
        if let Some((tracker, digests)) = self.pending_pass.take() {
            let today = date::iso(date::today());
            let _ = ai::incremental::State::record(&self.project.root, &tracker, &today, digests);
        }
    }

    /// The byte length of the context a costed pass will actually send — for the
    /// cost estimate (#90). The whole manuscript for most passes, but only the
    /// bible + changed chapters for an incremental tracker pass, and zero when its
    /// bible is already current (so an up-to-date pass never prompts a confirm).
    pub(crate) fn pass_context_len(&self, action: AiAction, force: bool) -> usize {
        let Some(t) = bible_tracker(action) else {
            return self.manuscript_text().len();
        };
        if force {
            return self.manuscript_text().len(); // a forced rebuild sends the whole book
        }
        let (scope, prior, _) = self.plan_bible_pass(&t);
        match scope {
            ai::incremental::Scope::UpToDate => 0,
            ai::incremental::Scope::Full => self.manuscript_text().len(),
            ai::incremental::Scope::Incremental(changed) => prior.len() + self.chapters_text(&changed).len(),
        }
    }

    /// Choose the backend for a request from settings (after the readiness gate).
    /// Cheap — the heavy local-model load is deferred to [`BackendSpec::build`] on
    /// the worker thread.
    fn backend_spec(&self) -> Result<BackendSpec, ai::AiError> {
        self.ai.check_ready()?;
        Ok(match self.ai.backend() {
            ai::BackendKind::Claude => {
                // check_ready already guaranteed a key for Claude; resolve it
                // defensively rather than unwrapping.
                let key = self
                    .ai
                    .resolved_key()
                    .ok_or_else(|| ai::AiError::NotConfigured("no API key".into()))?;
                BackendSpec::Claude(key.to_string())
            }
            ai::BackendKind::Local => {
                // Fail fast on the UI thread when the local backend wasn't compiled
                // in, so we don't spawn a worker just to error.
                if cfg!(not(feature = "local")) {
                    return Err(ai::AiError::NotConfigured(LOCAL_UNAVAILABLE.into()));
                }
                BackendSpec::Local(self.local_backend.clone())
            }
        })
    }

    /// Poll the in-flight AI request (non-blocking). On a result, dispatch by
    /// feature and clear the pending state. Called once per loop iteration so the
    /// result lands within the poll window.
    pub fn poll_ai(&mut self) {
        // A stuck AI step shouldn't wedge a running workflow (#227).
        self.tick_workflow_timeout();
        match self.ai_task.poll() {
            Poll::Ready(outcome) => {
                self.message = None;
                self.overlay = None; // a fresh result supersedes any open overlay
                let step_error = outcome.error().map(|e| e.to_string());
                match outcome {
                    Outcome::Answer(r) => self.apply_qa_result(r),
                    Outcome::Characters(r) => self.apply_characters(r),
                    Outcome::Continuity(r) => self.apply_continuity(r),
                    Outcome::Setting(r) => self.apply_setting(r),
                    Outcome::Scaffold(r) => self.apply_scaffold(r),
                    Outcome::Naming(r) => self.apply_naming(r),
                    Outcome::Style(r) => self.apply_style(r),
                    Outcome::Submission(r) => self.apply_submission(r),
                    Outcome::Brainstorm(r) => self.apply_brainstorm(r),
                    Outcome::Marketing(r) => self.apply_marketing(r),
                    Outcome::Social(r) => self.apply_social(r),
                    Outcome::Voice(r) => self.apply_voice(r),
                    Outcome::Image(r) => self.apply_images(r),
                    Outcome::Workflow(r) => self.apply_generated_workflow(r),
                    Outcome::Error(e) => self.message = Some(format!("AI: {e}")),
                }
                // The in-flight pass is done; a clean bible apply already recorded
                // its state (#90), so drop any snapshot a failed/conflicted one left.
                self.pending_pass = None;
                // If this AI request was a workflow step, advance the workflow.
                self.advance_after_ai(step_error);
            }
            Poll::Pending => {}
            Poll::Died => {
                self.message = Some("AI: request failed".into());
                self.advance_after_ai(Some("request failed".into()));
            }
        }
    }

    /// Show an AI result — a Q&A answer or a pass summary — as the full-screen
    /// overlay, scrolled to the top. The single place an AI result becomes an
    /// overlay.
    fn show_ai(&mut self, lines: Vec<Line<'static>>) {
        self.overlay = Some(Overlay::ai(lines));
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
        let mut chars = match result {
            Ok(c) => c,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
        };
        if chars.is_empty() {
            self.message = Some("No characters found.".into());
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

    /// Stage a generated file: write it, or report a hand-edit conflict, or
    /// report an I/O failure (#153). `render` receives the author's preserved
    /// Notes block and returns the full file content. Shared by the character,
    /// continuity/setting/scaffold, and submission passes.
    fn stage_bible(&self, path: PathBuf, name: &str, render: impl Fn(&str) -> String) -> Staged {
        let existing = std::fs::read_to_string(&path).ok();
        let notes = bible::extract_notes(existing.as_deref().unwrap_or(""));
        let proposed = render(&notes);
        if let Some(current) = &existing {
            if bible::is_hand_edited(current) {
                let diff = unified_diff(current, &proposed);
                return Staged::Conflict(BibleConflict { path, name: name.to_string(), proposed, diff });
            }
        }
        match self.write_file(&path, &proposed) {
            Ok(()) => Staged::Wrote,
            Err(msg) => Staged::Failed(msg),
        }
    }

    /// Write `content` to `path`, creating parent directories. On failure returns
    /// an author-facing message (the project-relative path plus the OS error), so
    /// the caller can surface it instead of silently dropping the write (#153).
    fn write_file(&self, path: &std::path::Path, content: &str) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", self.display_path(path)))?;
        }
        std::fs::write(path, content).map_err(|e| format!("{}: {e}", self.display_path(path)))
    }

    /// A project-relative label for `path`, for author-facing messages.
    fn display_path(&self, path: &std::path::Path) -> String {
        path.strip_prefix(&self.project.root).unwrap_or(path).display().to_string()
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
        let cont = match result {
            Ok(c) => c,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
        };
        let findings = cont
            .findings
            .iter()
            .map(|f| Finding {
                summary: format!("{}: {}", f.kind, f.detail),
                chapters: f.chapters.clone(),
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
        let setting = match result {
            Ok(s) => s,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
        };
        let findings = setting
            .findings
            .iter()
            .map(|f| Finding {
                summary: format!("{}: {}", f.rule, f.violation),
                chapters: f.chapters.clone(),
            })
            .collect();
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("setting.md");
        self.apply_bible_with_findings(path, "setting", |notes| {
            setting::render(&setting, notes, &today)
        }, findings);
    }

    /// Apply a scaffolding pass: write the beat outline to `bible/outline.md` and
    /// surface the structural gaps as findings.
    pub(crate) fn apply_scaffold(&mut self, result: Result<Scaffold, ai::AiError>) {
        let scaffold = match result {
            Ok(s) => s,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
        };
        let findings = scaffold
            .gaps
            .iter()
            .map(|g| Finding {
                summary: format!("gap: {}", g.detail),
                chapters: vec![g.chapter.clone()],
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
        let naming = match result {
            Ok(n) => n,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
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
            })
            .collect();
        self.show_findings(findings);
    }

    /// Apply a prose/style pass: surface the observations as findings. Pure
    /// analysis — nothing is written.
    pub(crate) fn apply_style(&mut self, result: Result<Style, ai::AiError>) {
        let style = match result {
            Ok(s) => s,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
        };
        let findings = style
            .findings
            .iter()
            .map(|f| Finding {
                summary: format!("{}: {}", f.kind, f.detail),
                chapters: f.chapters.clone(),
            })
            .collect();
        self.show_findings(findings);
    }

    /// Apply a character-voice pass (#183): surface the observations as findings,
    /// naming the character each concerns. Pure analysis — nothing is written.
    pub(crate) fn apply_voice(&mut self, result: Result<voice::Voice, ai::AiError>) {
        let voice = match result {
            Ok(v) => v,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
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

    /// Open a findings-only result (analysis passes that write no bible file):
    /// clear any prior overlay, stash the findings, and open the overlay.
    fn show_findings(&mut self, findings: Vec<Finding>) {
        self.overlay = None;
        self.accumulate_triage(&findings);
        self.pending_findings = findings;
        self.open_findings();
    }

    /// Write a single bible file and surface a set of findings — the shared shape
    /// of the continuity and setting passes. Clears any prior overlay, stashes the
    /// findings, writes the file, then opens the findings overlay — or, if the
    /// file was hand-edited, reviews that conflict first (`advance_bible_review`
    /// flushes the findings when the review queue drains).
    fn apply_bible_with_findings(
        &mut self,
        path: PathBuf,
        name: &str,
        render: impl Fn(&str) -> String,
        findings: Vec<Finding>,
    ) {
        self.overlay = None;
        self.accumulate_triage(&findings);
        self.pending_findings = findings;
        match self.stage_bible(path, name, render) {
            Staged::Wrote => {
                self.record_pass_state(); // the bible now reflects this manuscript state (#90)
                self.open_findings();
            }
            Staged::Conflict(conflict) => {
                self.mode = Mode::BibleReview(Box::new(BibleReview {
                    conflicts: vec![conflict],
                    editing: None,
                }));
                self.message = Some(format!("{name} edited — review, then findings"));
            }
            Staged::Failed(msg) => {
                // The analysis still holds, so show the findings — but report the
                // failed write instead of claiming it was saved to the bible.
                self.message = Some(write_failure_message(&[msg]));
                let items = std::mem::take(&mut self.pending_findings);
                if !items.is_empty() {
                    self.mode = Mode::Findings(Box::new(Findings { items, selected: 0 }));
                }
            }
        }
    }

    /// Open the stashed findings as a triage overlay, or summarize when there are
    /// none (the bible file has already been written).
    fn open_findings(&mut self) {
        let items = std::mem::take(&mut self.pending_findings);
        if items.is_empty() {
            self.show_ai(vec![
                Line::raw("Analysis complete."),
                Line::raw(""),
                Line::raw("Written to the story bible. No findings."),
            ]);
        } else {
            self.mode = Mode::Findings(Box::new(Findings { items, selected: 0 }));
        }
    }

    /// Whether an AI request is currently in flight (for the status indicator).
    pub fn ai_busy(&self) -> bool {
        self.ai_task.is_busy()
    }

    // --- Story-bible conflict review --------------------------------------

    pub(crate) fn on_key_bible_review(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let editing = matches!(&self.mode, Mode::BibleReview(r) if r.editing.is_some());

        if editing {
            // Editing the proposed version before saving (same text keys as
            // EditMeta, with bible-specific Ctrl-S / Esc).
            match (ctrl, key.code) {
                (true, KeyCode::Char('s')) => self.save_bible_edit(),
                (_, KeyCode::Esc) => {
                    if let Mode::BibleReview(r) = &mut self.mode {
                        r.editing = None; // back to the diff for this conflict
                    }
                }
                _ => {
                    self.handle_text_edit_key(key);
                }
            }
            return;
        }

        // Diff view: choose how to resolve the front conflict.
        match key.code {
            KeyCode::Char('o') => {
                let failed = if let Mode::BibleReview(r) = &self.mode {
                    r.conflicts
                        .first()
                        .and_then(|front| self.write_file(&front.path, &front.proposed).err())
                } else {
                    None
                };
                // A failed overwrite keeps the conflict (so the author can retry)
                // and reports the error rather than silently advancing.
                if let Some(msg) = failed {
                    self.message = Some(write_failure_message(&[msg]));
                } else {
                    self.advance_bible_review();
                }
            }
            KeyCode::Char('k') => self.advance_bible_review(),
            KeyCode::Char('e') => {
                if let Mode::BibleReview(r) = &mut self.mode {
                    if let Some(front) = r.conflicts.first() {
                        let buf = Buffer::from_text(&front.proposed);
                        r.editing = Some(buf);
                    }
                }
            }
            KeyCode::Esc => {
                self.mode = self.editing_mode();
                self.message = Some("Review cancelled — kept your versions".into());
            }
            _ => {}
        }
    }

    /// Persist the edited proposed version, then advance to the next conflict. A
    /// failed write keeps the conflict and reports the error (#153).
    fn save_bible_edit(&mut self) {
        let failed = if let Mode::BibleReview(r) = &self.mode {
            r.conflicts
                .first()
                .zip(r.editing.as_ref())
                .and_then(|(front, buf)| self.write_file(&front.path, &buf.text()).err())
        } else {
            None
        };
        if let Some(msg) = failed {
            self.message = Some(write_failure_message(&[msg]));
            return; // leave the editor open on the unsaved conflict
        }
        if let Mode::BibleReview(r) = &mut self.mode {
            r.editing = None;
        }
        self.advance_bible_review();
    }

    /// Drop the resolved front conflict; close the review when the queue empties.
    fn advance_bible_review(&mut self) {
        if let Mode::BibleReview(r) = &mut self.mode {
            if !r.conflicts.is_empty() {
                r.conflicts.remove(0);
            }
            if !r.conflicts.is_empty() {
                return;
            }
        }
        // Queue drained. A continuity pass leaves findings stashed — show them now;
        // a character pass leaves none, so this falls back to insert mode.
        if self.pending_findings.is_empty() {
            self.mode = self.editing_mode();
            self.message = Some("Bible review complete".into());
        } else {
            self.open_findings();
        }
    }

    pub(crate) fn on_key_findings(&mut self, key: KeyEvent) {
        let Mode::Findings(f) = &mut self.mode else {
            return;
        };
        if let Some(s) = list_move(f.selected, f.items.len(), key.code) {
            f.selected = s;
            return;
        }
        match key.code {
            KeyCode::Char('d') => {
                if f.selected < f.items.len() {
                    f.items.remove(f.selected);
                    if f.items.is_empty() {
                        self.mode = self.editing_mode();
                    } else {
                        f.selected = f.selected.min(f.items.len() - 1);
                    }
                }
            }
            KeyCode::Enter => {
                let target = f.items.get(f.selected).and_then(|it| it.chapters.first()).cloned();
                self.mode = self.editing_mode();
                if let Some(title) = target {
                    if let Some(i) = (0..self.chapter_count()).find(|&i| self.project.chapter_title(i) == Some(title.as_str())) {
                        self.activate_chapter(i);
                    } else {
                        self.message = Some(format!("Chapter '{title}' not found"));
                    }
                }
            }
            // Turn the finding into an `xxx` TODO marker in its chapter (a note,
            // not prose). The keypress is the confirmation.
            KeyCode::Char('i') => {
                if let Some(item) = f.items.get(f.selected) {
                    let chapter = item.chapters.first().cloned();
                    let note = item.summary.clone();
                    if let Some(title) = chapter {
                        self.insert_todo(&title, &note);
                    } else {
                        self.message = Some("No chapter to mark".into());
                    }
                }
            }
            KeyCode::Esc => self.mode = Mode::Insert,
            _ => {}
        }
    }

    /// Append an `xxx — <note>` TODO marker to the body of the chapter titled
    /// `title` (a note for the author; never prose). The active chapter is
    /// flushed and reloaded so its open buffer sees the marker; other chapters
    /// are appended on disk.
    fn insert_todo(&mut self, title: &str, note: &str) {
        let Some(i) = (0..self.chapter_count())
            .find(|&i| self.project.chapter_title(i) == Some(title))
        else {
            self.message = Some(format!("Chapter '{title}' not found"));
            return;
        };
        let marker = format!("\n\nxxx — {note}\n");

        if i == self.project.active_index() {
            // Persist current edits, append, and reload so the buffer is current.
            if self.flush_active().is_err() {
                return;
            }
        }
        let path = self.project.chapter_path(i);
        let mut body = std::fs::read_to_string(&path).unwrap_or_default();
        body.push_str(&marker);
        if std::fs::write(&path, body).is_err() {
            self.message = Some("Failed to write marker".into());
            return;
        }
        if i == self.project.active_index() {
            // Reload so the open buffer shows the marker and isn't overwritten on
            // the next save.
            if let Ok(buffer) = Buffer::from_path(&path) {
                self.buffer = buffer;
            }
            self.cache_focus_words();
        }
        // Non-active chapters: the marker is on disk; the report's `xxx` count
        // reads bodies live, and leaving the word cache untouched keeps an
        // AI-inserted note out of the author's word tally.
        self.message = Some(format!("Marked xxx in '{title}'"));
    }
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

fn unified_diff(current: &str, proposed: &str) -> Vec<String> {
    TextDiff::from_lines(current, proposed)
        .iter_all_changes()
        .map(|change| {
            let sign = match change.tag() {
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
                ChangeTag::Equal => ' ',
            };
            format!("{sign}{}", change.value().trim_end_matches('\n'))
        })
        .collect()
}

/// The summary overlay shown after a clean character pass (no conflicts).
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
