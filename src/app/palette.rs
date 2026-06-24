//! The AI command palette and action dispatch (#152/#82): the palette/action
//! menus, the run/dispatch routing, and the cost-estimate gate. Extracted from
//! `assist.rs` (#315).

use super::*;

use crate::ai::{cost, naming};
use crate::mode::{Action, AiAction};
use super::assist::list_move;

/// The cost decision for starting a network AI action (#82): either run it now,
/// or confirm first (the estimate exceeds the threshold) with the confirm label.
pub(crate) enum AiStart {
    Run,
    Confirm(String),
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
            ai("Draft setting", "Starter worldbuilding bible from a concept", AiAction::DraftSetting),
            ai("Character voice", "Dialogue & POV voice vs. the character bible", AiAction::Voice),
            ai("Submission material", "Synopsis, blurb & query letter (off-manuscript)", AiAction::Submission),
            ai("Marketing copy", "Blurb variants & taglines (off-manuscript)", AiAction::Marketing),
            ai("Social posts", "X / Facebook / Instagram copy (off-manuscript)", AiAction::Social),
            ai("Ad creative", "Amazon / Meta / X ad copy & targeting (off-manuscript)", AiAction::AdCreative),
            ai("Character portraits", "Portrait images from the character bible (fal.ai)", AiAction::Portrait),
            ai("Scene illustration", "Illustrate the active chapter (fal.ai)", AiAction::Scene),
            ai("Revision triage", "All findings so far, in one list (local)", AiAction::Triage),
            ai("Word search", "In-context thesaurus for a word or phrase (reference only)", AiAction::Thesaurus),
            ai("Editorial read", "Flag structural / continuity / line issues — no fixes", AiAction::Editorial),
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
            // Switching books only applies to a multi-book series (#329).
            Action::SwitchBook if self.project.book_count() <= 1 => {
                Some("this project has only one book".into())
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
            Action::UserGuide => self.open_guide(),
            Action::ImportNotes => self.start_import(),
            Action::NewProjectFromNotes => self.start_new_from_notes(),
            Action::SwitchBook => self.open_book_switcher(),
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
                    Some(Command::SwitchBook(i)) => self.switch_book(i),
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
            | AiAction::DraftSetting | AiAction::Portrait | AiAction::Scene
            // The thesaurus sends only a word/phrase + the current line, not the
            // manuscript, so there's nothing whole-book to estimate (#290).
            | AiAction::Thesaurus => None,
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
            AiAction::DraftSetting => {
                self.open_prompt(Pending::DraftSetting, "Concept / premise: ", "", None)
            }
            AiAction::Voice => self.analyze_voice(),
            AiAction::Submission => self.analyze_submission(),
            AiAction::Marketing => self.analyze_marketing(),
            AiAction::Social => self.analyze_social(),
            AiAction::AdCreative => self.analyze_ad_creative(),
            AiAction::Portrait => self.start_portrait(),
            AiAction::Scene => self.start_scene(),
            AiAction::Triage => self.open_triage(),
            AiAction::Thesaurus => self.start_thesaurus(),
            AiAction::Editorial => self.analyze_editorial(),
        }
    }
}
