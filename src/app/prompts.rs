//! The prompt / confirmation state machine: `on_key_prompt` and the helpers
//! that open, route, and close prompts and ex-commands. Extracted from `mod.rs` (#314).

use std::path::PathBuf;


use crossterm::event::{KeyCode, KeyEvent};


use crate::mode::{
    Mode, Pending,
    PromptState, WizardStep,
};

use super::App;

impl App {
    /// Where an overlay (prompt or metadata editor) returns when it closes
    /// without acting: back to the chapter browser at `selected`, or to the
    /// editor's resting mode.
    pub(super) fn closed_mode(&self, nav_return: Option<usize>) -> Mode {
        match nav_return {
            Some(selected) => Mode::Nav { selected },
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
            pending,
            label: label.into(),
            input: prefill.to_string(),
            nav_return,
        });
    }

    /// Enter (or stay in) the chapter browser with `selected` highlighted.
    pub(super) fn select(&mut self, selected: usize) {
        self.mode = Mode::Nav { selected };
    }

    /// Open the Vim Ex command line (#207): `:` then a command (`w`/`q`/`wq`/…).
    pub(super) fn open_ex_command(&mut self) {
        self.vim_state.op = None;
        self.open_prompt(Pending::ExCommand, ":", "", None);
    }

    /// Run a typed Ex command (#207). The supported set is the file commands a
    /// Vim user reaches for first; anything else is reported, like Vim's E492.
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
            other => self.message = Some(format!("Not an editor command: {other}")),
        }
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
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Deliberately quitting despite unsaved changes is an explicit
                    // discard, so don't re-offer this work next launch (#263).
                    self.clear_recovery();
                    self.should_quit = true;
                }
                _ => self.mode = closed,
            },
            Pending::Delete { at } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let selected = self.delete_chapter(at);
                    self.select(selected);
                }
                _ => self.mode = closed,
            },
            Pending::ConfirmAi { action, force, offer_local } => match key.code {
                // Confirm: run on the paid cloud backend.
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.mode = closed;
                    self.run_confirmed_ai(action, force, crate::ai::BackendKind::Claude);
                }
                // The hybrid router's third option (#372): run this one on-device.
                KeyCode::Char('l') | KeyCode::Char('L') if offer_local => {
                    self.mode = closed;
                    self.run_confirmed_ai(action, force, crate::ai::BackendKind::Local);
                }
                _ => {
                    self.mode = closed;
                    // Declining a workflow step's cost confirm cancels the run.
                    self.decline_workflow();
                }
            },
            Pending::ConfirmRemoveModel(id) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.mode = closed;
                    self.remove_model(id);
                }
                _ => self.mode = closed,
            },
            Pending::ConfirmRestore => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.mode = closed;
                    self.restore_chapter();
                }
                _ => self.mode = closed,
            },
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
            Pending::ConfirmSwitch => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // switch_to saves the current project, then replaces the app
                    // state (which resets the mode); on failure it restores a
                    // resting mode itself.
                    match self.pending_switch.take() {
                        Some(target) => self.switch_to(target),
                        None => self.mode = closed,
                    }
                }
                _ => {
                    self.pending_switch = None;
                    self.mode = closed;
                }
            },
            Pending::WizardStart => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.wizard_open(Some(WizardStep::Goals)),
                _ => self.mode = closed, // declined now — land in the editor
            },
            Pending::ConfirmPortraits => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.mode = closed;
                    self.generate_portraits();
                }
                _ => {
                    self.pending_portraits = None; // discard the picked selection
                    self.mode = closed;
                }
            },
            Pending::ConfirmScene => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.mode = closed;
                    self.generate_scene();
                }
                _ => self.mode = closed,
            },
            // Apply the AI-proposed config changes (#303), or cancel (changing
            // nothing). `y` writes and applies them; the editing mode that follows
            // reflects any newly-toggled Vim setting.
            Pending::ConfirmConfig => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.write_config_changes();
                    self.mode = self.editing_mode();
                }
                _ => {
                    self.pending_config = None;
                    self.mode = closed;
                }
            },
            // The text prompts share the editing keys; only Enter differs.
            Pending::NewChapter { .. }
            | Pending::Rename { .. }
            | Pending::SetMeta(_)
            | Pending::AskAi
            | Pending::Thesaurus
            | Pending::Commit
            | Pending::SetRemote
            | Pending::ExCommand
            | Pending::DescribeWorkflow
            | Pending::DescribeConfig
            | Pending::NameNewWorkflow
            | Pending::Brainstorm
            | Pending::DraftCharacters
            | Pending::DraftSetting
            | Pending::NewProject
            | Pending::Wizard(_)
            | Pending::ImportNotes
            | Pending::NewFromNotesSource
            | Pending::Annotate { .. }
            | Pending::NewFromNotesDest
            | Pending::ImportManuscriptSource
            | Pending::ImportManuscriptDest
            | Pending::SprintDuration => {
                match key.code {
                    KeyCode::Char(c) => self.prompt_mut().input.push(c),
                    KeyCode::Backspace => {
                        self.prompt_mut().input.pop();
                    }
                    KeyCode::Enter => {
                        let text = std::mem::take(&mut self.prompt_mut().input);
                        let text = text.trim();
                        let fallback = nav_return.unwrap_or(0);
                        match pending {
                            Pending::NewChapter { at } => {
                                let sel =
                                    if text.is_empty() { fallback } else { self.create_chapter(at, text) };
                                self.select(sel);
                            }
                            Pending::Rename { at } => {
                                if !text.is_empty() {
                                    self.rename_chapter(at, text);
                                }
                                self.select(fallback);
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
                            Pending::Annotate { start, end } => {
                                // Back to editing; the note anchors to its range (#302).
                                self.mode = closed;
                                self.create_annotation(start, end, text.to_string());
                            }
                            Pending::AskAi => {
                                // Back to editing immediately; the answer arrives
                                // asynchronously in the overlay.
                                self.mode = closed;
                                self.ask_ai(text);
                            }
                            Pending::Thesaurus => {
                                // Back to editing; the candidates arrive in the overlay.
                                self.mode = closed;
                                self.thesaurus(text);
                            }
                            Pending::Commit => {
                                self.mode = closed;
                                if text.is_empty() {
                                    self.message = Some("Commit cancelled — empty message.".into());
                                } else {
                                    self.commit(text);
                                }
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
                                if text.is_empty() || text.ends_with('/') || text.ends_with(std::path::MAIN_SEPARATOR) {
                                    self.mode = closed;
                                    self.message = Some("Enter a path ending in the new project's folder name.".into());
                                } else {
                                    self.create_project_at(PathBuf::from(text));
                                }
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
                                if text.is_empty() || text.ends_with('/') || text.ends_with(std::path::MAIN_SEPARATOR) {
                                    self.mode = closed;
                                    self.pending_notes_folder = None;
                                    self.message = Some("Enter a path ending in the new project's folder name.".into());
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
                                if text.is_empty() || text.ends_with('/') || text.ends_with(std::path::MAIN_SEPARATOR) {
                                    self.mode = closed;
                                    self.pending_manuscript_source = None;
                                    self.message = Some("Enter a path ending in the new project's folder name.".into());
                                } else {
                                    self.finish_import_manuscript(text);
                                }
                            }
                            Pending::SprintDuration => {
                                // begin_sprint restores the editing mode and reports.
                                self.begin_sprint(text);
                            }
                            // Confirmations never reach the text arm.
                            _ => self.select(fallback),
                        }
                    }
                    KeyCode::Esc => {
                        // Cancelling discards any prompt-scoped staging: the AI
                        // workflow proposal (#232) and the new-from-notes folder
                        // (#286). Harmless no-op for the other prompts.
                        self.pending_workflow = None;
                        self.pending_notes_folder = None;
                        self.mode = closed;
                    }
                    _ => {}
                }
            }
        }
    }
}
