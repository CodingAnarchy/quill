//! The single-key confirmations ([`Pending::is_confirm`]): y/N prompts and their
//! kin, answered on the first key. Most close the prompt and then act on the
//! answer; a few drive the mode themselves on `y` (or on cancel).

use crossterm::event::KeyCode;

use crate::ai::BackendKind;
use crate::mode::{Pending, WizardStep};

use super::super::App;

impl App {
    /// Answer the confirmation `pending` with `code`. `nav_return` is where the
    /// prompt returns when it closes without acting.
    pub(super) fn on_key_confirm(
        &mut self,
        pending: Pending,
        code: KeyCode,
        nav_return: Option<usize>,
    ) {
        let yes = matches!(code, KeyCode::Char('y' | 'Y'));
        let closed = self.closed_mode(nav_return);
        match pending {
            // These leave (or replace) the mode themselves on `y`.
            Pending::Quit if yes => {
                // Deliberately quitting despite unsaved changes is an explicit
                // discard, so don't re-offer this work next launch (#263).
                self.clear_recovery();
                self.should_quit = true;
            }
            Pending::Delete { at } if yes => {
                let selected = self.delete_chapter(at);
                self.select(selected);
            }
            Pending::Merge { at } if yes => {
                let selected = self.merge_chapter(at);
                self.select(selected);
            }
            // restore_pending_backup reopens the project (resetting the mode) on
            // success, or restores a resting mode itself on failure.
            Pending::ConfirmRestoreBackup if yes => self.restore_pending_backup(),
            // switch_to saves the current project, then replaces the app state
            // (which resets the mode); on failure it restores a resting mode itself.
            Pending::ConfirmSwitch if yes => match self.pending_switch.take() {
                Some(target) => self.switch_to(target),
                None => self.mode = closed,
            },
            Pending::ConfirmDeleteProject if yes => self.delete_browser_project(),
            Pending::ConfirmDeleteProject => {
                // Cancel: drop the stashed target, back to the browser (#612).
                self.pending_manage = None;
                self.reopen_project_browser();
            }
            Pending::WizardStart if yes => self.wizard_open(Some(WizardStep::Goals)),
            // Apply the AI-proposed config changes (#303); the editing mode that
            // follows reflects any newly-toggled Vim setting.
            Pending::ConfirmConfig if yes => {
                self.write_config_changes();
                self.mode = self.editing_mode();
            }
            // Everything else closes the prompt first, then acts on the answer.
            _ => {
                self.mode = closed;
                self.answer_confirm(pending, code, yes);
            }
        }
    }

    /// Act on a confirmation's answer once the prompt has closed.
    fn answer_confirm(&mut self, pending: Pending, code: KeyCode, yes: bool) {
        match pending {
            Pending::ConfirmAi {
                action,
                force,
                offer_local,
            } => match code {
                // Confirm: run on the paid cloud backend.
                KeyCode::Char('y' | 'Y') => {
                    self.run_confirmed_ai(action, force, BackendKind::Claude);
                }
                // The hybrid router's third option (#372): run this one on-device.
                KeyCode::Char('l' | 'L') if offer_local => {
                    self.run_confirmed_ai(action, force, BackendKind::Local);
                }
                // Declining a workflow step's cost confirm cancels the run.
                _ => self.decline_workflow(),
            },
            Pending::ConfirmRecovery => match code {
                KeyCode::Char('y' | 'Y' | 'r' | 'R') => self.restore_recovery(),
                KeyCode::Char('d' | 'D') => {
                    self.pending_recovery = None;
                    self.clear_recovery();
                    self.message = Some("Discarded recovered work.".into());
                }
                // Esc / any other key: keep the sidecar, decide next launch.
                _ => self.pending_recovery = None,
            },
            Pending::ClearAnnotations if yes => self.clear_annotations(),
            Pending::ConfirmRefreshBibles { force } if yes => self.refresh_bibles_confirmed(force),
            Pending::ConfirmQueuePasses { force } if yes => self.queue_passes_confirmed(force),
            Pending::ResumePassQueue if yes => self.resume_pass_queue(),
            Pending::ResumePassQueue => self.discard_pass_queue(),
            Pending::ConfirmBatchUpdate if yes => self.submit_batch_update(),
            Pending::ConfirmRemoveModel(id) if yes => self.remove_model(id),
            Pending::StripPlaintextKey(which) if yes => self.strip_plaintext_key(which),
            // Write `[ai] enabled = true`, then chain (key flow or the pass the
            // author reached for). AI stays off on any other key (#602).
            Pending::ConfirmEnableAi { resume } if yes => self.enable_ai(resume),
            Pending::ConfirmRestore if yes => self.restore_chapter(),
            Pending::ConfirmRestoreProject if yes => self.restore_project(),
            Pending::ConfirmRemoveBook { index } if yes => self.remove_book(index),
            Pending::ConfirmPortraits if yes => self.generate_portraits(),
            Pending::ConfirmScene if yes => self.generate_scene(),
            Pending::ConfirmToShortStory if yes => self.convert_to_short_story_confirmed(),
            Pending::ConfirmToNovel if yes => self.grow_into_novel_confirmed(),
            Pending::ConfirmRemoveStory { index } if yes => self.remove_story(index),
            _ => self.decline_confirm(pending),
        }
    }

    /// A declined confirmation: drop any target staged for it, and say so where
    /// the decline is worth reporting.
    fn decline_confirm(&mut self, pending: Pending) {
        match pending {
            Pending::ConfirmRestoreProject => self.pending_restore_rev = None,
            Pending::ConfirmRestoreBackup => self.pending_restore_backup = None,
            Pending::ConfirmSwitch => self.pending_switch = None,
            Pending::ConfirmPortraits => self.pending_portraits = None, // the picked selection
            Pending::ConfirmConfig => self.pending_config = None,
            _ => {}
        }
        let message = match pending {
            Pending::ConfirmRefreshBibles { .. } => "Bible refresh cancelled.",
            Pending::ConfirmQueuePasses { .. } => "Pass queue cancelled.",
            Pending::ConfirmBatchUpdate => "Background bible update cancelled.",
            // Leaving it is fine — it stays the lowest-precedence fallback.
            Pending::StripPlaintextKey(_) => "Left the plaintext key in the config.",
            Pending::ConfirmEnableAi { .. } => "AI left off.",
            Pending::ConfirmRestoreBackup => "Restore cancelled.",
            Pending::ConfirmRemoveBook { .. } => "Book kept.",
            Pending::ConfirmToShortStory => "Left as a novel.",
            Pending::ConfirmToNovel => "Left as a short story.",
            Pending::ConfirmRemoveStory { .. } => "Story kept in the collection.",
            _ => return,
        };
        self.message = Some(message.into());
    }
}
