//! Remote push & pull (#134): the remote URL prompt, the off-thread push / pull
//! worker, and its [`poll_vc`](App::poll_vc) landing path (#870, #838) that
//! reloads the buffer after a pull without clobbering unsaved edits. Child module
//! of `vcs_actions`, so these `impl App` methods keep access to `App`'s private
//! fields; the `#[cfg(test)]` helpers drive the worker outcomes hermetically.

use super::super::*;
use super::VcMsg;

impl App {
    /// Prompt for the remote URL the project backs up to (#134), prefilled with
    /// the current one so "change remote" shows what's set.
    pub(super) fn start_set_remote(&mut self) {
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
    pub(super) fn start_push(&mut self) {
        if !self.guard_remote() || self.vc_task.is_busy() {
            return;
        }
        self.save_edit(); // flush edits so the status the author sees is accurate
        let root = self.project.root.clone();
        self.start_vc(
            "⟳ Pushing to the remote…",
            move || match crate::vcs::push(&root) {
                Ok(branch) => VcMsg::plain(format!("Pushed {branch} to the remote."), false),
                Err(e) => VcMsg::plain(e, false),
            },
        );
    }

    /// Pull the remote's changes on a worker thread (#134). Refuses when the working
    /// tree is dirty (a fast-forward or merge checkout would overwrite uncommitted
    /// edits), so the author commits first. When local and remote have diverged the
    /// worker merges them in-app (#703): a clean merge is committed and reloaded like
    /// any pull, while a conflicting one comes back as the reported message naming the
    /// files to reconcile, leaving the tree untouched.
    pub(super) fn start_pull(&mut self) {
        if !self.guard_remote() || self.vc_task.is_busy() {
            return;
        }
        // Read-only instance (#871): a pull's fast-forward/merge force-checks-out the
        // tree via libgit2, bypassing the save-path write guards (bug H). Refuse rather
        // than race the instance that holds the project and clobber its chapter files.
        if self.project.is_read_only() {
            self.message = Some(
                "Read-only: this project is open in another Quill instance; not pulled.".into(),
            );
            return;
        }
        self.leave_continuous(); // the post-pull reload targets the single active chapter
        self.save_edit();
        self.refresh_vc_status();
        if self.vc_changes > 0 {
            self.message = Some("Commit your changes before pulling.".into());
            return;
        }
        // Stamp the chapter this pull started on (#870). A network-slow pull can
        // land after the author has navigated away (and typed unsaved edits);
        // [`poll_vc`] reloads only when the buffer is still on this origin and
        // clean, so a late pull never overwrites edits made in the meantime.
        self.pull_origin = Some(self.project.active_path());
        let root = self.project.root.clone();
        self.start_vc(
            "⟳ Pulling from the remote…",
            move || match crate::vcs::pull(&root) {
                Ok(text) => VcMsg::plain(text, true),
                Err(e) => VcMsg::plain(e, false),
            },
        );
    }

    /// Run a remote op on a worker thread (#134): spawn `work`, stash the receiver
    /// for [`App::poll_vc`], and show `status` until the result lands. The single
    /// place push/pull cross to a background thread.
    pub(super) fn start_vc(&mut self, status: &str, work: impl FnOnce() -> VcMsg + Send + 'static) {
        self.vc_task.spawn(work);
        self.message = Some(status.into());
    }

    /// Spawn a VC worker that panics with `cause` (#838), standing in for a push or
    /// pull whose worker thread dies. Lets a hermetic test drive the `poll_vc`
    /// death path — the one branch a real remote op can't reach on demand — and
    /// assert the cause reaches the UI message rather than a generic string.
    #[cfg(test)]
    pub(crate) fn spawn_failing_vc_for_test(&mut self, cause: &'static str) {
        self.start_vc("⟳ Working…", move || panic!("{cause}"));
    }

    /// Spawn a stand-in *pull* whose worker succeeds and asks for a reload (#870),
    /// stamping the current active chapter as its origin — exactly as
    /// [`start_pull`] does before crossing to the network. Lets a hermetic test
    /// drive the "a pull lands after the author navigated / typed" path (no real
    /// remote required) and assert [`poll_vc`] never clobbers unsaved edits.
    #[cfg(test)]
    pub(crate) fn spawn_landing_pull_for_test(&mut self) {
        self.pull_origin = Some(self.project.active_path());
        self.start_vc("⟳ Pulling from the remote…", || {
            VcMsg::plain("Pulled from the remote.".into(), true)
        });
    }

    /// Drive [`poll_vc`] to completion for a test (#838): tick until the worker's
    /// outcome lands, so an assertion sees the settled message rather than racing
    /// the thread. Silences the panic hook around the wait so a deliberately dying
    /// worker doesn't print to the test's stderr. Bounded so a stuck worker fails
    /// fast instead of hanging.
    #[cfg(test)]
    pub(crate) fn settle_vc_for_test(&mut self) {
        use std::panic;
        let prior = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        for _ in 0..10_000 {
            self.poll_vc();
            if !self.vc_task.is_busy() {
                panic::set_hook(prior);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic::set_hook(prior);
        panic!("vc task never settled");
    }

    /// Shared guard for the sync actions: under version control with a remote set.
    /// Sets the explanatory message and returns `false` when not.
    pub(super) fn guard_remote(&mut self) -> bool {
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
                // A pull can rewrite files on disk; reload the buffer unless doing so
                // would clobber unsaved edits (#870). The skip note rides alongside
                // the pull's own message so the author learns their open chapter was
                // left untouched rather than silently not refreshed.
                let note = if msg.reload {
                    self.reload_after_pull()
                } else {
                    None
                };
                self.pull_origin = None; // this pull is done
                                         // Stash any per-chapter sync conflicts for the resolution UI (#963);
                                         // a push/pull or a clean sync carries none, which clears a prior set.
                self.sync_conflicts = msg.conflicts;
                self.refresh_vc_status();
                self.message = Some(match note {
                    Some(note) => format!("{} {note}", msg.text),
                    None => msg.text,
                });
                // A sync that detected chapters changed on both machines opens the
                // resolution flow (#963), so the author resolves them in-app rather
                // than being left to the git CLI. But never clobber an active
                // prompt/overlay/modal (bug F): if the author isn't in a plain editing
                // mode, the flow is deferred — `surface_sync_conflicts` stashes it and
                // a later tick opens it once they're back in the editor, mirroring how
                // a pull defers its buffer reload.
                // A clean push/pull/sync carries none, clearing any prior deferral so
                // a stale pending flag can't fire.
                self.pending_sync_conflicts = !self.sync_conflicts.is_empty();
                self.surface_sync_conflicts();
            }
            Poll::Pending => {
                // A sync that landed earlier while the author was in a
                // prompt/overlay (bug F) surfaces here once they're back in a plain
                // editing mode — polled every tick, so it opens the moment they return.
                self.surface_sync_conflicts();
            }
            // The worker panicked; surface its cause (#838) rather than a generic
            // failure, so an auth/network/rejected push or pull is diagnosable.
            Poll::Died(cause) => {
                self.pull_origin = None;
                self.message = Some(cause.map_or_else(
                    || "Remote operation failed.".into(),
                    |cause| format!("Remote operation failed: {cause}"),
                ));
            }
        }
    }

    /// Open the sync conflict-resolution flow (#963) for a background sync that
    /// landed with conflicts — but only when the author is in a plain editing mode,
    /// so it never clobbers an active prompt/overlay/modal (bug F). A sync can land at
    /// any tick; if the author was mid-rename, in a `:` command, naming a milestone, or
    /// in the drafts overlay, the surfacing is deferred (`pending_sync_conflicts`) and
    /// re-attempted each tick from [`poll_vc`], opening the moment they return to the
    /// editor. A no-op when nothing is pending, or when a resolution flow is already up.
    fn surface_sync_conflicts(&mut self) {
        if !self.pending_sync_conflicts {
            return;
        }
        // Already resolving (this or a draft merge): leave the open flow alone.
        if matches!(self.mode, Mode::ConflictReview(_)) {
            return;
        }
        // Defer while the author is in a prompt/overlay/modal — surfacing now would
        // discard their in-flight state. The same guard the pass-queue resume uses.
        if !matches!(self.mode, Mode::Insert | Mode::Normal | Mode::Visual { .. })
            || self.overlay.is_some()
        {
            return;
        }
        self.pending_sync_conflicts = false;
        self.open_conflict_resolution();
    }

    /// Reload the buffer from disk after a pull landed (#870) — but never over
    /// unsaved edits. The pull dropped to single-chapter view at spawn and stamped
    /// its origin chapter (`pull_origin`); if the author navigated away during a
    /// network-slow pull, or typed edits the buffer hasn't saved, the reload is
    /// skipped so the just-pulled disk state can't clobber work with no undo, and a
    /// short note (returned to ride alongside the pull's message) says so. In that
    /// case the pulled files are already on disk and the next visit to a changed
    /// chapter reads them; only the buffer the author is actively editing is
    /// protected. On the clean, common path (still on the origin, no unsaved edits)
    /// this reloads exactly as before and returns no note.
    fn reload_after_pull(&mut self) -> Option<String> {
        let on_origin = self
            .pull_origin
            .as_deref()
            .is_some_and(|origin| origin == self.project.active_path());
        if !on_origin {
            return Some("You'd navigated away, so your open chapter was left untouched.".into());
        }
        if self.edit_dirty() {
            return Some("Kept your unsaved edits — save or commit, then pull again.".into());
        }
        let _ = self.reload_active_chapter();
        None
    }
}
