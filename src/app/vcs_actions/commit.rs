//! The commit lifecycle (#131, #704): begin a commit, the pre-commit review of
//! the changed files and their diff, and the commit itself. Child module of
//! `vcs_actions`, so these `impl App` methods keep access to `App`'s private
//! fields and reuse the parent's [`diff_lines`](super::diff_lines) colorizer.

use ratatui::style::Modifier;

use super::super::*;
use super::diff_lines;
use crate::theme;

impl App {
    /// Begin a commit (#131, #704): flush unsaved edits so the snapshot captures
    /// them, then open a pre-commit review — the changed-file list and the diff of
    /// what the commit would capture — before the message prompt, so the author
    /// never commits blind. A no-op (with a hint) if the project isn't under version
    /// control, a save fails, or there's nothing to commit.
    pub(super) fn start_commit(&mut self) {
        if !self.under_vc {
            self.message = Some("Not under version control — Ctrl-v to initialize.".into());
            return;
        }
        self.save_edit(); // commit the latest edits, not the last-saved state
        if self.edit_dirty() {
            return; // a save failed; save_edit already set the error message
        }
        match self.commit_review_lines() {
            Ok(None) => self.message = Some("Nothing to commit — working tree clean.".into()),
            Ok(Some(lines)) => self.overlay = Some(Overlay::commit_review(lines)),
            Err(e) => self.message = Some(format!("Commit review failed: {e}")),
        }
    }

    /// The pre-commit review's rows (#704): a header counting the changed files, the
    /// changed-file list, then the colorized diff of what a commit would capture
    /// (working tree vs. the last commit). `Ok(None)` when the tree is clean — there
    /// is nothing to review or commit. Reuses the existing `status` / `diff` helpers.
    pub(crate) fn commit_review_lines(&self) -> Result<Option<Vec<Line<'static>>>, String> {
        let root = &self.project.root;
        let changed = crate::vcs::status(root)?.changed;
        if changed.is_empty() {
            return Ok(None);
        }
        let dim = theme::current().dim;
        let n = changed.len();
        let header = format!(
            "{n} file{} will be committed:",
            if n == 1 { "" } else { "s" }
        );
        let mut lines = vec![Line::from(Span::styled(
            header,
            Style::default().add_modifier(Modifier::BOLD),
        ))];
        // The changed-file list, one dimmed path per row.
        for path in &changed {
            lines.push(Line::from(Span::styled(
                format!("  {}", path.display()),
                Style::default().fg(dim),
            )));
        }
        lines.push(Line::raw(""));
        // The diff of what the commit captures, colorized like the "Changes" view.
        let patch = crate::vcs::diff(root)?;
        if patch.trim().is_empty() {
            lines.push(Line::from(Span::styled(
                "(no textual diff)".to_string(),
                Style::default().fg(dim),
            )));
        } else {
            lines.extend(diff_lines(&patch));
        }
        Ok(Some(lines))
    }

    /// Proceed from the pre-commit review (#704) to the commit-message prompt. The
    /// overlay is already closed by the key handler; here we open the prompt so the
    /// confirmed commit flows through the existing message → [`App::commit`] path.
    pub(crate) fn confirm_commit_review(&mut self) {
        self.open_prompt(Pending::Commit, "Commit message: ", "", None);
    }

    /// Commit the staged project with `message` and report the result.
    pub(crate) fn commit(&mut self, message: &str) {
        self.message = Some(match crate::vcs::commit(&self.project.root, message) {
            Ok(Some(id)) => format!("Committed {id}"),
            Ok(None) => "Nothing to commit — working tree clean.".into(),
            Err(e) => format!("Commit failed: {e}"),
        });
        self.refresh_vc_status(); // the tree is clean now (or unchanged on failure)
    }
}
