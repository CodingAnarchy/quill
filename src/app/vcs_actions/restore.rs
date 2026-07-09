//! Milestones (#833), the working-tree / whole-project diffs, and the chapter /
//! whole-project restores (#135, #833). Child module of `vcs_actions`, so these
//! `impl App` methods keep access to `App`'s private fields and reuse the
//! parent's [`diff_lines`](super::diff_lines) colorizer and
//! [`reload_active_chapter`](App::reload_active_chapter) helper.

use super::super::*;
use super::diff_lines;

impl App {
    /// Show the working-tree diff since the last commit as a read-only overlay.
    pub(super) fn show_diff(&mut self) {
        match crate::vcs::diff(&self.project.root) {
            Ok(d) if d.trim().is_empty() => self.message = Some("No uncommitted changes.".into()),
            Ok(d) => self.overlay = Some(Overlay::report("changes", diff_lines(&d))),
            Err(e) => self.message = Some(format!("Diff failed: {e}")),
        }
    }

    /// Prompt for a milestone name (#833), then for an optional note (#966), then
    /// create the annotated tag on the current HEAD. Flushes unsaved edits and
    /// commits nothing itself — a milestone marks the last committed state, so it
    /// hints when there's uncommitted work rather than silently tagging a stale
    /// snapshot.
    pub(super) fn start_milestone(&mut self) {
        if !self.under_vc {
            self.message = Some("Not under version control — Ctrl-v to initialize.".into());
            return;
        }
        self.save_edit();
        self.open_prompt(Pending::Milestone, "Milestone name: ", "", None);
    }

    /// After the milestone name is entered (#833), ask for its optional note (#966).
    /// A blank name cancels here, before the note prompt opens; a valid name is
    /// stashed in `pending_milestone_name` for [`create_milestone`] to pick up when
    /// the note comes back.
    pub(crate) fn prompt_milestone_note(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.mode = self.editing_mode(); // close the name prompt, don't chain
            self.message = Some("Milestone cancelled — empty name.".into());
            return;
        }
        self.pending_milestone_name = Some(name.to_string());
        self.open_prompt(
            Pending::MilestoneNote,
            "Milestone note (optional): ",
            "",
            None,
        );
    }

    /// Create the named milestone (#833) from the stashed name and the just-entered
    /// `note` (#966) — an annotated tag over the whole project on the current HEAD.
    /// The note becomes the tag's annotation, shown beside the milestone in the
    /// history view; a blank note leaves the milestone without one.
    pub(crate) fn create_milestone(&mut self, note: &str) {
        let Some(name) = self.pending_milestone_name.take() else {
            return;
        };
        let note = note.trim();
        self.message = Some(
            match crate::vcs::create_tag(&self.project.root, &name, note) {
                Ok(()) => format!("Milestone '{name}' created."),
                Err(e) => format!("Milestone: {e}"),
            },
        );
    }

    /// Edit the note on the milestone selected in the history view (#966). Stashes
    /// its name and opens a note prompt prefilled with the current note, so the
    /// author revises it in place. The prompt returns to [`edit_milestone_note`].
    pub(crate) fn start_edit_milestone_note(&mut self, name: &str, note: &str) {
        self.pending_milestone_name = Some(name.to_string());
        self.open_prompt(Pending::EditMilestoneNote, "Milestone note: ", note, None);
    }

    /// Re-annotate the stashed milestone with the just-entered `note` (#966),
    /// leaving the revision it marks unchanged. Reopens the history view so the
    /// edited note shows immediately.
    pub(crate) fn edit_milestone_note(&mut self, note: &str) {
        let Some(name) = self.pending_milestone_name.take() else {
            return;
        };
        match crate::vcs::set_tag_message(&self.project.root, &name, note.trim()) {
            Ok(()) => {
                self.message = Some(format!("Milestone '{name}' note updated."));
                self.open_history(); // refresh the list so the new note shows
            }
            Err(e) => self.message = Some(format!("Milestone: {e}")),
        }
    }

    /// Show the whole-manuscript diff since the last commit as a read-only overlay
    /// (#833) — the project-scoped counterpart to [`show_diff`], which already spans
    /// the tree, so this flushes edits and reuses it under a project-scoped title.
    pub(super) fn show_project_diff(&mut self) {
        self.save_edit(); // diff against the latest edits, not the last-saved state
        match crate::vcs::diff(&self.project.root) {
            Ok(d) if d.trim().is_empty() => {
                self.message = Some("No uncommitted changes across the manuscript.".into());
            }
            Ok(d) => self.overlay = Some(Overlay::report("project changes", diff_lines(&d))),
            Err(e) => self.message = Some(format!("Diff failed: {e}")),
        }
    }

    /// Confirm restoring the current chapter to its last committed version (#135).
    /// Restore is destructive to that file, so it always asks first.
    pub(super) fn start_restore(&mut self) {
        self.leave_continuous(); // restore operates on the single active chapter
        let title = self
            .project
            .active_title()
            .unwrap_or("this chapter")
            .to_string();
        self.open_prompt(
            Pending::ConfirmRestore,
            format!("Restore '{title}' to last commit? Discards changes (y/N)"),
            "",
            None,
        );
    }

    /// Restore the active chapter from HEAD and reload it into the editor.
    pub(crate) fn restore_chapter(&mut self) {
        let rel = self.active_chapter_rel();
        match crate::vcs::restore(&self.project.root, &rel) {
            Ok(true) => {
                self.message = Some(match self.reload_active_chapter() {
                    Ok(()) => "Restored to last commit.".into(),
                    Err(e) => format!("Restored, but reload failed: {e}"),
                });
                self.refresh_vc_status();
            }
            Ok(false) => {
                self.message = Some("No committed version of this chapter to restore.".into());
            }
            Err(e) => self.message = Some(format!("Restore failed: {e}")),
        }
    }

    /// Confirm restoring the *whole project* to a revision (#833) — a milestone or
    /// commit from the history view (`rev = Some`), or the last commit from the Git
    /// palette (`rev = None` → HEAD). Restore is destructive across the manuscript,
    /// so it always asks first; the target waits in `pending_restore_rev`.
    pub(crate) fn start_restore_project(&mut self, rev: Option<String>) {
        if !self.under_vc {
            self.message = Some("Not under version control — Ctrl-v to initialize.".into());
            return;
        }
        self.leave_continuous(); // the post-restore reload targets the active chapter
        self.save_edit();
        let target = rev.unwrap_or_else(|| "HEAD".to_string());
        let label = if target == "HEAD" {
            "the last commit".to_string()
        } else {
            let short: String = target.chars().take(7).collect();
            format!("'{short}'")
        };
        self.pending_restore_rev = Some(target);
        self.open_prompt(
            Pending::ConfirmRestoreProject,
            format!("Restore the whole project to {label}? Discards all changes (y/N)"),
            "",
            None,
        );
    }

    /// Restore the whole working tree to the pending revision (#833) and reload the
    /// active chapter — a pull-style refresh, since every file may have changed on
    /// disk. Unlike the chapter restore (which loads into the editable buffer for
    /// review), a project restore writes the tree directly, matching how a pull
    /// lands the remote's files.
    pub(crate) fn restore_project(&mut self) {
        let Some(rev) = self.pending_restore_rev.take() else {
            return;
        };
        match crate::vcs::restore_all(&self.project.root, &rev) {
            Ok(()) => {
                self.message = Some(match self.reload_active_chapter() {
                    Ok(()) => "Restored the project.".into(),
                    Err(e) => format!("Restored, but reload failed: {e}"),
                });
                self.refresh_vc_status();
            }
            Err(e) => self.message = Some(format!("Restore failed: {e}")),
        }
    }
}
