//! The AI command palette and action dispatch (#152/#82): the palette/action
//! menus, the run/dispatch routing, and the cost-estimate gate. Extracted from
//! `assist.rs` (#315).

use super::*;

use super::assist::list_move;
use crate::ai::{self, cost, naming, router, BackendKind};
use crate::mode::{Action, AiAction, ExportScope};

/// The routing decision for starting an AI action (#82/#372): run it now on a
/// chosen backend, or confirm first (the estimate is over the threshold) — the
/// confirm optionally offering "run locally instead" as a third choice.
pub(crate) enum AiStart {
    Run(BackendKind),
    Confirm { label: String, offer_local: bool },
}

impl App {
    /// Open the AI command palette — the single entry point for the AI features.
    /// (The caller gates on [`ai::Settings::check_ready`].)
    pub(crate) fn open_ai_palette(&mut self) {
        let ai = |label: &str, desc: &str, a| PaletteItem {
            label: label.into(),
            desc: desc.into(),
            command: Command::Ai(a),
        };
        let items = vec![
            ai(
                "Ask the manuscript",
                "Answer a question grounded in the draft",
                AiAction::Ask,
            ),
            ai(
                "Character bible",
                "Profiles, arcs, and drift flags",
                AiAction::Characters,
            ),
            ai(
                "Timeline & continuity",
                "Ordered timeline + contradiction findings",
                AiAction::Continuity,
            ),
            ai(
                "Setting & worldbuilding",
                "Setting bible + rule-violation findings",
                AiAction::Setting,
            ),
            ai(
                "Scaffold / outline",
                "Beat outline + structural gaps",
                AiAction::Scaffold,
            ),
            ai(
                "Name & term consistency",
                "Spelling variants (cheap pass)",
                AiAction::Naming,
            ),
            ai(
                "Prose & style",
                "POV / tense / voice findings (analysis only)",
                AiAction::Style,
            ),
            ai(
                "Brainstorm ideas",
                "Premises, themes & conflicts from a spark (off-manuscript)",
                AiAction::Brainstorm,
            ),
            ai(
                "Draft characters",
                "Starter cast from a concept (no manuscript needed)",
                AiAction::DraftCharacters,
            ),
            ai(
                "Draft setting",
                "Starter worldbuilding bible from a concept",
                AiAction::DraftSetting,
            ),
            ai(
                "Character voice",
                "Dialogue & POV voice vs. the character bible",
                AiAction::Voice,
            ),
            ai(
                "Submission material",
                "Synopsis, blurb & query letter (off-manuscript)",
                AiAction::Submission,
            ),
            ai(
                "Marketing copy",
                "Blurb variants & taglines (off-manuscript)",
                AiAction::Marketing,
            ),
            ai(
                "Social posts",
                "X / Facebook / Instagram copy (off-manuscript)",
                AiAction::Social,
            ),
            ai(
                "Ad creative",
                "Amazon / Meta / X ad copy & targeting (off-manuscript)",
                AiAction::AdCreative,
            ),
            ai(
                "Character portraits",
                "Portrait images from the character bible (fal.ai)",
                AiAction::Portrait,
            ),
            ai(
                "Scene illustration",
                "Illustrate the active chapter (fal.ai)",
                AiAction::Scene,
            ),
            ai(
                "Revision triage",
                "All findings so far, in one list (local)",
                AiAction::Triage,
            ),
            ai(
                "Word search",
                "In-context thesaurus for a word or phrase (reference only)",
                AiAction::Thesaurus,
            ),
            ai(
                "Editorial read",
                "Flag structural / continuity / line issues — no fixes",
                AiAction::Editorial,
            ),
        ];
        self.mode = Mode::Palette(Box::new(Palette {
            title: "AI",
            items,
            selected: 0,
        }));
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
                    // The backend switch shows which backend is active right now,
                    // so the author can see the state before flipping it (#371).
                    None if matches!(a, Action::SwitchBackend) => {
                        format!("{} · now: {}", a.name(), self.ai_backend_label())
                    }
                    None => a.name().to_string(),
                };
                PaletteItem {
                    label: a.label().into(),
                    desc,
                    command: Command::Run(a),
                }
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Run action",
            items,
            selected: 0,
        }));
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
            Action::Ai(_) => self.ai_startable(),
            // Export compiles per-chapter files; the continuous view holds an
            // unsaved window, so leave it first.
            Action::Export(_) if self.manuscript.is_some() => {
                Some("leave the continuous view (Ctrl-l) first".into())
            }
            // Switching books only applies to a multi-book series (#329).
            Action::SwitchBook if self.project.book_count() <= 1 => {
                Some("this project has only one book".into())
            }
            // The series-bible refresh needs more than one book and a ready AI
            // backend (it spawns per-book passes) (#340).
            Action::RefreshSeriesBible if self.project.book_count() <= 1 => {
                Some("this project has only one book".into())
            }
            Action::RefreshSeriesBible => self.ai.check_ready().err().map(|e| e.to_string()),
            // Describe-to-config needs the AI to propose the settings (#303).
            Action::DescribeConfig => self.ai.check_ready().err().map(|e| e.to_string()),
            // The assistant pane is a single-chapter-view feature, like the splits.
            Action::Assistant if self.manuscript.is_some() => {
                Some("leave the continuous view (Ctrl-l) first".into())
            }
            // Sprint controls reflect whether one is running (#299).
            Action::StartSprint if self.sprint.is_some() => {
                Some("a sprint is already running".into())
            }
            Action::StopSprint if self.sprint.is_none() => Some("no sprint is running".into()),
            // Snippets need a `[snippets]` table to insert from (#411).
            Action::InsertSnippet if self.snippets.is_empty() => {
                Some("no snippets defined — add a [snippets] table to your config".into())
            }
            _ => None,
        }
    }

    /// Whether an AI action can start (#372), or why not. Normally the active
    /// backend's readiness — but under a routing policy with a usable local
    /// backend, an action can still run on-device even when Claude has no key, so
    /// it isn't blocked. The default `Configured` policy keeps the exact prior
    /// behavior (the session backend's readiness).
    fn ai_startable(&self) -> Option<String> {
        // The active backend being ready always suffices — covers `configured`, a
        // keyed Claude, and the `offline` pin (exact prior behavior).
        if self.ai.check_ready().is_ok() {
            return None;
        }
        // Otherwise, only a policy that can actually fall back to local — not
        // `prefer-cloud`, which always picks Claude — can run on a compiled-in
        // on-device backend without a key. (A cheap check: the precise per-pass
        // route would need the cost estimate, not worth a manuscript read per
        // menu item; an unkeyed pass a policy still sends to Claude fails at run
        // with the usual message.)
        let local_fallback = cfg!(feature = "local")
            && self.ai.enabled
            && matches!(
                self.ai.routing,
                router::Policy::PreferLocal | router::Policy::CostCeiling | router::Policy::Ask
            );
        if local_fallback {
            return None;
        }
        self.ai.check_ready().err().map(|e| e.to_string())
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
            Action::Export(f) => self.run_export(f, ExportScope::ActiveBook),
            Action::ProgressReport => self.open_progress_report(),
            Action::AnalysisReport => self.open_analysis_report(),
            Action::Ai(a) => self.start_ai_action(a, force),
            Action::UserGuide => self.open_guide(),
            Action::ImportNotes => self.start_import(),
            Action::NewProjectFromNotes => self.start_new_from_notes(),
            Action::ImportManuscript => self.start_import_manuscript(),
            Action::StartSprint => self.start_sprint(),
            Action::StopSprint => self.stop_sprint(),
            Action::SwitchBook => self.open_book_switcher(),
            Action::SeriesTree => self.open_tree(),
            Action::RenameProject => self.prompt_rename_project(),
            Action::RefreshSeriesBible => self.start_refresh_series_bible(),
            Action::ProjectFind => self.open_project_find(),
            Action::DescribeConfig => self.start_describe_config(),
            Action::AcceptWord => self.accept_word_at_cursor(),
            Action::NextMisspelling => self.navigate_spell_flag(1),
            Action::PrevMisspelling => self.navigate_spell_flag(-1),
            Action::ToggleTypewriter => {
                self.typewriter = !self.typewriter;
                self.message = Some(format!("Typewriter scrolling {}.", on_off(self.typewriter)));
            }
            Action::ToggleFocus => {
                self.focus_mode = !self.focus_mode;
                self.message = Some(format!("Focus mode {}.", on_off(self.focus_mode)));
            }
            Action::AddAnnotation => self.start_annotation(),
            Action::ReviewAnnotations => self.open_annotation_review(),
            Action::SwitchBackend => self.toggle_ai_backend(),
            Action::ManageModels => self.open_model_manager(),
            Action::Assistant => self.open_assistant(),
            Action::InsertSnippet => self.open_snippet_picker(),
        }
    }

    /// Open the snippet picker (#411): a palette listing each author-defined
    /// snippet by name (with a preview of its text), inserted on Enter. The
    /// run-action menu gates on [`action_status`](App::action_status) when none
    /// are defined, so this normally has at least one entry.
    pub(crate) fn open_snippet_picker(&mut self) {
        if self.snippets.is_empty() {
            self.message =
                Some("No snippets defined — add a [snippets] table to your config.".into());
            return;
        }
        let items = self
            .snippets
            .iter()
            .enumerate()
            .map(|(i, s)| PaletteItem {
                label: s.name.clone(),
                desc: snippet_preview(&s.text),
                command: Command::InsertSnippet(i),
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Insert snippet",
            items,
            selected: 0,
        }));
    }

    /// Insert the author-defined snippet's literal text at the cursor (#411),
    /// through the normal edit path (so it works in both the single-chapter and
    /// continuous views) as one undo group — a single Ctrl-Z removes it in the
    /// single-chapter view. No transformation: smart typography is bypassed, the
    /// text lands exactly as configured.
    pub(crate) fn insert_snippet(&mut self, index: usize) {
        let Some(snippet) = self.snippets.get(index).cloned() else {
            return;
        };
        self.edit_buffer_mut().seal_undo_group();
        for c in snippet.text.chars() {
            self.edit_insert_raw(c);
        }
        self.edit_buffer_mut().seal_undo_group();
        self.message = Some(format!("Inserted snippet '{}'", snippet.name));
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
                    Some(Command::Export(f, scope)) => self.run_export(f, scope),
                    Some(Command::Run(a)) => self.run_action(a, force),
                    Some(Command::RunWorkflow(i)) => self.run_workflow(i),
                    Some(Command::NewWorkflowFromPrompt) => self.start_describe_workflow(),
                    Some(Command::Portrait(c)) => self.confirm_portraits(c),
                    Some(Command::SwitchBook(i)) => self.switch_book(i),
                    Some(Command::InsertSnippet(i)) => self.insert_snippet(i),
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
            AiStart::Run(backend) => {
                // Route this request to the chosen backend (consumed by backend_spec).
                self.route_request(backend, action, force);
                self.run_ai_action(action, force);
            }
            AiStart::Confirm { label, offer_local } => self.open_prompt(
                Pending::ConfirmAi {
                    action,
                    force,
                    offer_local,
                },
                label,
                "",
                None,
            ),
        }
    }

    /// Set the one-shot routing for the next AI request (#372/#377): the backend,
    /// plus — when it's local — the model the router picks for this action's size
    /// from the author's configured set. Both are consumed by `backend_spec`.
    pub(crate) fn route_request(&self, backend: BackendKind, action: AiAction, force: bool) {
        self.routed_backend.set(Some(backend));
        if backend == BackendKind::Local {
            self.routed_model.set(self.pick_local_model(action, force));
        }
    }

    /// The local model best suited to this action (#377): the router picks from the
    /// author's downloaded set (`[ai] local_models`) by the pass's size. `None`
    /// when no set is configured — the backend then loads the single recommended
    /// model, as before.
    fn pick_local_model(
        &self,
        action: AiAction,
        force: bool,
    ) -> Option<&'static ai::model_select::Model> {
        let available = ai::model_select::available(&self.ai.local_models);
        if available.is_empty() {
            return None;
        }
        let tokens = (self.pass_context_len(action, force) / 4) as u32;
        ai::model_select::pick_for(tokens, &available)
    }

    /// Whether a downloaded local model's context window can hold a pass of
    /// `input_tokens` (#378) — the router routes on-device only when it can. With
    /// no model set configured, the single recommended (standard, 32k) model is
    /// the bound; otherwise it's the largest window among the configured set, so a
    /// long-context model lets a whole-manuscript pass run locally.
    fn local_fits(&self, input_tokens: u32) -> bool {
        cfg!(feature = "local")
            && input_tokens <= ai::model_select::local_window(&self.ai.local_models)
    }

    /// The cost decision for a network AI action (#82): run it now, or confirm
    /// first because the estimate exceeds the threshold. Shared by the AI palette
    /// ([`start_ai_action`]) and the workflow runner (#227) so a workflow's AI
    /// step follows the same cost policy.
    pub(crate) fn ai_action_start(&self, action: AiAction, force: bool) -> AiStart {
        let Some((model, max_tokens)) = self.cost_profile(action) else {
            // Interactive / image action — no costed pass to route; run on the
            // session backend (which an `offline` pin / the in-app switch resolve).
            return AiStart::Run(self.ai.backend());
        };
        // Estimate the remote cost, then let the hybrid router (#372) pick a backend
        // from the policy, the estimate, and each backend's readiness.
        let est = cost::estimate(self.pass_context_len(action, force), max_tokens, model);
        let route = router::route(&router::Inputs {
            policy: self.ai.routing,
            configured: self.ai.backend,
            offline: self.ai.offline,
            local_available: cfg!(feature = "local"),
            local_fits: self.local_fits(est.tokens),
            est,
            threshold: self.ai.cost_threshold,
        });
        match route.confirm {
            None => AiStart::Run(route.backend),
            Some(c) if c.offer_local => AiStart::Confirm {
                label: format!(
                    "{} — y Claude · l local · N cancel",
                    cost::compare_label(&est, model)
                ),
                offer_local: true,
            },
            Some(_) => AiStart::Confirm {
                label: format!("{} — run? (y/N)", cost::label(&est, model)),
                offer_local: false,
            },
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

/// A one-line preview of a snippet's text for the picker (#411): newlines shown
/// as `⏎`, truncated with an ellipsis so a long expansion stays on one row.
fn snippet_preview(text: &str) -> String {
    let one_line = text.replace('\n', "⏎");
    if one_line.chars().count() > 40 {
        format!("{}…", one_line.chars().take(40).collect::<String>())
    } else {
        one_line
    }
}

/// "on" / "off" for a toggle's status message.
fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}
