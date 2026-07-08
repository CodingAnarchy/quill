//! The prompt / confirmation state machine: `on_key_prompt` and the helpers
//! that open, route, and close prompts and ex-commands. Extracted from `mod.rs` (#314).

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::mode::{Mode, Pending, PromptState, WizardStep};
use crate::text::{match_ranges_opts, MatchOpts};

use super::App;

impl App {
    /// Where an overlay (prompt or metadata editor) returns when it closes
    /// without acting: back to the chapter browser at `selected`, or to the
    /// editor's resting mode.
    pub(super) const fn closed_mode(&self, nav_return: Option<usize>) -> Mode {
        match nav_return {
            Some(selected) => Mode::Nav {
                selected,
                filter: String::new(),
                typing: false,
            },
            None => self.editing_mode(),
        }
    }

    /// Save the current chapter (if dirty), then load `index` as the active
    /// chapter. The active index and the live buffer always move together here,
    /// so the two never drift. A no-op if `index` is already active or invalid.
    /// Save the active chapter if it has unsaved edits and refresh its cached
    /// word count. Returns `Err(())` (and sets a status message) if the save
    /// failed, so callers can abort before switching away from it.
    pub(super) fn flush_active(&mut self) -> Result<(), ()> {
        if self.buffer.dirty {
            if let Err(e) = self.buffer.save() {
                self.message = Some(format!("Save failed: {e}"));
                return Err(());
            }
        }
        // The chapter is now on disk; refresh its cached count.
        self.cache_focus_words();
        Ok(())
    }

    /// Open a prompt. `nav_return` is the chapter to reselect when it closes
    /// (`None` returns to insert mode, e.g. the quit prompt).
    pub(super) fn open_prompt(
        &mut self,
        pending: Pending,
        label: impl Into<String>,
        prefill: &str,
        nav_return: Option<usize>,
    ) {
        self.mode = Mode::Prompt(PromptState {
            nav_return,
            ..PromptState::new(pending, label.into(), prefill.to_string())
        });
    }

    /// Enter (or stay in) the chapter browser with `selected` highlighted.
    /// Moving the selection within the browser preserves any active filter /
    /// quick-jump query (#410); entering from another mode starts unfiltered.
    pub(super) fn select(&mut self, selected: usize) {
        if let Mode::Nav { selected: s, .. } = &mut self.mode {
            *s = selected;
        } else {
            self.mode = Mode::Nav {
                selected,
                filter: String::new(),
                typing: false,
            };
        }
    }

    /// Open the Vim Ex command line (#207): `:` then a command (`w`/`q`/`wq`/…).
    pub(super) fn open_ex_command(&mut self) {
        self.vim_state.op = None;
        self.open_prompt(Pending::ExCommand, ":", "", None);
    }

    /// Run a typed Ex command (#207). The supported set is the file commands a
    /// Vim user reaches for first, plus a bare line number (`:{N}`) and a
    /// whole-buffer substitute (`:%s/pat/rep/[g]`, #826); anything else is
    /// reported, like Vim's E492.
    fn run_ex_command(&mut self, cmd: &str) {
        match cmd {
            "" => {} // bare `:` — nothing to do
            "w" => self.save_edit(),
            // `:q` prompts on unsaved changes, exactly as Ctrl-Q does.
            "q" => self.request_quit(),
            // Write then quit; don't quit if the write failed (buffer still dirty).
            "wq" | "x" => {
                self.save_edit();
                if !self.edit_dirty() {
                    self.should_quit = true;
                }
            }
            "q!" => {
                self.clear_recovery(); // explicit discard — don't re-offer next launch (#263)
                self.should_quit = true;
            }
            // `:{N}` — jump to line N (1-based, clamped to the buffer), like Vim.
            _ if !cmd.is_empty() && cmd.bytes().all(|b| b.is_ascii_digit()) => {
                self.ex_goto_line(cmd);
            }
            // `:%s/pat/rep/[flags]` — whole-buffer substitute (#826).
            _ if cmd.starts_with("%s") => self.ex_substitute(&cmd[2..]),
            other => self.message = Some(format!("Not an editor command: {other}")),
        }
    }

    /// `:{N}` — move the cursor to 1-based line `n`, clamped to the buffer's last
    /// content line, landing on its first non-blank column like Vim's `{count}G`.
    fn ex_goto_line(&mut self, digits: &str) {
        // Only reached with an all-digit `cmd`, but a value too large for a
        // `usize` still means "past the end", so clamp to the last line.
        let n: usize = digits.parse().unwrap_or(usize::MAX);
        // 1-based line → 0-based index. Clamp to the last line with content
        // (skipping a trailing newline's empty line), so `:999` lands where
        // Vim's `G` would (#826).
        let target = n.saturating_sub(1);
        let last = self.edit_buffer().last_line_index();
        self.edit_buffer_mut().move_to_line(target.min(last));
    }

    /// `:%s/pat/rep/[flags]` — substitute `pat` with `rep` across the whole
    /// buffer, reusing the range-edit path find/replace goes through (#826). The
    /// `g` flag replaces every match on each line; without it, only the first per
    /// line (Vim's default). A missing trailing delimiter is tolerated. Reports
    /// the number of substitutions.
    fn ex_substitute(&mut self, rest: &str) {
        // The delimiter is the first character after `%s` (Vim allows any; `/`
        // is by far the most common). An empty argument is a malformed command.
        let Some(delim) = rest.chars().next() else {
            self.message = Some("Not an editor command: %s".into());
            return;
        };
        // Split into pattern / replacement / flags on the delimiter. A trailing
        // delimiter (and thus the flags field) may be omitted.
        let body = &rest[delim.len_utf8()..];
        let mut parts = body.splitn(3, delim);
        let pattern = parts.next().unwrap_or("");
        let replacement = parts.next().unwrap_or("");
        let flags = parts.next().unwrap_or("");
        if pattern.is_empty() {
            self.message = Some("E33: No previous substitute pattern".into());
            return;
        }
        let global = flags.contains('g');

        // Match over the whole buffer, then keep either every match (`g`) or the
        // first on each line (Vim's per-line default). `%s` is case-sensitive by
        // default, like Vim without `ignorecase`.
        let text = self.edit_buffer().text();
        let opts = MatchOpts {
            case_sensitive: true,
            whole_word: false,
            regex: false,
        };
        let ranges = match_ranges_opts(&text, pattern, opts);
        let targets: Vec<(usize, usize)> = if global {
            ranges
        } else {
            let mut last_line: Option<usize> = None;
            ranges
                .into_iter()
                .filter(|&(start, _)| {
                    let line = text[..start].bytes().filter(|&b| b == b'\n').count();
                    if last_line == Some(line) {
                        false
                    } else {
                        last_line = Some(line);
                        true
                    }
                })
                .collect()
        };

        // Replace right-to-left so earlier byte offsets stay valid as the text
        // shifts — the same range-edit path find/replace uses, so it is undoable.
        let mut replaced = 0usize;
        for &(start, end) in targets.iter().rev() {
            self.edit_replace_range(start, end, replacement);
            replaced += 1;
        }
        if replaced > 0 {
            self.cache_focus_words();
        }
        self.message = Some(match replaced {
            0 => format!("Pattern not found: {pattern}"),
            1 => "Replaced 1 occurrence.".into(),
            n => format!("Replaced {n} occurrences."),
        });
    }

    /// The prompt state, while in [`Mode::Prompt`]. Only call from a prompt key
    /// handler, which has already matched the variant.
    fn prompt_mut(&mut self) -> &mut PromptState {
        match &mut self.mode {
            Mode::Prompt(state) => state,
            _ => unreachable!("prompt editing runs only in Prompt mode"),
        }
    }

    pub(super) fn on_key_prompt(&mut self, key: KeyEvent) {
        let Mode::Prompt(state) = &self.mode else {
            return;
        };
        let pending = state.pending;
        let nav_return = state.nav_return;

        // Where to go when the prompt closes without acting.
        let closed = self.closed_mode(nav_return);

        match pending {
            Pending::Quit => match key.code {
                KeyCode::Char('y' | 'Y') => {
                    // Deliberately quitting despite unsaved changes is an explicit
                    // discard, so don't re-offer this work next launch (#263).
                    self.clear_recovery();
                    self.should_quit = true;
                }
                _ => self.mode = closed,
            },
            Pending::Delete { at } => match key.code {
                KeyCode::Char('y' | 'Y') => {
                    let selected = self.delete_chapter(at);
                    self.select(selected);
                }
                _ => self.mode = closed,
            },
            Pending::Merge { at } => match key.code {
                KeyCode::Char('y' | 'Y') => {
                    let selected = self.merge_chapter(at);
                    self.select(selected);
                }
                _ => self.mode = closed,
            },
            Pending::ConfirmAi {
                action,
                force,
                offer_local,
            } => match key.code {
                // Confirm: run on the paid cloud backend.
                KeyCode::Char('y' | 'Y') => {
                    self.mode = closed;
                    self.run_confirmed_ai(action, force, crate::ai::BackendKind::Claude);
                }
                // The hybrid router's third option (#372): run this one on-device.
                KeyCode::Char('l' | 'L') if offer_local => {
                    self.mode = closed;
                    self.run_confirmed_ai(action, force, crate::ai::BackendKind::Local);
                }
                _ => {
                    self.mode = closed;
                    // Declining a workflow step's cost confirm cancels the run.
                    self.decline_workflow();
                }
            },
            Pending::ConfirmRefreshBibles { force } => {
                if let KeyCode::Char('y' | 'Y') = key.code {
                    self.mode = closed;
                    self.refresh_bibles_confirmed(force);
                } else {
                    self.mode = closed;
                    self.message = Some("Bible refresh cancelled.".into());
                }
            }
            Pending::ConfirmQueuePasses { force } => {
                if let KeyCode::Char('y' | 'Y') = key.code {
                    self.mode = closed;
                    self.queue_passes_confirmed(force);
                } else {
                    self.mode = closed;
                    self.message = Some("Pass queue cancelled.".into());
                }
            }
            Pending::ResumePassQueue => {
                if let KeyCode::Char('y' | 'Y') = key.code {
                    self.mode = closed;
                    self.resume_pass_queue();
                } else {
                    self.mode = closed;
                    self.discard_pass_queue();
                }
            }
            Pending::ConfirmRemoveModel(id) => match key.code {
                KeyCode::Char('y' | 'Y') => {
                    self.mode = closed;
                    self.remove_model(id);
                }
                _ => self.mode = closed,
            },
            Pending::StripPlaintextKey(which) => {
                if let KeyCode::Char('y' | 'Y') = key.code {
                    self.mode = closed;
                    self.strip_plaintext_key(which);
                } else {
                    // Leaving it is fine — it stays the lowest-precedence fallback.
                    self.mode = closed;
                    self.message = Some("Left the plaintext key in the config.".into());
                }
            }
            Pending::ConfirmEnableAi { resume } => {
                // Confirm: write `[ai] enabled = true`, then chain (key flow or the
                // pass the author reached for). AI stays off on any other key (#602).
                if let KeyCode::Char('y' | 'Y') = key.code {
                    self.mode = closed;
                    self.enable_ai(resume);
                } else {
                    self.mode = closed;
                    self.message = Some("AI left off.".into());
                }
            }
            Pending::ConfirmRestore => match key.code {
                KeyCode::Char('y' | 'Y') => {
                    self.mode = closed;
                    self.restore_chapter();
                }
                _ => self.mode = closed,
            },
            Pending::ConfirmRestoreProject => {
                if let KeyCode::Char('y' | 'Y') = key.code {
                    self.mode = closed;
                    self.restore_project();
                } else {
                    self.pending_restore_rev = None; // cancelled — drop the target
                    self.mode = closed;
                }
            }
            Pending::ConfirmRestoreBackup => {
                if let KeyCode::Char('y' | 'Y') = key.code {
                    // restore_pending_backup reopens the project (resetting the
                    // mode) on success, or restores a resting mode itself on failure.
                    self.restore_pending_backup();
                } else {
                    self.pending_restore_backup = None; // cancelled — drop the target
                    self.mode = closed;
                    self.message = Some("Restore cancelled.".into());
                }
            }
            Pending::ConfirmRecovery => match key.code {
                KeyCode::Char('y' | 'Y' | 'r' | 'R') => {
                    self.mode = closed;
                    self.restore_recovery();
                }
                KeyCode::Char('d' | 'D') => {
                    self.mode = closed;
                    self.pending_recovery = None;
                    self.clear_recovery();
                    self.message = Some("Discarded recovered work.".into());
                }
                _ => {
                    // Esc / any other key: keep the sidecar, decide next launch.
                    self.mode = closed;
                    self.pending_recovery = None;
                }
            },
            Pending::ConfirmSwitch => {
                if let KeyCode::Char('y' | 'Y') = key.code {
                    // switch_to saves the current project, then replaces the app
                    // state (which resets the mode); on failure it restores a
                    // resting mode itself.
                    match self.pending_switch.take() {
                        Some(target) => self.switch_to(target),
                        None => self.mode = closed,
                    }
                } else {
                    self.pending_switch = None;
                    self.mode = closed;
                }
            }
            Pending::ConfirmRemoveBook { index } => {
                if let KeyCode::Char('y' | 'Y') = key.code {
                    self.mode = closed;
                    self.remove_book(index);
                } else {
                    self.mode = closed;
                    self.message = Some("Book kept.".into());
                }
            }
            Pending::ConfirmDeleteProject => {
                if let KeyCode::Char('y' | 'Y') = key.code {
                    self.delete_browser_project();
                } else {
                    // Cancel: drop the stashed target, back to the browser (#612).
                    self.pending_manage = None;
                    self.reopen_project_browser();
                }
            }
            Pending::WizardStart => match key.code {
                KeyCode::Char('y' | 'Y') => {
                    self.wizard_open(Some(WizardStep::Goals));
                }
                _ => self.mode = closed, // declined now — land in the editor
            },
            Pending::ConfirmPortraits => {
                if let KeyCode::Char('y' | 'Y') = key.code {
                    self.mode = closed;
                    self.generate_portraits();
                } else {
                    self.pending_portraits = None; // discard the picked selection
                    self.mode = closed;
                }
            }
            Pending::ConfirmScene => match key.code {
                KeyCode::Char('y' | 'Y') => {
                    self.mode = closed;
                    self.generate_scene();
                }
                _ => self.mode = closed,
            },
            // Apply the AI-proposed config changes (#303), or cancel (changing
            // nothing). `y` writes and applies them; the editing mode that follows
            // reflects any newly-toggled Vim setting.
            Pending::ConfirmConfig => {
                if let KeyCode::Char('y' | 'Y') = key.code {
                    self.write_config_changes();
                    self.mode = self.editing_mode();
                } else {
                    self.pending_config = None;
                    self.mode = closed;
                }
            }
            // The text prompts share the editing keys; only Enter differs.
            Pending::GoToChapter
            | Pending::NewChapter { .. }
            | Pending::SplitChapter { .. }
            | Pending::Duplicate { .. }
            | Pending::Rename { .. }
            | Pending::SetMeta(_)
            | Pending::SetGoal(_)
            | Pending::SetBookField(_)
            | Pending::SetStyleText(_)
            | Pending::SetSettingText(_)
            | Pending::AskAi
            | Pending::SemanticFind
            | Pending::Thesaurus
            | Pending::NameGen(_)
            | Pending::Commit
            | Pending::Milestone
            | Pending::SetRemote
            | Pending::ExCommand
            | Pending::DescribeWorkflow
            | Pending::DescribeConfig
            | Pending::NameNewWorkflow
            | Pending::Brainstorm
            | Pending::DraftCharacters
            | Pending::DraftSetting
            | Pending::NewProject
            | Pending::ConvertToSeries
            | Pending::AddBook
            | Pending::RenameProject
            | Pending::RenameBrowserProject
            | Pending::Wizard(_)
            | Pending::ImportNotes
            | Pending::NewFromNotesSource
            | Pending::Annotate { .. }
            | Pending::AnnotateReply { .. }
            | Pending::SetSceneSynopsis { .. }
            | Pending::NewFromNotesDest
            | Pending::ImportManuscriptSource
            | Pending::ImportManuscriptDest
            | Pending::ImportReviewComments
            | Pending::ArchiveProjectDest
            | Pending::RestoreArchiveSource
            | Pending::RestoreArchiveDest
            | Pending::SetApiKey(_)
            | Pending::SprintDuration => {
                // Line editing shared by every text prompt (#706): typing and
                // deletion happen at the caret, which the arrows / Home / End
                // move and Ctrl/Alt-Backspace word-deletes back through.
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let alt = key.modifiers.contains(KeyModifiers::ALT);
                match key.code {
                    KeyCode::Char(c) => self.prompt_mut().insert(c),
                    KeyCode::Backspace if ctrl || alt => self.prompt_mut().delete_word_back(),
                    KeyCode::Backspace => self.prompt_mut().backspace(),
                    KeyCode::Delete => self.prompt_mut().delete_forward(),
                    KeyCode::Left => self.prompt_mut().move_left(),
                    KeyCode::Right => self.prompt_mut().move_right(),
                    KeyCode::Home => self.prompt_mut().move_home(),
                    KeyCode::End => self.prompt_mut().move_end(),
                    KeyCode::Enter => {
                        let text = std::mem::take(&mut self.prompt_mut().input);
                        let text = text.trim();
                        let fallback = nav_return.unwrap_or(0);
                        match pending {
                            Pending::GoToChapter => {
                                // Jump straight to a chapter by 1-based number or
                                // title substring (#588); a blank entry or no match
                                // returns to the browser unchanged.
                                if let Some(index) = self.resolve_chapter_target(text) {
                                    // Record the jump-back target (#825) so `` `` ``
                                    // returns to the chapter the author left.
                                    self.push_jump();
                                    self.activate_chapter(index);
                                    self.browser_opened_outline = false;
                                    self.mode = self.editing_mode();
                                } else {
                                    if !text.is_empty() {
                                        self.message =
                                            Some(format!("No chapter matches '{text}'."));
                                    }
                                    self.select(fallback);
                                }
                            }
                            Pending::NewChapter { at } => {
                                let sel = if text.is_empty() {
                                    fallback
                                } else {
                                    self.create_chapter(at, text)
                                };
                                self.select(sel);
                            }
                            Pending::Rename { at } => {
                                if !text.is_empty() {
                                    self.rename_chapter(at, text);
                                }
                                self.select(fallback);
                            }
                            Pending::SplitChapter { at } => {
                                let sel = if text.is_empty() {
                                    fallback
                                } else {
                                    self.split_chapter(at, text)
                                };
                                self.select(sel);
                            }
                            Pending::Duplicate { at } => {
                                let sel = if text.is_empty() {
                                    fallback
                                } else {
                                    self.duplicate_chapter(at, text)
                                };
                                self.select(sel);
                            }
                            Pending::SetMeta(field) => {
                                // Single-line metadata prompts (summary, POV,
                                // attribution, book fields). The multi-line
                                // epigraph quotations are edited in the editor
                                // (Mode::EditMeta), which chains here for the
                                // attribution. A role change re-sorts the list, so
                                // follow the chapter to its new index (#180).
                                let moved = self.apply_meta(field, text);
                                self.select(moved.unwrap_or(fallback));
                            }
                            Pending::SetGoal(field) => {
                                // Validate, persist to `[goals]`, and chain to the
                                // next goal field (or re-prompt on a bad value). The
                                // flow drives the mode itself, so don't reset it here.
                                self.apply_goal(field, text);
                            }
                            Pending::SetBookField(field) => {
                                // Persist this book field (title keeps its current
                                // value on blank; the optional fields clear), then
                                // chain to the next. The flow drives the mode
                                // itself, so don't reset it here.
                                self.apply_book_field(field, text);
                            }
                            Pending::SetStyleText(knob) => {
                                // Splice the `[style]` knob (blank clears it). The
                                // setter re-opens the style menu itself, so don't
                                // reset the mode here.
                                self.apply_style_text(knob, text);
                            }
                            Pending::SetSettingText(knob) => {
                                // Validate + persist the numeric / list Settings
                                // knob, apply it live, and re-open the Settings
                                // menu. The setter drives the mode itself, so don't
                                // reset it here.
                                self.apply_setting_text(knob, text);
                            }
                            Pending::Annotate { start, end } => {
                                // Back to editing; the note anchors to its range (#302).
                                self.mode = closed;
                                self.create_annotation(start, end, text.to_string());
                            }
                            Pending::AnnotateReply { index } => {
                                // Append the reply and re-open the review list (#889);
                                // the setter drives the mode, so don't reset it here.
                                self.add_annotation_reply(index, text.to_string());
                            }
                            Pending::SetSceneSynopsis {
                                chapter,
                                scene,
                                manuscript,
                            } => {
                                // Persist the synopsis (blank clears it); the setter
                                // re-opens the corkboard itself, so don't reset the
                                // mode here (#636 / #701).
                                self.apply_scene_synopsis(chapter, scene, manuscript, text);
                            }
                            Pending::AskAi => {
                                // Back to editing immediately; the answer arrives
                                // asynchronously in the overlay.
                                self.mode = closed;
                                self.ask_ai(text);
                            }
                            Pending::SemanticFind => {
                                // Back to editing; the ranked similar passages arrive
                                // in the findings overlay (#924).
                                self.mode = closed;
                                self.semantic_find(text);
                            }
                            Pending::Thesaurus => {
                                // Back to editing; the candidates arrive in the overlay.
                                self.mode = closed;
                                self.thesaurus(text);
                            }
                            Pending::NameGen(kind) => {
                                // Back to editing; the candidate names arrive as a
                                // pickable list overlay (#610).
                                self.mode = closed;
                                self.namegen(kind, text);
                            }
                            Pending::Commit => {
                                self.mode = closed;
                                if text.is_empty() {
                                    self.message = Some("Commit cancelled — empty message.".into());
                                } else {
                                    self.commit(text);
                                }
                            }
                            Pending::Milestone => {
                                self.mode = closed;
                                self.create_milestone(text);
                            }
                            Pending::SetRemote => {
                                self.mode = closed;
                                self.set_remote(text);
                            }
                            Pending::ExCommand => {
                                self.mode = closed; // back to the editor, then run it
                                self.run_ex_command(text);
                            }
                            Pending::DescribeWorkflow => {
                                // Back to editing; the proposal arrives asynchronously.
                                self.mode = closed;
                                self.generate_workflow(text);
                            }
                            Pending::DescribeConfig => {
                                // Back to editing; the proposal arrives asynchronously (#303).
                                self.mode = closed;
                                self.generate_config(text);
                            }
                            Pending::Brainstorm => {
                                // Back to editing; the ideas arrive asynchronously.
                                self.mode = closed;
                                self.brainstorm(text);
                            }
                            Pending::DraftCharacters => {
                                // Back to editing; the profiles arrive asynchronously.
                                self.mode = closed;
                                self.draft_characters(text);
                            }
                            Pending::DraftSetting => {
                                // Back to editing; the setting bible arrives asynchronously.
                                self.mode = closed;
                                self.draft_setting(text);
                            }
                            Pending::NameNewWorkflow => {
                                self.mode = closed;
                                self.save_generated_workflow(text);
                            }
                            Pending::NewProject => {
                                // A trailing separator means no name was typed
                                // after the prefilled location (#273).
                                if text.is_empty()
                                    || text.ends_with('/')
                                    || text.ends_with(std::path::MAIN_SEPARATOR)
                                {
                                    self.mode = closed;
                                    self.message = Some(
                                        "Enter a path ending in the new project's folder name."
                                            .into(),
                                    );
                                } else {
                                    self.create_project_at(PathBuf::from(text));
                                }
                            }
                            Pending::ConvertToSeries => {
                                // convert_to_series persists the series and chains
                                // to the add-a-book prompt itself (#648), so don't
                                // reset the mode here.
                                self.convert_to_series(text);
                            }
                            Pending::AddBook => {
                                // add_book creates the book and restores the editing
                                // mode; a blank title re-prompts there (#648).
                                self.add_book(text);
                            }
                            Pending::RenameProject => {
                                // rename_project restores the editing mode and
                                // reports; a blank name is rejected there (#424).
                                self.rename_project(text);
                            }
                            Pending::RenameBrowserProject => {
                                // rename_browser_project moves the stashed folder
                                // and re-lists the browser; a blank name is
                                // rejected there (#612).
                                self.rename_browser_project(text);
                            }
                            Pending::Wizard(step) => {
                                // Apply this step (blank = skip), then move to the
                                // next one — or finish (#282).
                                self.apply_wizard_step(step, text);
                                self.wizard_open(step.next());
                            }
                            Pending::ImportNotes => {
                                self.mode = closed;
                                if text.is_empty() {
                                    self.message = Some("Import cancelled — no path.".into());
                                } else {
                                    self.import_notes(text);
                                }
                            }
                            Pending::NewFromNotesSource => {
                                if text.is_empty() {
                                    self.mode = closed;
                                    self.message = Some("Cancelled — no notes folder.".into());
                                } else {
                                    // Validate the folder, then ask for the new project's path.
                                    self.begin_new_from_notes(text);
                                }
                            }
                            Pending::NewFromNotesDest => {
                                if text.is_empty()
                                    || text.ends_with('/')
                                    || text.ends_with(std::path::MAIN_SEPARATOR)
                                {
                                    self.mode = closed;
                                    self.pending_import_source = None;
                                    self.message = Some(
                                        "Enter a path ending in the new project's folder name."
                                            .into(),
                                    );
                                } else {
                                    self.finish_new_from_notes(text);
                                }
                            }
                            Pending::ImportManuscriptSource => {
                                if text.is_empty() {
                                    self.mode = closed;
                                    self.message = Some("Cancelled — no manuscript file.".into());
                                } else {
                                    // Validate the file, then ask for the new project's path.
                                    self.begin_import_manuscript(text);
                                }
                            }
                            Pending::ImportManuscriptDest => {
                                if text.is_empty()
                                    || text.ends_with('/')
                                    || text.ends_with(std::path::MAIN_SEPARATOR)
                                {
                                    self.mode = closed;
                                    self.pending_import_source = None;
                                    self.message = Some(
                                        "Enter a path ending in the new project's folder name."
                                            .into(),
                                    );
                                } else {
                                    self.finish_import_manuscript(text);
                                }
                            }
                            Pending::ImportReviewComments => {
                                if text.is_empty() {
                                    self.mode = closed;
                                    self.message = Some("Cancelled — no .docx file.".into());
                                } else {
                                    // Recover the comments, resolve them, and open
                                    // the review-import preview (or report + stop).
                                    self.begin_import_review_comments(text);
                                }
                            }
                            Pending::ArchiveProjectDest => {
                                // finish_archive_project restores the editing mode
                                // and reports; a blank path is rejected there (#594).
                                self.finish_archive_project(text);
                            }
                            Pending::RestoreArchiveSource => {
                                // begin_restore_archive validates the archive, then
                                // asks for the new folder (or reports and stops).
                                self.begin_restore_archive(text);
                            }
                            Pending::RestoreArchiveDest => {
                                // finish_restore_archive unpacks and opens (or reports).
                                self.finish_restore_archive(text);
                            }
                            Pending::SprintDuration => {
                                // begin_sprint restores the editing mode and reports.
                                self.begin_sprint(text);
                            }
                            Pending::SetApiKey(which) => {
                                // Back to editing; store_api_key may chain a strip
                                // confirm, which opens its own prompt (#530).
                                self.mode = closed;
                                self.store_api_key(which, text);
                            }
                            // Confirmations never reach the text arm.
                            _ => self.select(fallback),
                        }
                    }
                    KeyCode::Esc => {
                        // Cancelling discards any prompt-scoped staging: the AI
                        // workflow proposal (#232), the new-from-notes folder
                        // (#286), and the browser rename target (#612). Harmless
                        // no-op for the other prompts.
                        self.pending_workflow = None;
                        self.pending_import_source = None;
                        // A cancelled browser rename returns to the library, not
                        // the editor, so the author stays where they were (#612).
                        if matches!(pending, Pending::RenameBrowserProject) {
                            self.pending_manage = None;
                            self.reopen_project_browser();
                        } else if let Pending::AnnotateReply { index } = pending {
                            // A cancelled reply returns to the review list, not the
                            // editor, so the author stays with the thread (#889).
                            self.reopen_annotation_review(index);
                        } else {
                            self.mode = closed;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
