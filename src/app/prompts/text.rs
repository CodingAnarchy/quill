//! The text prompts — every [`Pending`] that isn't a confirmation. They share the
//! line-editing keys (#706); only what Enter does differs, which [`App::submit_text`]
//! resolves per prompt.

use std::mem;
use std::path::{PathBuf, MAIN_SEPARATOR};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::mode::Pending;

use super::super::App;

/// Why a new-project destination was refused: it names no folder to create.
const NO_FOLDER_NAME: &str = "Enter a path ending in the new project's folder name.";

/// Whether a new-project destination path names the folder to create — a blank
/// entry or a trailing separator means no name was typed after the prefilled
/// location (#273).
fn names_new_folder(text: &str) -> bool {
    !(text.is_empty() || text.ends_with('/') || text.ends_with(MAIN_SEPARATOR))
}

impl App {
    /// A key in a text prompt: line editing at the caret (the arrows / Home / End
    /// move it, Ctrl/Alt-Backspace word-deletes back), Enter submits, Esc cancels.
    pub(super) fn on_key_text_prompt(
        &mut self,
        pending: Pending,
        key: KeyEvent,
        nav_return: Option<usize>,
    ) {
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
                let text = mem::take(&mut self.prompt_mut().input);
                self.submit_text(pending, text.trim(), nav_return);
            }
            KeyCode::Esc => self.cancel_text_prompt(pending, nav_return),
            _ => {}
        }
    }

    /// Esc in a text prompt.
    fn cancel_text_prompt(&mut self, pending: Pending, nav_return: Option<usize>) {
        // Cancelling discards any prompt-scoped staging: the AI workflow proposal
        // (#232), the new-from-notes folder (#286), and the browser rename target
        // (#612). Harmless no-op for the other prompts.
        self.pending_workflow = None;
        self.pending_import_source = None;
        match pending {
            // A cancelled browser rename returns to the library, not the editor,
            // so the author stays where they were (#612).
            Pending::RenameBrowserProject => {
                self.pending_manage = None;
                self.reopen_project_browser();
            }
            // A cancelled reply returns to the review list, not the editor, so the
            // author stays with the thread (#889).
            Pending::AnnotateReply { index } => self.reopen_annotation_review(index),
            _ => self.mode = self.closed_mode(nav_return),
        }
    }

    /// Enter in a text prompt: act on the (trimmed) `text`. Prompts that close
    /// before acting are resolved by [`submit_closed`](Self::submit_closed); the
    /// rest drive the mode themselves (reopening a menu, chaining a prompt, …).
    fn submit_text(&mut self, pending: Pending, text: &str, nav_return: Option<usize>) {
        let fallback = nav_return.unwrap_or(0);
        match pending {
            Pending::GoToChapter => {
                // Jump straight to a chapter by 1-based number or title substring
                // (#588); a blank entry or no match returns to the browser unchanged.
                if let Some(index) = self.resolve_chapter_target(text) {
                    // Record the jump-back target (#825) so `` `` `` returns to the
                    // chapter the author left.
                    self.push_jump();
                    self.activate_chapter(index);
                    self.browser_opened_outline = false;
                    self.mode = self.editing_mode();
                } else {
                    if !text.is_empty() {
                        self.message = Some(format!("No chapter matches '{text}'."));
                    }
                    self.select(fallback);
                }
            }
            Pending::AddStoryPath => self.add_story(text),
            Pending::NewStoryTitle => self.write_new_story(text),
            Pending::StoryCredit { index } => self.set_story_credit(index, text),
            Pending::LocateStory { index } => self.locate_story(index, text),
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
                // Single-line metadata prompts (summary, POV, attribution, book
                // fields). The multi-line epigraph quotations are edited in the
                // editor (Mode::EditMeta), which chains here for the attribution. A
                // role change re-sorts the list, so follow the chapter to its new
                // index (#180).
                let moved = self.apply_meta(field, text);
                self.select(moved.unwrap_or(fallback));
            }
            // Validate, persist to `[goals]`, and chain to the next goal field (or
            // re-prompt on a bad value).
            Pending::SetGoal(field) => self.apply_goal(field, text),
            // Persist this book field (title keeps its current value on blank; the
            // optional fields clear), then chain to the next.
            Pending::SetBookField(field) => self.apply_book_field(field, text),
            // Splice the `[style]` knob (blank clears it); re-opens the style menu.
            Pending::SetStyleText(knob) => self.apply_style_text(knob, text),
            // Validate + persist the numeric / list Settings knob, apply it live,
            // and re-open the Settings menu.
            Pending::SetSettingText(knob) => self.apply_setting_text(knob, text),
            // Append the reply and re-open the review list (#889).
            Pending::AnnotateReply { index } => {
                self.add_annotation_reply(index, text.to_string());
            }
            // Persist the synopsis (blank clears it); re-opens the corkboard
            // (#636 / #701).
            Pending::SetSceneSynopsis {
                chapter,
                scene,
                manuscript,
            } => self.apply_scene_synopsis(chapter, scene, manuscript, text),
            // Chains straight into the optional-note prompt (#966).
            Pending::Milestone => self.prompt_milestone_note(text),
            Pending::NewProject | Pending::NewFromNotesDest | Pending::ImportManuscriptDest
                if !names_new_folder(text) =>
            {
                self.mode = self.closed_mode(nav_return);
                if !matches!(pending, Pending::NewProject) {
                    self.pending_import_source = None;
                }
                self.message = Some(NO_FOLDER_NAME.into());
            }
            Pending::NewProject => self.create_project_at(PathBuf::from(text)),
            Pending::NewFromNotesDest => self.finish_new_from_notes(text),
            Pending::ImportManuscriptDest => self.finish_import_manuscript(text),
            // Persists the series and chains to the add-a-book prompt (#648).
            Pending::ConvertToSeries => self.convert_to_series(text),
            // Creates the book and restores the editing mode; a blank title
            // re-prompts there (#648).
            Pending::AddBook => self.add_book(text),
            // Restores the editing mode and reports; a blank name is rejected
            // there (#424).
            Pending::RenameProject => self.rename_project(text),
            // Moves the stashed folder and re-lists the browser; a blank name is
            // rejected there (#612).
            Pending::RenameBrowserProject => self.rename_browser_project(text),
            Pending::Wizard(step) => {
                // Apply this step (blank = skip), then move to the next one — or
                // finish (#282).
                self.apply_wizard_step(step, text);
                self.wizard_open(step.next());
            }
            Pending::NewFromNotesSource if !text.is_empty() => self.begin_new_from_notes(text),
            Pending::ImportManuscriptSource if !text.is_empty() => {
                self.begin_import_manuscript(text);
            }
            // Recover the comments, resolve them, and open the review-import preview
            // (or report + stop).
            Pending::ImportReviewComments if !text.is_empty() => {
                self.begin_import_review_comments(text);
            }
            Pending::NewFromNotesSource
            | Pending::ImportManuscriptSource
            | Pending::ImportReviewComments => {
                self.mode = self.closed_mode(nav_return);
                self.message = Some(
                    match pending {
                        Pending::NewFromNotesSource => "Cancelled — no notes folder.",
                        Pending::ImportManuscriptSource => "Cancelled — no manuscript file.",
                        _ => "Cancelled — no .docx file.",
                    }
                    .into(),
                );
            }
            // Restores the editing mode and reports; a blank path is rejected
            // there (#594).
            Pending::ArchiveProjectDest => self.finish_archive_project(text),
            // Validates the archive, then asks for the new folder (or reports).
            Pending::RestoreArchiveSource => self.begin_restore_archive(text),
            // Unpacks and opens (or reports).
            Pending::RestoreArchiveDest => self.finish_restore_archive(text),
            // Restores the editing mode and reports.
            Pending::SprintDuration => self.begin_sprint(text),
            // Restores the editing mode and reports; a blank entry clears any
            // running target.
            Pending::SessionTarget => self.begin_session_target(text),
            _ => {
                self.mode = self.closed_mode(nav_return);
                self.submit_closed(pending, text);
            }
        }
    }

    /// Enter in a text prompt that closes before acting — back to where it was
    /// opened from, then run it (an async request's result arrives in an overlay;
    /// some handlers reopen a view or chain a prompt of their own).
    fn submit_closed(&mut self, pending: Pending, text: &str) {
        match pending {
            // Move to the entered line (#1010); a blank / non-numeric entry just
            // closes.
            Pending::GoToLine if !text.is_empty() => self.go_to_line_input(text),
            // The note anchors to its range (#302).
            Pending::Annotate { start, end } => {
                self.create_annotation(start, end, text.to_string());
            }
            Pending::AiArg(action) => self.submit_ai_arg(action, text),
            // The ranked similar passages arrive in the findings overlay (#924).
            Pending::SemanticFind => self.semantic_find(text),
            Pending::Thesaurus => self.thesaurus(text),
            // The candidate names arrive as a pickable list overlay (#610).
            Pending::NameGen(kind) => self.namegen(kind, text),
            Pending::Commit if text.is_empty() => {
                self.message = Some("Commit cancelled — empty message.".into());
            }
            Pending::Commit => self.commit(text),
            Pending::MilestoneNote => self.create_milestone(text),
            // Reopens the history view.
            Pending::EditMilestoneNote => self.edit_milestone_note(text),
            // Reopens the drafts overlay (or the editor on failure) (#965).
            Pending::CreateDraft => self.create_draft(text),
            Pending::SetRemote => self.set_remote(text),
            Pending::ExCommand => self.run_ex_command(text),
            // The proposal arrives asynchronously (#232 / #303).
            Pending::DescribeWorkflow => self.generate_workflow(text),
            Pending::DescribeConfig => self.generate_config(text),
            Pending::NameNewWorkflow => self.save_generated_workflow(text),
            Pending::ImportNotes if text.is_empty() => {
                self.message = Some("Import cancelled — no path.".into());
            }
            Pending::ImportNotes => self.import_notes(text),
            // store_api_key may chain a strip confirm, which opens its own prompt
            // (#530).
            Pending::SetApiKey(which) => self.store_api_key(which, text),
            _ => {}
        }
    }
}
