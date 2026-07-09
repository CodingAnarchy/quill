//! Two-way sync (#962): the off-thread sync flow that fetches the remote and
//! reconciles its chapter edits with the local working copy, plus the accessor the
//! conflict-resolution UI (#963) reads. Child module of `vcs_actions`, so these
//! `impl App` methods keep access to `App`'s private fields; the sync worker rides
//! the shared `vc_task` and lands through [`poll_vc`](App::poll_vc) exactly like a
//! pull, so a network-slow sync never blocks the editor and its failure surfaces its
//! cause (#838).

use super::super::*;
use super::VcMsg;

impl App {
    /// Sync with the remote on a worker thread (#962): flush unsaved work, then fetch
    /// and reconcile the remote's chapter edits with local — a true two-way sync for
    /// writing the same project on two machines.
    ///
    /// Guards data loss the same way [`start_pull`](App::start_pull) does: it drops to
    /// single-chapter view (the reload targets one chapter), saves the buffer, and
    /// refuses when the tree still has uncommitted changes, so the merge checkout can
    /// never overwrite unsaved words. The reconciliation itself is a chapter-level
    /// three-way in [`crate::vcs::sync`]: chapters only one side changed merge
    /// cleanly; chapters both sides changed differently come back as conflicts, which
    /// [`poll_vc`](App::poll_vc) stashes for the resolution UI (#963) rather than ever
    /// clobbering a side.
    pub(super) fn start_sync(&mut self) {
        if !self.guard_remote() || self.vc_task.is_busy() {
            return;
        }
        self.leave_continuous(); // the post-sync reload targets the single active chapter
        self.save_edit();
        self.refresh_vc_status();
        if self.vc_changes > 0 {
            self.message = Some("Commit your changes before syncing.".into());
            return;
        }
        // Stamp the origin chapter like a pull (#870): a slow sync that lands after
        // the author navigated (or typed unsaved edits) must not reload over them.
        self.pull_origin = Some(self.project.active_path());
        let root = self.project.root.clone();
        self.start_vc(
            "⟳ Syncing with the remote…",
            move || match crate::vcs::sync(&root) {
                Ok(out) => VcMsg {
                    text: out.summary,
                    reload: out.reload,
                    conflicts: out.conflicts,
                },
                Err(e) => VcMsg::plain(e, false),
            },
        );
    }

    /// The per-chapter conflicts from the last two-way sync (#962), for the
    /// resolution UI (#963) to render and act on. Empty after a clean sync (or before
    /// any sync).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn sync_conflicts(&self) -> &[crate::vcs::ChapterConflict] {
        &self.sync_conflicts
    }

    /// Spawn a stand-in *sync* whose worker reports `conflicts` chapter conflicts
    /// (#962), so a hermetic test can drive the [`poll_vc`](App::poll_vc) landing path
    /// — the conflicts reaching `sync_conflicts`, both versions preserved — without a
    /// real remote. The conflict entries carry only their path (the field the UI keys
    /// on); the version text is exercised end-to-end by the `vcs` tests.
    #[cfg(test)]
    pub(crate) fn spawn_sync_conflicts_for_test(&mut self, paths: &[&str]) {
        let conflicts = paths
            .iter()
            .map(|p| crate::vcs::ChapterConflict {
                path: (*p).to_string(),
                local: format!("local {p}"),
                remote: format!("remote {p}"),
                base: format!("base {p}"),
            })
            .collect::<Vec<_>>();
        let summary = format!("{} chapter(s) changed on both machines.", conflicts.len());
        self.pull_origin = Some(self.project.active_path());
        self.start_vc("⟳ Syncing with the remote…", move || VcMsg {
            text: summary,
            reload: false,
            conflicts,
        });
    }
}
