//! The AI command palette and action dispatch (#152/#82): the palette/action
//! menus, the action-status/availability lines, the run/dispatch routing, and
//! the snippet picker. Extracted from `assist.rs` (#315).
//!
//! The backend-selection half — deciding *how* a chosen AI action runs (local vs
//! cloud, the cost-estimate confirm, the model pick) and then running it — lives
//! in [`routing`], which owns [`AiStart`] and is re-exported here so callers keep
//! using `app::palette::AiStart`.

use super::*;

use super::assist::list_move;
use crate::ai::{self, router};
use crate::mode::{Action, AiAction, ExportScope, PaletteRow};

mod routing;

pub use routing::AiStart;

impl App {
    /// Open the AI command palette — the single entry point for the AI features.
    /// (The caller gates on [`ai::Settings::check_ready`].)
    pub(crate) fn open_ai_palette(&mut self) {
        // Capture the on-demand pass scope (#1054) from the live selection *before*
        // the palette mode replaces Visual: opening the palette over a selection
        // scopes the continuity/setting pass to the selected scene(s). No selection
        // clears any stale scope, so the pass runs whole-book as before.
        self.pass_scope = self.selection_pass_scope();
        // The action *set* and its order come from the single descriptor table
        // (#563); this only supplies each row's palette label + blurb.
        let items = AiAction::SPECS
            .iter()
            .filter_map(|s| {
                // The palette label + blurb are a descriptor field (#563), so the AI
                // palette is a pure projection of `SPECS` — no parallel match to drift.
                let (label, desc) = s.action.palette_entry()?;
                Some(PaletteItem::new(label, desc, Command::Ai(s.action)))
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "AI",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Open the unified run-action menu (#225): every *applicable* registry
    /// [`Action`], run by name through the same dispatch its dedicated shortcut
    /// uses. Actions that can't run in the current context are hidden (#944) rather
    /// than listed as "unavailable" — [`action_status`](App::action_status) returns
    /// `None` exactly when an action is applicable, so filtering on `is_some()`
    /// drops the inapplicable ones (Enable-AI when AI is on, the AI passes when it's
    /// off, series actions on a single book, …) while `SwitchBackend` (status
    /// `None`) stays. The groups open collapsed by default and remember the author's
    /// collapse/expand changes across opens (#943).
    pub(crate) fn open_action_palette(&mut self) {
        let items: Vec<PaletteItem> = Action::ALL
            .iter()
            .filter(|&&a| self.action_status(a).is_none())
            .map(|&a| {
                // The backend switch shows which backend is active right now, so the
                // author can see the state before flipping it (#371).
                let desc = if matches!(a, Action::SwitchBackend) {
                    format!("{} · now: {}", a.name(), self.ai_backend_label())
                } else {
                    a.name().to_string()
                };
                // The shortcut comes straight from the registry (#1103), which shares
                // its binding table with the key dispatch, so the palette shows the
                // real key that runs the action.
                PaletteItem::new(a.label(), desc, Command::Run(a))
                    .in_group(a.group())
                    .with_shortcut(a.shortcut())
            })
            .collect();
        let collapsed = self.palette_collapsed_for("Run action", &items);
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Run action",
            items,
            selected: 0,
            collapsed,
        }));
    }

    /// The collapse state a grouped palette opens with (#943): the author's
    /// remembered set for this `title`, or — the first time it's opened this session
    /// — every group collapsed, so the palette opens as a short list of group
    /// headers. Only groups that still exist in `items` are kept, so an action that
    /// became inapplicable (and dropped its whole group) doesn't leave a stale entry.
    fn palette_collapsed_for(
        &mut self,
        title: &'static str,
        items: &[PaletteItem],
    ) -> std::collections::HashSet<&'static str> {
        let groups: std::collections::HashSet<&'static str> =
            items.iter().filter_map(|i| i.group).collect();
        let saved = self
            .palette_collapsed
            .entry(title)
            // First open this session: default every group collapsed.
            .or_insert_with(|| groups.clone());
        // Prune groups that no longer exist so the stored set stays in step with the
        // current (filtered) action set.
        saved.retain(|g| groups.contains(g));
        saved.clone()
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
            Action::Ai(
                AiAction::Triage
                | AiAction::Dashboard
                | AiAction::CoOccurrence
                | AiAction::AskHistory,
            ) => None,
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
            // The durable pass queue (#923) spawns the bible passes, so it needs a
            // ready AI backend; works for a standalone novel too.
            Action::QueuePasses => self.ai.check_ready().err().map(|e| e.to_string()),
            // The background batch update (#950) is opt-in and Claude-batch-only —
            // its own gate covers the setting, the cloud backend, and the key.
            Action::BackgroundBibleUpdate => self.batch_update_status().err(),
            // Describe-to-config needs the AI to propose the settings (#303).
            Action::DescribeConfig => self.ai.check_ready().err().map(|e| e.to_string()),
            // The assistant pane is a single-chapter-view feature, like the splits.
            Action::Assistant if self.manuscript.is_some() => {
                Some("leave the continuous view (Ctrl-l) first".into())
            }
            // Chapter split/merge act on the active single-chapter buffer (the
            // cursor is the split seam); the continuous view holds a multi-chapter
            // window, so leave it first.
            Action::SplitChapter | Action::MergeChapter if self.manuscript.is_some() => {
                Some("leave the continuous view (Ctrl-l) first".into())
            }
            // The first chapter has nothing above it to merge into.
            Action::MergeChapter if self.project.active_index() == 0 => {
                Some("no chapter above to merge into".into())
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
                    | AiAction::Accuracy
                    | AiAction::Relationship
                    | AiAction::Foreshadow
                    | AiAction::FactCheck
                    | AiAction::Pacing
            ) | Action::RefreshSeriesBible
                | Action::RefreshBibles
                | Action::QueuePasses
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
            Action::RevisionPlan => self.export_revision_plan(),
            Action::HealthTrend => self.export_health_trend(),
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
            Action::SetSessionTarget => self.set_session_target(),
            Action::SwitchBook => self.open_book_switcher(),
            Action::ManageBooks => self.open_manage_books(),
            Action::SeriesTree => self.open_tree(),
            Action::RenameProject => self.prompt_rename_project(),
            Action::BookDetails => self.start_book_details(),
            Action::SetGoals => self.start_set_goals(),
            Action::RefreshSeriesBible => self.start_refresh_series_bible(),
            Action::RefreshBibles => self.start_refresh_bibles(force),
            Action::QueuePasses => self.start_queue_passes(force),
            Action::BackgroundBibleUpdate => self.start_batch_update(),
            Action::ProjectFind => self.open_project_find(),
            Action::Drafts => self.open_drafts(),
            Action::DescribeConfig => self.start_describe_config(),
            Action::AcceptWord => self.accept_word_at_cursor(),
            Action::ViewDictionary => self.view_dictionary(),
            Action::GoToLine => self.open_go_to_line(),
            Action::RecentEditBack => self.jump_to_older_edit(),
            Action::RecentEditForward => self.jump_to_newer_edit(),
            Action::LastInsertion => self.jump_to_last_insertion(),
            Action::NextMisspelling => self.navigate_spell_flag(1),
            Action::PrevMisspelling => self.navigate_spell_flag(-1),
            Action::StyleFlagSweep => self.open_style_flag_picker(),
            Action::ToggleProximityRepeats => self.toggle_proximity_repeats(),
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
            Action::ToggleAnnotationMargin => {
                self.annotation_margin = !self.annotation_margin;
                self.message = Some(format!(
                    "Annotation margin {}.",
                    on_off(self.annotation_margin)
                ));
            }
            Action::FoldAll => self.fold_all(),
            Action::ExpandAll => self.expand_all_folds(),
            Action::ToggleComposition => self.toggle_distraction_free(),
            Action::ToggleGalley => self.toggle_galley(),
            Action::Corkboard => self.open_corkboard(),
            Action::ManuscriptCorkboard => self.open_manuscript_corkboard(),
            Action::SplitChapter => self.start_split_chapter(),
            Action::MergeChapter => self.start_merge_chapter(),
            Action::AddAnnotation => self.start_annotation(),
            Action::ReviewAnnotations => self.open_annotation_review(),
            Action::RemoveAnnotation => self.remove_annotation_at_cursor(),
            Action::ClearAnnotations => self.start_clear_annotations(),
            Action::SwitchBackend => self.toggle_ai_backend(),
            Action::ManageModels => self.open_model_manager(),
            Action::SetClaudeKey => self.start_set_api_key(ai::keychain::Key::Anthropic),
            Action::SetImageKey => self.start_set_api_key(ai::keychain::Key::Fal),
            Action::Assistant => self.open_assistant(),
            Action::BrowseBible => self.open_bible_files(),
            Action::ViewResearch => self.open_research_notebook(),
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

    /// Split the open chapter at the cursor into two (#585), from the Ctrl-K
    /// palette — the named-action twin of the chapter browser's `x`. The live
    /// cursor is the seam, so it operates on the active chapter and prompts for the
    /// new chapter's title, returning to that chapter's row in the browser.
    pub(crate) fn start_split_chapter(&mut self) {
        let at = self.project.active_index();
        self.open_prompt(
            Pending::SplitChapter { at },
            "Title for the new chapter after the cursor: ",
            "",
            Some(at),
        );
    }

    /// Merge the active chapter into the one above it (#585), from the Ctrl-K
    /// palette — the named-action twin of the chapter browser's `M`, behind the
    /// same y/N confirm. The first chapter has nothing above it to merge into.
    pub(crate) fn start_merge_chapter(&mut self) {
        let at = self.project.active_index();
        if at == 0 {
            self.message = Some("No chapter above to merge into.".into());
            return;
        }
        let title = self.project.manifest.chapters[at].title.clone();
        let above = self
            .project
            .chapter_title(at - 1)
            .unwrap_or("the chapter above")
            .to_string();
        self.open_prompt(
            Pending::Merge { at },
            format!("Merge '{title}' into '{above}'? (y/N)"),
            "",
            Some(at),
        );
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
        // Set when a group's collapse state changes, so it's persisted back to
        // `App.palette_collapsed` (#943) once the `p` borrow ends — remembering the
        // author's collapse/expand across opens.
        let mut collapse_changed = false;
        match key.code {
            // On a group header: →/← (and Enter) expand or collapse it (#514).
            KeyCode::Right => {
                if let Some(PaletteRow::Header {
                    group,
                    collapsed: true,
                }) = current
                {
                    p.set_collapsed(group, false);
                    collapse_changed = true;
                }
            }
            KeyCode::Left => {
                match current {
                    Some(PaletteRow::Header {
                        group,
                        collapsed: false,
                    }) => {
                        p.set_collapsed(group, true);
                        collapse_changed = true;
                    }
                    // From an item, ← collapses its group and lands on that header.
                    Some(PaletteRow::Item(i)) => {
                        if let Some(g) = p.items[*i].group {
                            p.set_collapsed(g, true);
                            collapse_changed = true;
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
                        let (title, collapsed) = (p.title, p.collapsed.clone());
                        // The `p` borrow ends here; persist the toggle (#943).
                        self.palette_collapsed.insert(title, collapsed);
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
                    Some(Command::PreviewPrintInterior) => self.preview_print_interior(),
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
                    Some(Command::PickStyleFlag(cat)) => self.start_style_flag_sweep(cat),
                    Some(Command::ClearStyleFlags) => self.clear_style_flags(),
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
        // A →/← collapse or expand landed on a header (the Enter path persists and
        // returns on its own): remember it for the next open (#943).
        if collapse_changed {
            if let Mode::Palette(p) = &self.mode {
                self.palette_collapsed.insert(p.title, p.collapsed.clone());
            }
        }
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
