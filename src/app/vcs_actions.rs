//! Version-control commands (#54): the Git palette dispatch and the
//! init / commit / history / diff / restore lifecycle plus remote push & pull.
//! Split out of `app/mod.rs` (#150); these are `App` methods, reached through
//! `use super::*`.

use super::*;

/// The result of a remote push/pull (#134), sent back from the worker thread to
/// the UI thread. `reload` asks the UI to re-read the active chapter, since a pull
/// can change files on disk.
pub(crate) struct VcMsg {
    text: String,
    reload: bool,
}

impl App {
    /// Refresh the cached uncommitted-changes count (#132). Cheap (stat-based) but
    /// not free, so it's called on idle and after save / commit / init — never per
    /// frame. Zero when the project isn't under version control.
    pub(crate) fn refresh_vc_status(&mut self) {
        self.vc_changes = if self.under_vc {
            crate::vcs::status(&self.project.root).map_or(0, |s| s.count())
        } else {
            0
        };
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

    /// Begin a commit (#131): flush unsaved edits so the snapshot captures them,
    /// then prompt for a message. A no-op (with a hint) if the project isn't under
    /// version control or a save fails.
    fn start_commit(&mut self) {
        if !self.under_vc {
            self.message = Some("Not under version control — Ctrl-v to initialize.".into());
            return;
        }
        self.save_edit(); // commit the latest edits, not the last-saved state
        if self.edit_dirty() {
            return; // a save failed; save_edit already set the error message
        }
        self.open_prompt(Pending::Commit, "Commit message: ", "", None);
    }

    /// Open the version-control palette (#54) — the single entry point for Git
    /// actions, shown with context-appropriate items.
    pub(crate) fn open_git_palette(&mut self) {
        let git = |label: &str, desc: &str, a| PaletteItem { label: label.into(), desc: desc.into(), command: Command::Git(a) };
        let items = if self.under_vc {
            let has_remote = crate::vcs::remote_url(&self.project.root).is_some();
            let mut items = vec![git("Commit", "Snapshot the project with a message", GitAction::Commit)];
            // Sync actions are only meaningful once a remote is configured.
            if has_remote {
                items.push(git("Push", "Back up your commits to the remote", GitAction::Push));
                items.push(git("Pull", "Bring down the remote's latest changes", GitAction::Pull));
            }
            items.push(git("History", "Browse past commits", GitAction::History));
            items.push(git("Changes", "Diff the working tree against the last commit", GitAction::Diff));
            items.push(git("Restore chapter", "Revert the current chapter to its last commit", GitAction::RestoreChapter));
            items.push(if has_remote {
                git("Change remote", "Point backups at a different remote", GitAction::SetRemote)
            } else {
                git("Set remote", "Back the project up to a hosted remote", GitAction::SetRemote)
            });
            items
        } else {
            vec![git("Initialize", "Put this project under version control", GitAction::Initialize)]
        };
        self.mode = Mode::Palette(Box::new(Palette { title: "Version control", items, selected: 0 }));
    }

    /// Dispatch a version-control action chosen from the palette.
    pub(crate) fn start_git_action(&mut self, action: GitAction) {
        match action {
            GitAction::Initialize => self.init_version_control(),
            GitAction::Commit => self.start_commit(),
            GitAction::History => self.overlay = Some(Overlay::report("history", self.vc_log_lines())),
            GitAction::Diff => self.show_diff(),
            GitAction::RestoreChapter => self.start_restore(),
            GitAction::SetRemote => self.start_set_remote(),
            GitAction::Push => self.start_push(),
            GitAction::Pull => self.start_pull(),
        }
    }

    /// Prompt for the remote URL the project backs up to (#134), prefilled with
    /// the current one so "change remote" shows what's set.
    fn start_set_remote(&mut self) {
        if !self.under_vc {
            self.message = Some("Not under version control — Ctrl-v to initialize.".into());
            return;
        }
        let current = crate::vcs::remote_url(&self.project.root).unwrap_or_default();
        self.open_prompt(Pending::SetRemote, "Remote URL: ", &current, None);
    }

    /// Store the remote URL from the prompt.
    pub(crate) fn set_remote(&mut self, url: &str) {
        if url.is_empty() {
            self.message = Some("Remote unchanged.".into());
            return;
        }
        self.message = Some(match crate::vcs::set_remote(&self.project.root, url) {
            Ok(()) => format!("Remote set to {url}"),
            Err(e) => format!("Set remote: {e}"),
        });
    }

    /// Push committed work to the remote on a worker thread (#134). The network
    /// op runs off the UI thread so the editor stays responsive; the result lands
    /// via [`App::poll_vc`].
    fn start_push(&mut self) {
        if !self.guard_remote() || self.vc_task.is_busy() {
            return;
        }
        self.save_edit(); // flush edits so the status the author sees is accurate
        let root = self.project.root.clone();
        self.start_vc("⟳ Pushing to the remote…", move || match crate::vcs::push(&root) {
            Ok(branch) => VcMsg { text: format!("Pushed {branch} to the remote."), reload: false },
            Err(e) => VcMsg { text: e, reload: false },
        });
    }

    /// Pull the remote's changes on a worker thread (#134). Conservative: refuses
    /// when the working tree is dirty (a fast-forward checkout would overwrite
    /// uncommitted edits), so the author commits first.
    fn start_pull(&mut self) {
        if !self.guard_remote() || self.vc_task.is_busy() {
            return;
        }
        self.leave_continuous(); // the post-pull reload targets the single active chapter
        self.save_edit();
        self.refresh_vc_status();
        if self.vc_changes > 0 {
            self.message = Some("Commit your changes before pulling.".into());
            return;
        }
        let root = self.project.root.clone();
        self.start_vc("⟳ Pulling from the remote…", move || match crate::vcs::pull(&root) {
            Ok(text) => VcMsg { text, reload: true },
            Err(e) => VcMsg { text: e, reload: false },
        });
    }

    /// Run a remote op on a worker thread (#134): spawn `work`, stash the receiver
    /// for [`App::poll_vc`], and show `status` until the result lands. The single
    /// place push/pull cross to a background thread.
    fn start_vc(&mut self, status: &str, work: impl FnOnce() -> VcMsg + Send + 'static) {
        self.vc_task.spawn(work);
        self.message = Some(status.into());
    }

    /// Shared guard for the sync actions: under version control with a remote set.
    /// Sets the explanatory message and returns `false` when not.
    fn guard_remote(&mut self) -> bool {
        if !self.under_vc {
            self.message = Some("Not under version control — Ctrl-v to initialize.".into());
            return false;
        }
        if crate::vcs::remote_url(&self.project.root).is_none() {
            self.message = Some("No remote — choose “Set remote” first.".into());
            return false;
        }
        true
    }

    /// Apply a finished push/pull result (non-blocking). Called once per loop tick.
    pub fn poll_vc(&mut self) {
        match self.vc_task.poll() {
            Poll::Ready(msg) => {
                if msg.reload {
                    let _ = self.reload_active_chapter(); // a pull can rewrite files on disk
                }
                self.refresh_vc_status();
                self.message = Some(msg.text);
            }
            Poll::Pending => {}
            Poll::Died => self.message = Some("Remote operation failed.".into()),
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

    /// Show the working-tree diff since the last commit as a read-only overlay.
    fn show_diff(&mut self) {
        match crate::vcs::diff(&self.project.root) {
            Ok(d) if d.trim().is_empty() => self.message = Some("No uncommitted changes.".into()),
            Ok(d) => self.overlay = Some(Overlay::report("changes", diff_lines(&d))),
            Err(e) => self.message = Some(format!("Diff failed: {e}")),
        }
    }

    /// Confirm restoring the current chapter to its last committed version (#135).
    /// Restore is destructive to that file, so it always asks first.
    fn start_restore(&mut self) {
        self.leave_continuous(); // restore operates on the single active chapter
        let title = self.project.active_title().unwrap_or("this chapter").to_string();
        self.open_prompt(
            Pending::ConfirmRestore,
            format!("Restore '{title}' to last commit? Discards changes (y/N)"),
            "",
            None,
        );
    }

    /// Restore the active chapter from HEAD and reload it into the editor.
    pub(crate) fn restore_chapter(&mut self) {
        let path = self.project.active_path();
        let rel = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        match crate::vcs::restore(&self.project.root, &rel) {
            Ok(true) => {
                self.message = Some(match self.reload_active_chapter() {
                    Ok(()) => "Restored to last commit.".into(),
                    Err(e) => format!("Restored, but reload failed: {e}"),
                });
                self.refresh_vc_status();
            }
            Ok(false) => self.message = Some("No committed version of this chapter to restore.".into()),
            Err(e) => self.message = Some(format!("Restore failed: {e}")),
        }
    }

    /// The commit-history rows for the read-only overlay (#133), newest first.
    fn vc_log_lines(&self) -> Vec<Line<'static>> {
        match crate::vcs::log(&self.project.root, 200) {
            Ok(commits) if commits.is_empty() => vec![Line::raw("No commits yet.")],
            Ok(commits) => commits
                .iter()
                .map(|c| Line::raw(format!("{}  {}  {}", c.when, c.id, c.summary)))
                .collect(),
            Err(e) => vec![Line::raw(format!("Could not read history: {e}"))],
        }
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
