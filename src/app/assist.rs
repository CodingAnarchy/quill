//! AI-feature orchestration for [`App`]: the worker-thread request lifecycle
//! (`spawn_ai`/`poll_ai`), each tracker's analyze/apply pair, the shared
//! story-bible write+conflict+findings flow, and the bible-review / findings key
//! handlers. Split out of `app/mod.rs` to keep that module focused on core
//! editor state and key dispatch (a child module, so it keeps access to `App`'s
//! private fields). See `docs/ai-features.md`.

use super::*;

use std::sync::{Arc, Mutex};

use crate::ai::cost;
use crate::ai::naming::{self, Naming};
use crate::ai::scaffold::{self, Scaffold};
use crate::ai::style::{self, Style};
use crate::ai::submission::{self, Submission};
use crate::mode::{AiAction, Command, Palette, PaletteItem};
use crate::text::count_matches;

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

impl App {
    /// Open the AI command palette — the single entry point for the AI features.
    /// (The caller gates on [`ai::Settings::check_ready`].)
    pub(crate) fn open_ai_palette(&mut self) {
        let ai = |label, desc, a| PaletteItem { label, desc, command: Command::Ai(a) };
        let items = vec![
            ai("Ask the manuscript", "Answer a question grounded in the draft", AiAction::Ask),
            ai("Character bible", "Profiles, arcs, and drift flags", AiAction::Characters),
            ai("Timeline & continuity", "Ordered timeline + contradiction findings", AiAction::Continuity),
            ai("Setting & worldbuilding", "Setting bible + rule-violation findings", AiAction::Setting),
            ai("Scaffold / outline", "Beat outline + structural gaps", AiAction::Scaffold),
            ai("Name & term consistency", "Spelling variants (cheap pass)", AiAction::Naming),
            ai("Prose & style", "POV / tense / voice findings (analysis only)", AiAction::Style),
            ai("Submission material", "Synopsis, blurb & query letter (off-manuscript)", AiAction::Submission),
            ai("Revision triage", "All findings so far, in one list (local)", AiAction::Triage),
        ];
        self.mode = Mode::Palette(Box::new(Palette { title: "AI", items, selected: 0 }));
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
            KeyCode::Enter => {
                let command = p.items.get(p.selected).map(|it| it.command);
                self.mode = Mode::Insert;
                match command {
                    Some(Command::Ai(a)) => self.start_ai_action(a),
                    Some(Command::Git(g)) => self.start_git_action(g),
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
    pub(crate) fn start_ai_action(&mut self, action: AiAction) {
        // The local backend is free, so there's nothing to confirm — run directly.
        if self.ai.backend() == ai::BackendKind::Local {
            self.run_ai_action(action);
            return;
        }
        let Some((model, max_tokens)) = self.cost_profile(action) else {
            // Interactive / local-only action — no whole-manuscript request to estimate.
            self.run_ai_action(action);
            return;
        };
        let est = cost::estimate(self.manuscript_text().len(), max_tokens, model);
        if est.dollars > self.ai.cost_threshold {
            let label = format!("{} — run? (y/N)", cost::label(&est, model));
            self.open_prompt(Pending::ConfirmAi(action), label, "", None);
        } else {
            self.run_ai_action(action);
        }
    }

    /// The model and output bound a network pass will send — the inputs to its
    /// cost estimate, read from the same constants the request builders use so
    /// the estimate can't drift from the real request. `None` for the local /
    /// interactive actions (Ask, Triage), which issue no costed pass.
    fn cost_profile(&self, action: AiAction) -> Option<(&str, u32)> {
        match action {
            AiAction::Ask | AiAction::Triage => None,
            AiAction::Naming => Some((naming::MODEL, naming::MAX_TOKENS)),
            _ => Some((&self.ai.model, ai::MAX_OUTPUT_TOKENS)),
        }
    }

    /// Dispatch an AI action to its handler. Shared by the palette and the cost
    /// confirm.
    pub(crate) fn run_ai_action(&mut self, action: AiAction) {
        match action {
            AiAction::Ask => self.open_prompt(Pending::AskAi, "Ask the manuscript: ", "", None),
            AiAction::Characters => self.analyze_characters(),
            AiAction::Continuity => self.analyze_continuity(),
            AiAction::Setting => self.analyze_setting(),
            AiAction::Scaffold => self.analyze_scaffold(),
            AiAction::Naming => self.analyze_naming(),
            AiAction::Style => self.analyze_style(),
            AiAction::Submission => self.analyze_submission(),
            AiAction::Triage => self.open_triage(),
        }
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

    /// The whole manuscript as plain text for an AI request: each chapter under
    /// its title heading, the active chapter live from its buffer and the rest
    /// from disk (the `count_todo` pattern).
    pub(crate) fn manuscript_text(&self) -> String {
        let mut out = String::new();
        for i in 0..self.chapter_count() {
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

    /// Start a character-extraction pass; profiles are written / conflicts queued
    /// by [`App::poll_ai`].
    pub(crate) fn analyze_characters(&mut self) {
        self.spawn_ai("⟳ Analyzing characters…", |backend, manuscript, model| {
            let req = character::build_request(manuscript, &model);
            Outcome::Characters(backend.complete(&req).and_then(|r| character::parse(&r.text)))
        });
    }

    /// Start a timeline & continuity pass; the timeline + findings are applied by
    /// [`App::poll_ai`].
    pub(crate) fn analyze_continuity(&mut self) {
        self.spawn_ai("⟳ Checking continuity…", |backend, manuscript, model| {
            let req = continuity::build_request(manuscript, &model);
            Outcome::Continuity(backend.complete(&req).and_then(|r| continuity::parse(&r.text)))
        });
    }

    /// Start a setting & worldbuilding pass; the setting bible + rule findings are
    /// applied by [`App::poll_ai`].
    pub(crate) fn analyze_setting(&mut self) {
        self.spawn_ai("⟳ Building setting bible…", |backend, manuscript, model| {
            let req = setting::build_request(manuscript, &model);
            Outcome::Setting(backend.complete(&req).and_then(|r| setting::parse(&r.text)))
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

    /// Generate submission material (synopsis / blurb / query); written to
    /// `submission/` by [`App::poll_ai`].
    pub(crate) fn analyze_submission(&mut self) {
        self.spawn_ai("⟳ Drafting submission material…", |backend, manuscript, model| {
            let req = submission::build_request(manuscript, &model);
            Outcome::Submission(backend.complete(&req).and_then(|r| submission::parse(&r.text)))
        });
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
        if self.ai_task.is_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        // Resolve which backend to use (and gate on readiness) on the UI thread;
        // actually *building* it — which for the local model means loading (and
        // maybe downloading) — happens on the worker thread so the UI never blocks.
        let spec = match self.backend_spec() {
            Ok(spec) => spec,
            Err(e) => {
                self.message = Some(e.to_string());
                return;
            }
        };
        let model = self.ai.model.clone();
        let manuscript = self.manuscript_text();

        self.ai_task.spawn(move || match spec.build() {
            Ok(backend) => run(backend.as_ref(), manuscript, model),
            Err(e) => Outcome::Error(e),
        });
        self.message = Some(message.to_string());
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
        match self.ai_task.poll() {
            Poll::Ready(outcome) => {
                self.message = None;
                self.overlay = None; // a fresh result supersedes any open overlay
                match outcome {
                    Outcome::Answer(r) => self.apply_qa_result(r),
                    Outcome::Characters(r) => self.apply_characters(r),
                    Outcome::Continuity(r) => self.apply_continuity(r),
                    Outcome::Setting(r) => self.apply_setting(r),
                    Outcome::Scaffold(r) => self.apply_scaffold(r),
                    Outcome::Naming(r) => self.apply_naming(r),
                    Outcome::Style(r) => self.apply_style(r),
                    Outcome::Submission(r) => self.apply_submission(r),
                    Outcome::Error(e) => self.message = Some(format!("AI: {e}")),
                }
            }
            Poll::Pending => {}
            Poll::Died => self.message = Some("AI: request failed".into()),
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

    /// Apply a submission pass: write the synopsis / blurb / query to
    /// `submission/` (conflict-reviewed like any generated file) and summarize.
    pub(crate) fn apply_submission(&mut self, result: Result<Submission, ai::AiError>) {
        self.overlay = None;
        let sub = match result {
            Ok(s) => s,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
        };
        let today = date::iso(date::today());
        let mut written = Vec::new();
        let mut conflicts = Vec::new();
        let mut failures = Vec::new();
        for (kind, file, text) in sub.artifacts() {
            let path = self.project.root.join("submission").join(file);
            let title = format!("title: {kind}\n");
            match self.stage_bible(path, kind, |notes| bible::render(&title, text, notes, &today)) {
                Staged::Wrote => written.push(format!("submission/{file}")),
                Staged::Conflict(c) => conflicts.push(c),
                Staged::Failed(msg) => failures.push(msg),
            }
        }

        if !failures.is_empty() {
            self.message = Some(write_failure_message(&failures));
        } else if conflicts.is_empty() {
            let mut lines = vec![Line::raw("Submission material written:"), Line::raw("")];
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
            Staged::Wrote => self.open_findings(),
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
                self.mode = Mode::Insert;
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
            self.mode = Mode::Insert;
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
                        self.mode = Mode::Insert;
                    } else {
                        f.selected = f.selected.min(f.items.len() - 1);
                    }
                }
            }
            KeyCode::Enter => {
                let target = f.items.get(f.selected).and_then(|it| it.chapters.first()).cloned();
                self.mode = Mode::Insert;
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
            self.project.set_active_words(self.buffer.word_count());
        }
        // Non-active chapters: the marker is on disk; the report's `xxx` count
        // reads bodies live, and leaving the word cache untouched keeps an
        // AI-inserted note out of the author's word tally.
        self.message = Some(format!("Marked xxx in '{title}'"));
    }
}

/// A unified line diff (current → proposed), each line signed `+`/`-`/` `.
/// Computed once when a conflict is staged (not per frame).
/// The new selection for a navigable list given a key, or `None` if the key
/// isn't a movement key. Shared by the findings and palette overlays.
fn list_move(selected: usize, len: usize, code: KeyCode) -> Option<usize> {
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
