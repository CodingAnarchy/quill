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
#[cfg_attr(test, derive(Debug))]
pub enum AiStart {
    Run(BackendKind),
    Confirm {
        label: String,
        offer_local: bool,
    },
    /// The pass can't run as routed and is refused with this actionable message
    /// (#849): an `offline`-pinned pass too big for the on-device window, where
    /// there's no cloud to fall back to. Surfaced as a status message; nothing runs.
    Refuse(String),
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
            collapsed: std::collections::HashSet::default(),
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
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Why `action` can't run right now, or `None` if it can — the one
    /// availability check the menu annotates with, [`run_action`](App::run_action)
    /// guards on, and the workflow runner (#226) treats as a step failure. The
    /// per-action runtime guards (AI cost confirm, etc.) still live at the dispatch
    /// layer; this only covers the up-front "can't start" cases.
    // Some guarded arms share a message (e.g. "leave the continuous view") but are
    // kept separate, per-action, with their own documenting comments — merging the
    // patterns would interleave unrelated actions and lose that structure.
    #[allow(clippy::match_same_arms)]
    pub(crate) fn action_status(&self, action: Action) -> Option<String> {
        match action {
            // Triage, the health dashboard, and the Ask history are local — they
            // work with the AI backend off (browsing already-collected findings /
            // past answers, #609/#827).
            Action::Ai(AiAction::Triage | AiAction::Dashboard | AiAction::AskHistory) => None,
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
            // unsaved window, so leave it first. The preflight reads the same
            // per-chapter compile, so it shares the constraint.
            Action::Export(_)
            | Action::ReviewCopy
            | Action::PdfWrapCover
            | Action::PreflightExport
            | Action::PrepareForKdp
            | Action::PrepareForIngramSpark
                if self.manuscript.is_some() =>
            {
                Some("leave the continuous view (Ctrl-l) first".into())
            }
            // Review comments anchor into the active chapter's prose; the continuous
            // view holds a multi-chapter window, so leave it first.
            Action::ImportReviewComments if self.manuscript.is_some() => {
                Some("leave the continuous view (Ctrl-l) first".into())
            }
            // The publish bundles set [print]/language defaults on book.toml, which a
            // multi-book series has no single top-level table for; guide per-book. The
            // wrap cover's spine comes from one book's interior page count, so it's a
            // standalone artifact too (a series wraps per book).
            Action::PrepareForKdp | Action::PrepareForIngramSpark | Action::PdfWrapCover
                if self.project.book_count() > 1 =>
            {
                Some("standalone novels only — set [print] and export per book".into())
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
            // The all-bibles refresh (#655) spawns the four bible passes, so it
            // needs a ready AI backend, but works for a standalone novel too.
            Action::RefreshBibles => self.ai.check_ready().err().map(|e| e.to_string()),
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
    const fn wants_ai_but_off(&self, action: Action) -> bool {
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
                    | AiAction::NameGen
                    | AiAction::Editorial
            ) | Action::RefreshSeriesBible
                | Action::RefreshBibles
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
            Action::QuickStart => self.open_onboarding(),
            Action::ImportNotes => self.start_import(),
            Action::NewProjectFromNotes => self.start_new_from_notes(),
            Action::ImportManuscript => self.start_import_manuscript(),
            Action::ImportReviewComments => self.start_import_review_comments(),
            Action::ArchiveProject => self.start_archive_project(),
            Action::RestoreArchive => self.start_restore_archive(),
            Action::BrowseBackups => self.open_backup_browser(),
            Action::StartSprint => self.start_sprint(),
            Action::StopSprint => self.stop_sprint(),
            Action::SwitchBook => self.open_book_switcher(),
            Action::ManageBooks => self.open_manage_books(),
            Action::SeriesTree => self.open_tree(),
            Action::RenameProject => self.prompt_rename_project(),
            Action::BookDetails => self.start_book_details(),
            Action::SetGoals => self.start_set_goals(),
            Action::RefreshSeriesBible => self.start_refresh_series_bible(),
            Action::RefreshBibles => self.start_refresh_bibles(force),
            Action::ProjectFind => self.open_project_find(),
            Action::DescribeConfig => self.start_describe_config(),
            Action::AcceptWord => self.accept_word_at_cursor(),
            Action::ViewDictionary => self.view_dictionary(),
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
            Action::ToggleParagraphIndent => {
                self.paragraph_indent = !self.paragraph_indent;
                self.message = Some(format!(
                    "Paragraph indent {}.",
                    on_off(self.paragraph_indent)
                ));
            }
            Action::Corkboard => self.open_corkboard(),
            Action::ManuscriptCorkboard => self.open_manuscript_corkboard(),
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
            Action::ExportBible => self.export_story_bible(),
            Action::PreflightExport => self.run_preflight(),
            Action::PrepareForKdp => self.prepare_for_target(exporting::PublishTarget::Kdp),
            Action::PrepareForIngramSpark => {
                self.prepare_for_target(exporting::PublishTarget::IngramSpark);
            }
            Action::ReviewCopy => self.run_review_copy_export(),
            Action::PdfWrapCover => self.run_wrap_cover_export(),
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
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Insert the author-defined snippet's literal text at the cursor (#411),
    /// through the normal edit path (so it works in both the single-chapter and
    /// continuous views) as one undo group — a single Ctrl-Z removes it in the
    /// single-chapter view. No transformation: smart typography is bypassed, the
    /// text lands exactly as configured.
    ///
    /// A single caret marker `$0` (#823) is consumed (not inserted), and the
    /// cursor lands at its position — so a template can place the cursor
    /// mid-snippet. `$$0` escapes to a literal `$0` (no caret); a snippet with no
    /// marker inserts as before, cursor at the end.
    pub(crate) fn insert_snippet(&mut self, index: usize) {
        let Some(snippet) = self.snippets.get(index).cloned() else {
            return;
        };
        let (text, tail) = split_snippet_caret(&snippet.text);
        self.edit_buffer_mut().seal_undo_group();
        for c in text.chars() {
            self.edit_insert_raw(c);
        }
        // The whole snippet is inserted, then the cursor is stepped back over the
        // `tail` (the chars after the caret), landing it at the marker — one
        // motion inside the sealed group, so it's still a single undo.
        for _ in 0..tail {
            self.edit_move(Buffer::move_left);
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
                    Some(Command::AddBook) => self.prompt_add_book(),
                    Some(Command::RemoveBook(i)) => self.confirm_remove_book(i),
                    Some(Command::MoveBook { index, up }) => self.move_book(index, up),
                    Some(Command::NewProjectTemplate(i)) => self.create_project_with_template(i),
                    Some(Command::InsertSnippet(i)) => self.insert_snippet(i),
                    Some(Command::PickNameKind(k)) => self.prompt_namegen(k),
                    Some(Command::InsertName(n)) => self.insert_name(&n),
                    Some(Command::InsertWord(w)) => self.insert_word(&w),
                    Some(Command::ReplaceMisspelling {
                        start,
                        end,
                        word,
                        replacement,
                    }) => self.replace_misspelling(start, end, &word, &replacement),
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
            // Offline and too big to run or fold on-device (#849): surface the
            // actionable refusal, run nothing.
            AiStart::Refuse(message) => self.message = Some(message),
        }
    }

    /// Set the one-shot routing for the next AI request (#372/#377): the backend,
    /// plus — when it's local — the model the router picks for this action's size
    /// from the author's configured set. Both are consumed by `backend_spec`.
    pub(crate) fn route_request(&self, backend: BackendKind, action: AiAction, force: bool) {
        self.routed_backend.set(Some(backend));
        if backend == BackendKind::Local {
            self.routed_model.set(self.pick_local_model(action, force));
            // A local pass never uses a per-action cloud model — clear any one-shot
            // a prior (cancelled) cloud route may have armed, so it can't leak.
            self.routed_cloud_model.set(None);
        } else {
            // Cloud: arm the per-action model the pass's weight + estimate resolved
            // to (#930), so the request runs on the chosen tier rather than always
            // the session `[ai] model`. Only for the costed manuscript passes — an
            // interactive/local action has no `cost_profile`, so it keeps the
            // session model (the one-shot stays `None`).
            self.routed_cloud_model
                .set(self.cloud_model_override(action, force));
        }
    }

    /// The per-action cloud model to arm for this request (#930), or `None` when
    /// the pass keeps the session `[ai] model`: `Some` only for a costed manuscript
    /// pass whose picker chose a model *different* from the session one (a light,
    /// small pass stepped down to the cheaper tier). A heavy pass — or any pass the
    /// picker leaves on the configured model — arms nothing, so the existing
    /// `self.ai.model` path is untouched.
    fn cloud_model_override(&self, action: AiAction, force: bool) -> Option<String> {
        if action.cost() != AiCost::Manuscript {
            return None;
        }
        let chosen = self.cloud_model_for(action, self.pass_context_len(action, force));
        (chosen != self.ai.model).then(|| chosen.to_string())
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

    /// Whether an *actually-present* local model can hold a pass of `input_tokens`
    /// (#378/#725) — the router routes on-device only when one can. Requires both a
    /// fitting context window and a model whose weights are already on disk
    /// (`--features local` compiled in but nothing downloaded ⇒ not eligible, so a
    /// routed pass never kicks off a surprise multi-GB HuggingFace fetch, #725).
    /// With no model set configured, the single recommended model is the candidate;
    /// otherwise it's the configured set, so a long-context model lets a
    /// whole-manuscript pass run locally.
    pub(crate) fn local_fits(&self, input_tokens: u32) -> bool {
        cfg!(feature = "local")
            && input_tokens <= ai::model_select::local_window(&self.ai.local_models)
            && self.local_model_present(input_tokens)
    }

    /// Whether an oversized `offline` pass for `action` can be folded on-device
    /// instead of refused (#849): the persistent-bible passes (character, setting,
    /// timeline-continuity, plot, theme) have an incremental-revise path, so a full
    /// rebuild that doesn't fit is walked chapter-by-chapter through the window.
    /// A one-shot pass with no merge step (whole-manuscript Q&A, the findings-only
    /// scans) can't fold and takes the graceful-report path.
    fn foldable_offline(action: AiAction) -> bool {
        super::worker::bible_tracker(action).is_some()
    }

    /// The cost decision for a network AI action (#82): run it now, or confirm
    /// first because the estimate exceeds the threshold. Shared by the AI palette
    /// ([`start_ai_action`]) and the workflow runner (#227) so a workflow's AI
    /// step follows the same cost policy.
    pub(crate) fn ai_action_start(&self, action: AiAction, force: bool) -> AiStart {
        let Some((model, max_tokens)) = self.cost_profile(action, force) else {
            // Interactive / image action — no costed pass to route; run on the
            // session backend (which an `offline` pin / the in-app switch resolve).
            return AiStart::Run(self.ai.backend());
        };
        // Estimate the remote cost, then let the hybrid router (#372) pick a backend
        // from the policy, the estimate, and each backend's readiness.
        let est = cost::estimate(
            self.pass_context_len(action, force),
            max_tokens,
            model,
            self.pass_batches(action, force),
        );
        let route = router::route(&router::Inputs {
            policy: self.ai.routing,
            configured: self.ai.backend,
            offline: self.ai.offline,
            local_available: cfg!(feature = "local"),
            local_fits: self.local_fits(est.tokens),
            est_tokens: est.tokens,
            est,
            threshold: self.ai.cost_threshold,
        });
        // An offline pass too big for the on-device window is refused (#849) —
        // *unless* it can be folded: a persistent-bible pass runs incrementally,
        // window-sized chunk by chunk, so it still completes on-device. The fold is
        // driven by `run_bible_tracker`, which re-detects the oversized-offline
        // condition; here we only decline to refuse a foldable pass.
        if let Some(message) = route.refuse {
            return if Self::foldable_offline(action) {
                AiStart::Run(BackendKind::Local)
            } else {
                AiStart::Refuse(message)
            };
        }
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
    fn cost_profile(&self, action: AiAction, force: bool) -> Option<(&str, u32)> {
        match action.cost() {
            AiCost::None => None,
            AiCost::Naming => Some((naming::MODEL, naming::MAX_TOKENS)),
            // Per-action model routing (#930): a heavy reasoning pass runs on the
            // configured top model; a light, small one can drop to a cheaper tier.
            // The estimate below then reflects whichever model is chosen, and the
            // same model is armed as the one-shot cloud model the request uses.
            AiCost::Manuscript => Some((
                self.cloud_model_for(action, self.pass_context_len(action, force)),
                ai::MAX_OUTPUT_TOKENS,
            )),
        }
    }

    /// The cloud (Claude) model this action's manuscript pass will run on (#930):
    /// the pure per-action picker applied to the pass's weight, its token estimate
    /// (`context_chars / 4`, the same figure the cost estimate uses), and the
    /// author's configured `[ai] model` — a heavy pass keeps the top model, a
    /// light, small one drops to the cheaper tier, and an author-set model always
    /// wins. Borrows from `self` (either `self.ai.model` or a `'static` tier id).
    fn cloud_model_for(&self, action: AiAction, context_len: usize) -> &str {
        router::pick_cloud_model(action.weight(), (context_len / 4) as u32, &self.ai.model)
    }

    /// Dispatch an AI action to its handler. Shared by the palette and the cost
    /// confirm.
    pub(crate) fn run_ai_action(&mut self, action: AiAction, force: bool) {
        match action {
            AiAction::Ask => self.open_prompt(Pending::AskAi, "Ask the manuscript: ", "", None),
            AiAction::Characters => self.analyze_characters(force),
            AiAction::Continuity => self.analyze_continuity(force),
            AiAction::Setting => self.analyze_setting(force),
            AiAction::Plot => self.analyze_plot(force),
            AiAction::Theme => self.analyze_theme(force),
            AiAction::Scaffold => self.analyze_scaffold(),
            AiAction::Naming => self.analyze_naming(),
            AiAction::Style => self.analyze_style(),
            AiAction::Brainstorm => {
                self.open_prompt(Pending::Brainstorm, "Premise or idea: ", "", None);
            }
            AiAction::DraftCharacters => {
                self.open_prompt(Pending::DraftCharacters, "Concept / premise: ", "", None);
            }
            AiAction::DraftSetting => {
                self.open_prompt(Pending::DraftSetting, "Concept / premise: ", "", None);
            }
            AiAction::Voice => self.analyze_voice(),
            AiAction::Submission => self.analyze_submission(),
            AiAction::Marketing => self.analyze_marketing(),
            AiAction::Social => self.analyze_social(),
            AiAction::AdCreative => self.analyze_ad_creative(),
            AiAction::Portrait => self.start_portrait(),
            AiAction::Scene => self.start_scene(),
            AiAction::SemanticFind => self.start_semantic_find(),
            AiAction::Echo => self.start_echo_detection(),
            AiAction::Triage => self.open_triage(),
            AiAction::Dashboard => self.open_health_dashboard(),
            AiAction::AskHistory => self.open_qa_history(),
            AiAction::Thesaurus => self.start_thesaurus(),
            AiAction::NameGen => self.start_namegen(),
            AiAction::Editorial => self.analyze_editorial(),
        }
    }
}

/// The AI palette's label + one-line blurb for an action. Presentation only —
/// the action *set* and order come from [`AiAction::SPECS`] (#563), so this is an
/// exhaustive `match` the compiler checks, not a hand-maintained list that can
/// silently omit a variant.
const fn ai_palette_entry(action: AiAction) -> (&'static str, &'static str) {
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
        AiAction::Plot => (
            "Plot & arc tracker",
            "Thread setup→payoff status + dangling-thread findings",
        ),
        AiAction::Theme => (
            "Theme & motif tracker",
            "Motif → where it recurs / status + dangling-motif findings",
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
        AiAction::Dashboard => (
            "Manuscript health",
            "Open findings by kind & chapter, with counts (local)",
        ),
        AiAction::AskHistory => (
            "Ask history",
            "Re-open a past Ask — answer + jumpable citations (local)",
        ),
        AiAction::SemanticFind => (
            "Find similar scenes",
            "Ranked, jumpable passages like a selection or a query (local, reads only)",
        ),
        AiAction::Echo => (
            "Echo & near-duplicate detection",
            "Flag repeated / echoed passage pairs with scores (local, reads only)",
        ),
        AiAction::Thesaurus => (
            "Word search",
            "In-context thesaurus for a word or phrase (reference only)",
        ),
        AiAction::NameGen => (
            "Name generator",
            "Candidate character / place / term names by constraints (you pick)",
        ),
        AiAction::Editorial => (
            "Editorial read",
            "Flag structural / continuity / line issues — no fixes",
        ),
    }
}

/// Resolve a snippet's caret marker (#823): return the literal text to insert
/// (marker stripped, escapes resolved) and the number of chars *after* the caret
/// — how far left the cursor steps back to land on the marker.
///
/// The first unescaped `$0` is the caret; it's removed and its position becomes
/// the cursor. `$$0` is an escape that yields a literal `$0` (never a caret);
/// only the leading `$$` collapses, so `$$0` → `$0`. Any `$0` after the first
/// caret inserts literally (a template gets one cursor slot). A snippet with no
/// caret returns its text unchanged with a `0` tail, so the cursor stays at the
/// end — exactly the prior behavior.
fn split_snippet_caret(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut tail: Option<usize> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            match chars.peek() {
                // `$$0` → literal `$0`: consume the second `$` and the `0`, emit
                // `$0` verbatim (this can't become a caret).
                Some('$') => {
                    let mut lookahead = chars.clone();
                    lookahead.next(); // the second `$`
                    if lookahead.peek() == Some(&'0') {
                        chars.next(); // second `$`
                        chars.next(); // `0`
                        out.push_str("$0");
                        // Two literal chars land after any caret already seen.
                        if let Some(t) = tail.as_mut() {
                            *t += 2;
                        }
                        continue;
                    }
                }
                // The first unescaped `$0` is the caret: drop both chars and mark
                // the cursor here. Later `$0`s fall through and insert literally.
                Some('0') if tail.is_none() => {
                    chars.next(); // `0`
                    tail = Some(0);
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
        // Count the chars after the caret so the cursor can step back onto it.
        if let Some(t) = tail.as_mut() {
            *t += 1;
        }
    }
    (out, tail.unwrap_or(0))
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
const fn resume_action(action: Action) -> Option<AiAction> {
    match action {
        Action::Ai(a) => Some(a),
        _ => None,
    }
}

/// "on" / "off" for a toggle's status message.
const fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}
