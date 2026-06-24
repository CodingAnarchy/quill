//! The first-run setup wizard (#282): step through title/author/setting and
//! seed the bible, draining queued AI drafts. Extracted from `mod.rs` (#314).





use crate::mode::{
    Pending, WizardStep,
};
use crate::project::Goals;

use super::App;

impl App {
    /// Offer the setup wizard for a freshly-created project: a y/N gate, then a
    /// short sequence of skippable steps (goals, characters, setting, notes).
    pub(super) fn start_wizard(&mut self) {
        self.open_prompt(
            Pending::WizardStart,
            "Set up this project now — goals, characters, setting, notes? (y/N)",
            "",
            None,
        );
    }

    /// Open the prompt for `step`, or finish the wizard when there are no steps
    /// left. AI-dependent steps are skipped outright when AI isn't ready, so the
    /// wizard works the same with AI off.
    pub(super) fn wizard_open(&mut self, step: Option<WizardStep>) {
        let mut step = step;
        while let Some(s) = step {
            if s.needs_ai() && self.ai.check_ready().is_err() {
                step = s.next();
                continue;
            }
            self.open_prompt(Pending::Wizard(s), Self::wizard_label(s), "", None);
            return;
        }
        // No steps left — land in a normal, editable project.
        self.mode = self.editing_mode();
        self.message = Some("Project ready.".into());
    }

    /// The prompt label for a wizard step. Each notes that blank skips and Esc
    /// dismisses the rest.
    fn wizard_label(step: WizardStep) -> &'static str {
        match step {
            WizardStep::Goals => "Goals — target words and chapters, e.g. \"80000 30\" (blank skips · Esc finishes)",
            WizardStep::Characters => "Characters — describe the cast for the AI to draft (blank skips · Esc finishes)",
            WizardStep::Setting => "Setting — describe the world for the AI to draft (blank skips · Esc finishes)",
            WizardStep::Notes => "Notes — a starting note to save (blank skips · Esc finishes)",
        }
    }

    /// Apply one wizard step's input (a no-op when blank — the step is skipped).
    pub(super) fn apply_wizard_step(&mut self, step: WizardStep, text: &str) {
        if text.is_empty() {
            return;
        }
        match step {
            WizardStep::Goals => {
                let nums: Vec<usize> = text.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                match nums[..] {
                    [words, chapters, ..] if words > 0 && chapters > 0 => {
                        self.project.manifest.goals = Some(Goals { words, chapters, words_per_page: 250, daily: None });
                        self.message = match self.project.save_manifest() {
                            Ok(()) => Some("Writing goals set.".into()),
                            Err(e) => Some(format!("Couldn't save goals: {e}")),
                        };
                    }
                    _ => self.message = Some("Skipped goals — give target words and chapters, e.g. 80000 30.".into()),
                }
            }
            WizardStep::Characters => self.draft_characters(text),
            WizardStep::Setting => {
                // Only one AI pass runs at a time; if the characters draft is
                // still in flight, queue this for when it finishes (poll_ai).
                if self.ai_task.is_busy() {
                    self.wizard_pending_setting = Some(text.to_string());
                } else {
                    self.draft_setting(text);
                }
            }
            WizardStep::Notes => {
                let path = self.project.root.join("notes.md");
                let body = format!("# Notes\n\n{text}\n");
                self.message = match crate::atomic::write(&path, body.as_bytes()) {
                    Ok(()) => Some("Saved notes.md.".into()),
                    Err(e) => Some(format!("Couldn't save notes: {e}")),
                };
            }
        }
    }

    /// Fire a wizard's deferred setting draft once the characters draft frees the
    /// AI worker (#282). Called from `poll_ai`.
    pub(crate) fn pump_wizard_drafts(&mut self) {
        if !self.ai_task.is_busy() {
            if let Some(concept) = self.wizard_pending_setting.take() {
                self.draft_setting(&concept);
            }
        }
    }
}
