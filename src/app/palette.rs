//! The AI command palette and action dispatch (#152/#82): the palette/action
//! menus, the run/dispatch routing, and the cost-estimate gate. Extracted from
//! `assist.rs` (#315).

use super::*;

use super::assist::list_move;
use crate::ai::{self, cost, naming, router, BackendKind};
use crate::mode::{Action, AiAction, AiCost, ExportScope, PaletteRow};

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
        // The action *set* and its order come from the single descriptor table
        // (#563); this only supplies each row's palette label + blurb.
        let items = AiAction::SPECS
            .iter()
            .map(|s| {
                let (label, desc) = ai_palette_entry(s.action);
                PaletteItem::new(label, desc, Command::Ai(s.action))
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "AI",
            items,
            selected: 0,
            collapsed: Default::default(),
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
                PaletteItem::new(a.label(), desc, Command::Run(a)).in_group(a.group())
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Run action",
            items,
            selected: 0,
            collapsed: Default::default(),
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
            // The in-app on-ramp (#602) only makes sense while AI is off; once it's
            // enabled the switch, key, and backend actions cover the rest.
            Action::EnableAi if self.ai.enabled => Some("AI is already enabled".into()),
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
        // When AI is off (the default) and the author reaches for an AI action,
        // offer to enable it in-app instead of the dead-end "set enabled = true"
        // message (#602) — the prompt writes `[ai] enabled = true`, then chains
        // into the key flow, and re-runs the action they wanted.
        if self.wants_ai_but_off(action) {
            self.offer_enable_ai(resume_action(action));
            return;
        }
        if let Some(reason) = self.action_status(action) {
            self.message = Some(format!("{}: {reason}", action.label()));
            return;
        }
        self.dispatch_action(action, force);
    }

    /// Whether `action` needs the AI backend but AI is off (#602) — the case the
    /// in-app enable on-ramp intercepts. Triage is local aggregation and the image
    /// passes carry their own opt-in, so neither counts; a keyed-but-off setup
    /// still routes here (the enable confirm chains to the key flow only when a key
    /// is missing). `EnableAi` itself is excluded so its own dispatch runs.
    fn wants_ai_but_off(&self, action: Action) -> bool {
        if self.ai.enabled {
            return false;
        }
        matches!(
            action,
            Action::Ai(
                AiAction::Ask
                    | AiAction::Characters
                    | AiAction::Continuity
                    | AiAction::Setting
                    | AiAction::Scaffold
                    | AiAction::Naming
                    | AiAction::Style
                    | AiAction::Brainstorm
                    | AiAction::DraftCharacters
                    | AiAction::DraftSetting
                    | AiAction::Voice
                    | AiAction::Submission
                    | AiAction::Marketing
                    | AiAction::Social
                    | AiAction::AdCreative
                    | AiAction::Thesaurus
                    | AiAction::Editorial
            ) | Action::RefreshSeriesBible
                | Action::DescribeConfig
        )
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
            Action::WritingHistory => self.open_writing_history(),
            Action::Ai(a) => self.start_ai_action(a, force),
            Action::EnableAi => self.offer_enable_ai(None),
            Action::UserGuide => self.open_guide(),
            Action::ImportNotes => self.start_import(),
            Action::NewProjectFromNotes => self.start_new_from_notes(),
            Action::ImportManuscript => self.start_import_manuscript(),
            Action::ArchiveProject => self.start_archive_project(),
            Action::RestoreArchive => self.start_restore_archive(),
            Action::StartSprint => self.start_sprint(),
            Action::StopSprint => self.stop_sprint(),
            Action::SwitchBook => self.open_book_switcher(),
            Action::SeriesTree => self.open_tree(),
            Action::RenameProject => self.prompt_rename_project(),
            Action::BookDetails => self.start_book_details(),
            Action::SetGoals => self.start_set_goals(),
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
            Action::Corkboard => self.open_corkboard(),
            Action::AddAnnotation => self.start_annotation(),
            Action::ReviewAnnotations => self.open_annotation_review(),
            Action::SwitchBackend => self.toggle_ai_backend(),
            Action::ManageModels => self.open_model_manager(),
            Action::SetClaudeKey => self.start_set_api_key(ai::keychain::Key::Anthropic),
            Action::SetImageKey => self.start_set_api_key(ai::keychain::Key::Fal),
            Action::Assistant => self.open_assistant(),
            Action::BrowseBible => self.open_bible_files(),
            Action::InsertSnippet => self.open_snippet_picker(),
            Action::BrowseFonts => self.open_font_browser(),
            Action::SetExportStyle => self.open_export_style_menu(),
            Action::Settings => self.open_settings_menu(),
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
            .map(|(i, s)| {
                PaletteItem::new(
                    s.name.clone(),
                    snippet_preview(&s.text),
                    Command::InsertSnippet(i),
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Insert snippet",
            items,
            selected: 0,
            collapsed: Default::default(),
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
        // Navigation and collapse run over the *visible* rows (headers + the items
        // of expanded groups), so the selection tracks what's on screen (#514).
        let rows = p.rows();
        if let Some(s) = list_move(p.selected, rows.len(), key.code) {
            p.selected = s;
            return;
        }
        let current = rows.get(p.selected);
        match key.code {
            // On a group header: →/← (and Enter) expand or collapse it (#514).
            KeyCode::Right => {
                if let Some(PaletteRow::Header {
                    group,
                    collapsed: true,
                }) = current
                {
                    p.set_collapsed(group, false);
                }
            }
            KeyCode::Left => {
                match current {
                    Some(PaletteRow::Header {
                        group,
                        collapsed: false,
                    }) => p.set_collapsed(group, true),
                    // From an item, ← collapses its group and lands on that header.
                    Some(PaletteRow::Item(i)) => {
                        if let Some(g) = p.items[*i].group {
                            p.set_collapsed(g, true);
                            // Collapsing only removes the rows *after* the header, so its
                            // index in the rows we already computed still points at it.
                            p.selected = rows
                            .iter()
                            .position(|r| matches!(r, PaletteRow::Header { group, .. } if *group == g))
                            .unwrap_or(p.selected);
                        }
                    }
                    _ => {}
                }
            }
            // Enter toggles a header; on an item it runs the action (auto-scoped for
            // bibles); `R` forces a full-manuscript bible rebuild (#176).
            KeyCode::Enter | KeyCode::Char('R') => {
                let command = match current {
                    Some(PaletteRow::Header { group, collapsed }) => {
                        p.set_collapsed(group, !*collapsed);
                        return;
                    }
                    Some(PaletteRow::Item(i)) => Some(p.items[*i].command.clone()),
                    None => None,
                };
                let force = matches!(key.code, KeyCode::Char('R'));
                self.mode = self.editing_mode();
                match command {
                    Some(Command::Ai(a)) => self.start_ai_action(a, force),
                    Some(Command::Git(g)) => self.start_git_action(g),
                    Some(Command::Export(f, scope)) => self.run_export(f, scope),
                    Some(Command::PreviewStyle) => self.preview_style(),
                    Some(Command::Run(a)) => self.run_action(a, force),
                    Some(Command::RunWorkflow(i)) => self.run_workflow(i),
                    Some(Command::NewWorkflowFromPrompt) => self.start_describe_workflow(),
                    Some(Command::Portrait(c)) => self.confirm_portraits(c),
                    Some(Command::SwitchBook(i)) => self.switch_book(i),
                    Some(Command::NewProjectTemplate(i)) => self.create_project_with_template(i),
                    Some(Command::InsertSnippet(i)) => self.insert_snippet(i),
                    Some(Command::PickFontRole(i)) => self.open_font_role_picker(i),
                    Some(Command::ApplyFont(i, target)) => self.apply_font(i, target),
                    Some(Command::OpenStylePicker(p)) => self.open_style_picker(p),
                    Some(Command::SetStylePreset(i)) => self.set_style_preset(i),
                    Some(Command::SetStyleAlign(i)) => self.set_style_align(i),
                    Some(Command::SetStyleDropCap(v)) => self.set_style_drop_cap(v),
                    Some(Command::SetStyleTrim(i)) => self.set_style_trim(i),
                    Some(Command::EditStyleText(k)) => self.prompt_style_text(k),
                    Some(Command::OpenSettingPicker(p)) => self.open_setting_picker(p),
                    Some(Command::SetSettingBool(knob, v)) => self.set_setting_bool(knob, v),
                    Some(Command::SetSettingTheme(i)) => self.set_setting_theme(i),
                    Some(Command::EditSettingText(k)) => self.prompt_setting_text(k),
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
    /// the estimate can't drift from the real request. Which profile each action
    /// carries is the descriptor's [`AiCost`] (#563); this resolves it against
    /// live AI settings. `None` for the local / interactive / image actions
    /// (Ask, Triage, Brainstorm, the drafts, the portraits/scene, the thesaurus),
    /// which issue no costed whole-manuscript pass.
    fn cost_profile(&self, action: AiAction) -> Option<(&str, u32)> {
        match action.cost() {
            AiCost::None => None,
            AiCost::Naming => Some((naming::MODEL, naming::MAX_TOKENS)),
            AiCost::Manuscript => Some((&self.ai.model, ai::MAX_OUTPUT_TOKENS)),
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

/// The AI palette's label + one-line blurb for an action. Presentation only —
/// the action *set* and order come from [`AiAction::SPECS`] (#563), so this is an
/// exhaustive `match` the compiler checks, not a hand-maintained list that can
/// silently omit a variant.
fn ai_palette_entry(action: AiAction) -> (&'static str, &'static str) {
    match action {
        AiAction::Ask => (
            "Ask the manuscript",
            "Answer a question grounded in the draft",
        ),
        AiAction::Characters => ("Character bible", "Profiles, arcs, and drift flags"),
        AiAction::Continuity => (
            "Timeline & continuity",
            "Ordered timeline + contradiction findings",
        ),
        AiAction::Setting => (
            "Setting & worldbuilding",
            "Setting bible + rule-violation findings",
        ),
        AiAction::Scaffold => ("Scaffold / outline", "Beat outline + structural gaps"),
        AiAction::Naming => ("Name & term consistency", "Spelling variants (cheap pass)"),
        AiAction::Style => (
            "Prose & style",
            "POV / tense / voice findings (analysis only)",
        ),
        AiAction::Brainstorm => (
            "Brainstorm ideas",
            "Premises, themes & conflicts from a spark (off-manuscript)",
        ),
        AiAction::DraftCharacters => (
            "Draft characters",
            "Starter cast from a concept (no manuscript needed)",
        ),
        AiAction::DraftSetting => (
            "Draft setting",
            "Starter worldbuilding bible from a concept",
        ),
        AiAction::Voice => (
            "Character voice",
            "Dialogue & POV voice vs. the character bible",
        ),
        AiAction::Submission => (
            "Submission material",
            "Synopsis, blurb & query letter (off-manuscript)",
        ),
        AiAction::Marketing => (
            "Marketing copy",
            "Blurb variants & taglines (off-manuscript)",
        ),
        AiAction::Social => (
            "Social posts",
            "X / Facebook / Instagram copy (off-manuscript)",
        ),
        AiAction::AdCreative => (
            "Ad creative",
            "Amazon / Meta / X ad copy & targeting (off-manuscript)",
        ),
        AiAction::Portrait => (
            "Character portraits",
            "Portrait images from the character bible (fal.ai)",
        ),
        AiAction::Scene => (
            "Scene illustration",
            "Illustrate the active chapter (fal.ai)",
        ),
        AiAction::Triage => (
            "Revision triage",
            "All findings so far, in one list (local)",
        ),
        AiAction::Thesaurus => (
            "Word search",
            "In-context thesaurus for a word or phrase (reference only)",
        ),
        AiAction::Editorial => (
            "Editorial read",
            "Flag structural / continuity / line issues — no fixes",
        ),
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

/// The AI action to re-run after the enable on-ramp finishes (#602): the pass the
/// author reached for, so confirming enable lands them in the flow they wanted.
/// `None` for the AI-powered helpers (series bible, describe-config) and the plain
/// "Enable AI assistance" action — those just enable and stop.
fn resume_action(action: Action) -> Option<AiAction> {
    match action {
        Action::Ai(a) => Some(a),
        _ => None,
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
