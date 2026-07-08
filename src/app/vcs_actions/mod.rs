//! Version-control commands (#54): the Git palette dispatch and the
//! init / commit / history / diff / restore lifecycle plus remote push & pull.
//! Split out of `app/mod.rs` (#150); once it outgrew a single file (#925), split
//! by concern into cohesive submodules, each `impl App` and reached through
//! `use super::super::*`:
//!
//! - [`status`] — the cached uncommitted-changes count and its #779 idle
//!   debounce (with the #869 backoff), plus the `#[cfg(test)]` settle helpers.
//! - [`commit`] — `init` and the commit lifecycle: the pre-commit review (#704)
//!   and the commit itself.
//! - [`remote`] — the remote push / pull worker (#134), its `poll_vc` landing
//!   path (#870), and the remote URL prompt.
//! - [`history`] — the navigable commit-history view (#587) and its
//!   type-to-filter sub-mode (#822).
//! - [`restore`] — milestones (#833), the working-tree / project diffs, and the
//!   chapter / whole-project restores.
//!
//! Shared here at the module root: the [`VcMsg`] worker result, the palette
//! dispatch, and the [`diff_lines`] colorizer / [`reload_active_chapter`] helper
//! the submodules reuse. The public surface is re-exported so callers elsewhere
//! in `app` are unaffected by the split.

use super::*;

mod commit;
mod history;
mod remote;
mod restore;
mod status;

pub use status::VC_STATUS_INTERVAL;

/// The result of a remote push/pull (#134), sent back from the worker thread to
/// the UI thread. `reload` asks the UI to re-read the active chapter, since a pull
/// can change files on disk.
pub struct VcMsg {
    text: String,
    reload: bool,
}

impl App {
    /// Open the version-control palette (#54) — the single entry point for Git
    /// actions, shown with context-appropriate items.
    pub(crate) fn open_git_palette(&mut self) {
        let git = |label: &str, desc: &str, a| PaletteItem::new(label, desc, Command::Git(a));
        let items = if self.under_vc {
            let has_remote = crate::vcs::remote_url(&self.project.root).is_some();
            let mut items = vec![git(
                "Commit",
                "Snapshot the project with a message",
                GitAction::Commit,
            )];
            // Sync actions are only meaningful once a remote is configured.
            if has_remote {
                items.push(git(
                    "Push",
                    "Back up your commits to the remote",
                    GitAction::Push,
                ));
                items.push(git(
                    "Pull",
                    "Bring down the remote's latest changes",
                    GitAction::Pull,
                ));
            }
            items.push(git(
                "Milestone",
                "Mark a named manuscript version (e.g. “first draft”)",
                GitAction::Milestone,
            ));
            items.push(git(
                "History",
                "Browse past commits and milestones",
                GitAction::History,
            ));
            items.push(git(
                "Changes",
                "Diff the working tree against the last commit",
                GitAction::Diff,
            ));
            items.push(git(
                "Project changes",
                "Diff the whole manuscript against the last commit",
                GitAction::DiffProject,
            ));
            items.push(git(
                "Restore chapter",
                "Revert the current chapter to its last commit",
                GitAction::RestoreChapter,
            ));
            items.push(git(
                "Restore project",
                "Revert the whole manuscript to its last commit",
                GitAction::RestoreProject,
            ));
            items.push(if has_remote {
                git(
                    "Change remote",
                    "Point backups at a different remote",
                    GitAction::SetRemote,
                )
            } else {
                git(
                    "Set remote",
                    "Back the project up to a hosted remote",
                    GitAction::SetRemote,
                )
            });
            items
        } else {
            vec![git(
                "Initialize",
                "Put this project under version control",
                GitAction::Initialize,
            )]
        };
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Version control",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Dispatch a version-control action chosen from the palette.
    pub(crate) fn start_git_action(&mut self, action: GitAction) {
        match action {
            GitAction::Initialize => self.init_version_control(),
            GitAction::Commit => self.start_commit(),
            GitAction::History => self.open_history(),
            GitAction::Diff => self.show_diff(),
            GitAction::DiffProject => self.show_project_diff(),
            GitAction::RestoreChapter => self.start_restore(),
            GitAction::RestoreProject => self.start_restore_project(None),
            GitAction::Milestone => self.start_milestone(),
            GitAction::SetRemote => self.start_set_remote(),
            GitAction::Push => self.start_push(),
            GitAction::Pull => self.start_pull(),
        }
    }

    /// Put the project under version control, or report that it already is (#130).
    /// Opt-in and non-destructive: it never clobbers an existing repo or
    /// `.gitignore`.
    fn init_version_control(&mut self) {
        if self.under_vc {
            self.message = Some("Already under version control.".into());
            return;
        }
        match crate::vcs::init(&self.project.root) {
            Ok(()) => {
                self.under_vc = true;
                self.refresh_vc_status();
                self.message = Some("Initialized version control (git).".into());
            }
            Err(e) => self.message = Some(format!("Version control: {e}")),
        }
    }

    /// Re-read the active chapter from disk into the buffer, after something (a
    /// pull, a restore) rewrote the file underneath the editor. Returns the read
    /// error so callers can report it; on failure the current buffer is left in
    /// place.
    fn reload_active_chapter(&mut self) -> Result<(), String> {
        let path = self.project.active_path();
        let buffer = Buffer::from_path(&path).map_err(|e| e.to_string())?;
        self.buffer = buffer;
        self.cache_focus_words();
        Ok(())
    }

    /// The active chapter's path relative to the repo root — the file name, since
    /// chapters live at the project root. The handle the per-file version-control
    /// operations (restore, diff/restore-from-a-commit) address.
    fn active_chapter_rel(&self) -> String {
        self.project
            .active_path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string()
    }
}

/// Color a unified diff for the changes overlay (#135): additions green, removals
/// red, file/hunk headers and context dimmed. Mirrors the bible-review diff.
fn diff_lines(patch: &str) -> Vec<Line<'static>> {
    let theme = crate::theme::current();
    patch
        .lines()
        .map(|l| {
            let color = match l.chars().next() {
                Some('+') => theme.added,
                Some('-') => theme.removed,
                _ => theme.dim,
            };
            Line::from(Span::styled(l.to_string(), Style::default().fg(color)))
        })
        .collect()
}
