//! Version-control commands (#54): the Git palette dispatch and the
//! init / commit / history / diff / restore lifecycle plus remote push & pull.
//! Split out of `app/mod.rs` (#150); these are `App` methods, reached through
//! `use super::*`.

use super::*;
use crate::theme;
use ratatui::style::Modifier;

/// The result of a remote push/pull (#134), sent back from the worker thread to
/// the UI thread. `reload` asks the UI to re-read the active chapter, since a pull
/// can change files on disk.
pub struct VcMsg {
    text: String,
    reload: bool,
}

/// How long the cached uncommitted-changes count may stand before the idle path
/// recomputes it to catch edits made outside Quill (#779). Long enough that pure
/// typing doesn't re-walk the tree every input burst, short enough that an
/// external change surfaces promptly.
const VC_STATUS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(4);

impl App {
    /// Refresh the cached uncommitted-changes count (#132) synchronously — a
    /// `.gitignore`-respecting working-tree walk. Called after commit / init /
    /// restore / pull, where the status has demonstrably just changed and the
    /// count must be current before the next frame. The per-idle-tick refresh
    /// takes the debounced, off-thread [`refresh_vc_status_on_idle`] path instead
    /// (#779). Zero when the project isn't under version control.
    pub(crate) fn refresh_vc_status(&mut self) {
        self.vc_changes = if self.under_vc {
            crate::vcs::status(&self.project.root).map_or(0, |s| s.count())
        } else {
            0
        };
        // The count is current now; reset the interval clock and clear the pending
        // recompute so the idle path doesn't immediately redo this walk.
        self.last_vc_refresh = std::time::Instant::now();
        self.vc_status_dirty = false;
    }

    /// Mark that the working tree may have changed (#779), so the next idle tick
    /// recomputes the uncommitted-changes count off-thread. Called from the save
    /// path, where an in-app edit was just written to disk — the one moment pure
    /// typing can flip git status — without blocking the save on a status walk.
    pub(crate) const fn mark_vc_status_dirty(&mut self) {
        self.vc_status_dirty = true;
    }

    /// The idle-tick refresh of the uncommitted-changes count (#779). Unlike the
    /// synchronous [`refresh_vc_status`], this never walks the tree on the UI
    /// thread and never walks it merely because the author typed: it recomputes
    /// only when a save marked the status dirty or the bounded interval has
    /// elapsed (the fallback that catches external edits), and it runs the walk on
    /// a worker thread so a large repository can't stutter the editor.
    pub(crate) fn refresh_vc_status_on_idle(&mut self) {
        // First, land a finished walk's count.
        if let Poll::Ready(count) = self.vc_status_task.poll() {
            self.vc_changes = count;
        }
        if !self.under_vc {
            self.vc_changes = 0;
            return;
        }
        // Only walk when the status can have changed (a save marked it dirty) or
        // the interval fallback is due — never on every idle burst from typing.
        let due = self.vc_status_dirty || self.last_vc_refresh.elapsed() >= VC_STATUS_INTERVAL;
        if !due || self.vc_status_task.is_busy() {
            return;
        }
        self.vc_status_dirty = false;
        self.last_vc_refresh = std::time::Instant::now();
        let root = self.project.root.clone();
        self.vc_status_task
            .spawn(move || crate::vcs::status(&root).map_or(0, |s| s.count()));
    }

    /// Whether an off-thread status walk is in flight or a save has marked the
    /// count stale (#779) — the two states in which [`refresh_vc_status_on_idle`]
    /// still has work to land. Lets a test spin the idle path deterministically to
    /// a settled count without a timing race.
    #[cfg(test)]
    pub(crate) const fn vc_status_settling(&self) -> bool {
        self.vc_status_dirty || self.vc_status_task.is_busy()
    }

    /// Whether the status has been marked stale (a save happened) — the specific
    /// invariant pure typing must NOT trip (#779). Unlike `vc_status_settling` this
    /// ignores an in-flight walk, so a test can assert "typing marked nothing"
    /// without racing the wall-clock interval fallback, which may legitimately
    /// spawn a walk on an idle tick regardless of typing.
    #[cfg(test)]
    pub(crate) const fn vc_status_marked_dirty(&self) -> bool {
        self.vc_status_dirty
    }

    /// Backdate the interval clock so the next idle refresh's fallback is due
    /// (#779), letting a test exercise the "catch an external edit" path without
    /// waiting real wall-clock seconds.
    #[cfg(test)]
    pub(crate) fn expire_vc_status_interval_for_test(&mut self) {
        self.last_vc_refresh = std::time::Instant::now()
            .checked_sub(VC_STATUS_INTERVAL)
            .unwrap_or_else(std::time::Instant::now);
    }

    /// Drive the debounced idle status refresh (#779) to completion for tests:
    /// tick the idle path until a save-marked walk has been spawned and its count
    /// applied, so an assertion sees the settled `vc_changes` rather than racing
    /// the worker thread. Bounded so a stuck walk can't hang the test.
    #[cfg(test)]
    pub(crate) fn settle_vc_status(&mut self) {
        // Sleep (not `yield_now`) between polls: under heavy parallel test load the
        // off-thread git walk's worker can be CPU-starved, and a busy spin never
        // relinquishes the core for it to run. Sleeping yields the CPU so the walk
        // finishes; a 10s ceiling still fails fast on a genuinely stuck walk.
        for _ in 0..10_000 {
            self.refresh_vc_status_on_idle();
            if !self.vc_status_settling() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("vc status never settled");
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

    /// Begin a commit (#131, #704): flush unsaved edits so the snapshot captures
    /// them, then open a pre-commit review — the changed-file list and the diff of
    /// what the commit would capture — before the message prompt, so the author
    /// never commits blind. A no-op (with a hint) if the project isn't under version
    /// control, a save fails, or there's nothing to commit.
    fn start_commit(&mut self) {
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
            items.push(git("History", "Browse past commits", GitAction::History));
            items.push(git(
                "Changes",
                "Diff the working tree against the last commit",
                GitAction::Diff,
            ));
            items.push(git(
                "Restore chapter",
                "Revert the current chapter to its last commit",
                GitAction::RestoreChapter,
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
        self.start_vc(
            "⟳ Pushing to the remote…",
            move || match crate::vcs::push(&root) {
                Ok(branch) => VcMsg {
                    text: format!("Pushed {branch} to the remote."),
                    reload: false,
                },
                Err(e) => VcMsg {
                    text: e,
                    reload: false,
                },
            },
        );
    }

    /// Pull the remote's changes on a worker thread (#134). Refuses when the working
    /// tree is dirty (a fast-forward or merge checkout would overwrite uncommitted
    /// edits), so the author commits first. When local and remote have diverged the
    /// worker merges them in-app (#703): a clean merge is committed and reloaded like
    /// any pull, while a conflicting one comes back as the reported message naming the
    /// files to reconcile, leaving the tree untouched.
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
        self.start_vc(
            "⟳ Pulling from the remote…",
            move || match crate::vcs::pull(&root) {
                Ok(text) => VcMsg { text, reload: true },
                Err(e) => VcMsg {
                    text: e,
                    reload: false,
                },
            },
        );
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

    /// Open the navigable commit-history view (#587). A read-only list until a
    /// commit is chosen; from the selection the author diffs the current chapter
    /// against it or restores that chapter to it. Falls back to a message when
    /// there's nothing to browse (no commits yet, or the log can't be read).
    fn open_history(&mut self) {
        match crate::vcs::log(&self.project.root, 200) {
            Ok(commits) if commits.is_empty() => {
                self.message = Some("No commits yet.".into());
            }
            Ok(commits) => {
                self.mode = Mode::History(Box::new(History {
                    commits,
                    selected: 0,
                    filter: String::new(),
                    typing: false,
                }));
            }
            Err(e) => self.message = Some(format!("Could not read history: {e}")),
        }
    }

    /// Handle a key in the commit-history view (#587): arrows move the selection
    /// (skipping filtered-out commits), `d` diffs the current chapter against the
    /// selected commit, `r` restores the chapter to it (into the editable buffer
    /// for review-and-save), Esc closes. `/` starts a type-to-filter query (#822)
    /// that narrows the list to commits whose summary, id, or date match; Esc
    /// clears the filter before it closes the view.
    pub(crate) fn on_key_history(&mut self, key: KeyEvent) {
        let Mode::History(history) = &self.mode else {
            return;
        };
        // In the filter-input sub-mode, keystrokes build the query (#822); the
        // view's single-key commands are reachable only outside it.
        if history.typing {
            self.on_key_history_filter(key);
            return;
        }
        match key.code {
            KeyCode::Char('/') => self.set_history_typing(true),
            KeyCode::Up | KeyCode::Char('k') => {
                let s = self.history_filter_step(false);
                self.set_history_selected(s);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let s = self.history_filter_step(true);
                self.set_history_selected(s);
            }
            KeyCode::Char('d') => self.diff_against_selected_commit(),
            KeyCode::Char('r') => self.restore_from_selected_commit(),
            // Esc clears an active filter first (restoring the full list), then a
            // second Esc closes the view.
            KeyCode::Esc => {
                if self.history_has_filter() {
                    self.clear_history_filter();
                } else {
                    self.mode = self.editing_mode();
                }
            }
            _ => {}
        }
    }

    /// Keys for the history view's filter-input sub-mode (#822): printable
    /// characters build the query (live), Backspace edits it, the arrows move
    /// within the matches, Enter leaves input, and Esc leaves input too — keeping
    /// the filter so `d`/`r` act on the narrowed list (a second Esc, in command
    /// mode, clears it).
    fn on_key_history_filter(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.set_history_typing(false),
            KeyCode::Up => {
                let s = self.history_filter_step(false);
                self.set_history_selected(s);
            }
            KeyCode::Down => {
                let s = self.history_filter_step(true);
                self.set_history_selected(s);
            }
            KeyCode::Backspace => self.history_filter_pop(),
            KeyCode::Char(c) => self.history_filter_push(c),
            _ => {}
        }
    }

    /// The active history filter, lowercased (empty off the view or unfiltered).
    fn history_query(&self) -> String {
        match &self.mode {
            Mode::History(h) => h.filter.to_lowercase(),
            _ => String::new(),
        }
    }

    fn history_has_filter(&self) -> bool {
        matches!(&self.mode, Mode::History(h) if !h.filter.is_empty())
    }

    fn set_history_typing(&mut self, on: bool) {
        if let Mode::History(h) = &mut self.mode {
            h.typing = on;
        }
    }

    fn set_history_selected(&mut self, selected: usize) {
        if let Mode::History(h) = &mut self.mode {
            h.selected = selected;
        }
    }

    fn clear_history_filter(&mut self) {
        if let Mode::History(h) = &mut self.mode {
            h.filter.clear();
            h.typing = false;
        }
    }

    fn history_filter_push(&mut self, c: char) {
        if let Mode::History(h) = &mut self.mode {
            h.filter.push(c);
        }
        self.history_snap();
    }

    fn history_filter_pop(&mut self) {
        if let Mode::History(h) = &mut self.mode {
            h.filter.pop();
        }
        self.history_snap();
    }

    /// After the filter changes, keep the selection on a visible commit: if the
    /// current one no longer matches, jump to the first that does (#822).
    fn history_snap(&mut self) {
        let q = self.history_query();
        let Mode::History(h) = &self.mode else {
            return;
        };
        let stays = h
            .commits
            .get(h.selected)
            .is_some_and(|c| commit_matches(c, &q));
        if !stays {
            if let Some(first) = h.commits.iter().position(|c| commit_matches(c, &q)) {
                self.set_history_selected(first);
            }
        }
    }

    /// The next (or previous, by `forward`) commit that passes the filter, or the
    /// current selection unchanged when there's no further match that way (#822).
    fn history_filter_step(&self, forward: bool) -> usize {
        let q = self.history_query();
        let Mode::History(h) = &self.mode else {
            return 0;
        };
        let from = h.selected;
        let n = h.commits.len();
        let mut i = from;
        loop {
            if forward {
                if i + 1 >= n {
                    return from;
                }
                i += 1;
            } else {
                if i == 0 {
                    return from;
                }
                i -= 1;
            }
            if h.commits.get(i).is_some_and(|c| commit_matches(c, &q)) {
                return i;
            }
        }
    }

    /// The revision id of the commit selected in the history view, or `None` when
    /// that view isn't open — or the selection is hidden by an active filter, so
    /// `d`/`r` never act on a row the query narrowed out (#822).
    fn selected_commit_rev(&self) -> Option<String> {
        let q = self.history_query();
        match &self.mode {
            Mode::History(h) => h
                .commits
                .get(h.selected)
                .filter(|c| commit_matches(c, &q))
                .map(|c| c.rev.clone()),
            _ => None,
        }
    }

    /// Diff the current chapter against the selected commit (#587), showing the
    /// unified patch in the read-only diff overlay (the same colorizer as the
    /// working-tree "Changes" view). The history view stays open behind it.
    fn diff_against_selected_commit(&mut self) {
        let Some(rev) = self.selected_commit_rev() else {
            return;
        };
        self.save_edit(); // diff against the latest edits, not the last-saved state
        let rel = self.active_chapter_rel();
        // `rev` → working tree: what changed in this chapter since that commit.
        match crate::vcs::diff_file_to_worktree(&self.project.root, &rev, &rel) {
            Ok(d) if d.trim().is_empty() => {
                self.message = Some("This chapter is unchanged since that commit.".into());
            }
            Ok(d) => self.overlay = Some(Overlay::report("diff", diff_lines(&d))),
            Err(e) => self.message = Some(format!("Diff failed: {e}")),
        }
    }

    /// Restore the current chapter to the selected commit (#587) into the editable
    /// buffer, matching the review-and-save semantics of the HEAD restore: the
    /// recovered version loads into the editor, marked dirty, so the author reviews
    /// it and saves (or discards) rather than the file changing underneath them.
    fn restore_from_selected_commit(&mut self) {
        let Some(rev) = self.selected_commit_rev() else {
            return;
        };
        self.leave_continuous(); // restore operates on the single active chapter
        let rel = self.active_chapter_rel();
        match crate::vcs::show_file_at(&self.project.root, &rev, &rel) {
            Ok(Some(text)) => {
                let path = self.project.active_path();
                self.buffer = Buffer::from_file_text(&path, &text);
                self.cache_focus_words();
                self.mode = self.editing_mode();
                let short: String = rev.chars().take(7).collect();
                self.message = Some(format!(
                    "Restored from {short} — review and save to keep it."
                ));
            }
            Ok(None) => {
                self.message = Some("That commit has no version of this chapter.".into());
            }
            Err(e) => self.message = Some(format!("Restore failed: {e}")),
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

/// Whether commit `c` passes the history view's active filter (#822): a
/// case-insensitive substring match on its summary, short id, or date. `q` is the
/// already-lowercased query (empty = every commit matches). Mirrors the same test
/// the renderer applies when drawing the filtered list.
fn commit_matches(c: &crate::vcs::CommitMeta, q: &str) -> bool {
    q.is_empty()
        || c.summary.to_lowercase().contains(q)
        || c.id.to_lowercase().contains(q)
        || c.when.to_lowercase().contains(q)
}

/// Color a unified diff for the changes overlay (#135): additions green, removals
/// red, file/hunk headers and context dimmed. Mirrors the bible-review diff.
fn diff_lines(patch: &str) -> Vec<Line<'static>> {
    let theme = theme::current();
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
