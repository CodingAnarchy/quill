//! Two-way sync (#962): the off-thread sync flow that fetches the remote and
//! reconciles its chapter edits with the local working copy, plus the accessor the
//! conflict-resolution UI (#963) reads. Child module of `vcs_actions`, so these
//! `impl App` methods keep access to `App`'s private fields; the sync worker rides
//! the shared `vc_task` and lands through [`poll_vc`](App::poll_vc) exactly like a
//! pull, so a network-slow sync never blocks the editor and its failure surfaces its
//! cause (#838).

use std::collections::BTreeMap;

use crate::app::bible_review::unified_diff;
use crate::mode::{ChapterConflictItem, ConflictReview};

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

    /// Open the conflict-resolution overlay for the chapters the last sync found
    /// changed on both machines (#963). Called by [`poll_vc`](App::poll_vc) as soon
    /// as a sync lands with conflicts, so the author resolves them in-app instead of
    /// dropping to the git CLI.
    ///
    /// Before anything is shown, a safety milestone is tagged on the current commit
    /// (both sides' work is already committed — the sync detected them from history),
    /// so neither version can be lost even if the author abandons the flow. Each
    /// conflict carries both full versions and a pre-rendered local↔remote diff for
    /// the overlay. No-op when there are no conflicts to resolve.
    pub(crate) fn open_conflict_resolution(&mut self) {
        if self.sync_conflicts.is_empty() {
            return;
        }
        // Safety milestone (#963): tag the current HEAD so both authors' committed
        // work is recoverable regardless of how the resolution goes. A duplicate
        // name (a prior aborted attempt) is fine to ignore — the earlier tag already
        // marks the same safe point.
        let name = format!("pre-sync-{}", crate::date::file_stamp());
        let _ = crate::vcs::create_tag(
            &self.project.root,
            &name,
            "Safety snapshot before resolving sync conflicts",
        );

        let conflicts = self
            .sync_conflicts
            .iter()
            .map(|c| ChapterConflictItem {
                path: c.path.clone(),
                diff: unified_diff(&c.local, &c.remote),
                local: c.local.clone(),
                remote: c.remote.clone(),
            })
            .collect::<Vec<_>>();
        self.mode = Mode::ConflictReview(Box::new(ConflictReview {
            conflicts,
            resolved: BTreeMap::new(),
            editing: None,
        }));
    }

    /// Record `text` as the resolution for the front conflicting chapter and move to
    /// the next (#963). When the queue drains, the accumulated resolutions are landed
    /// as one merge commit by [`finalize_conflict_resolution`](App::finalize_conflict_resolution).
    fn resolve_front_conflict(&mut self, text: String) {
        if let Mode::ConflictReview(r) = &mut self.mode {
            if r.conflicts.is_empty() {
                return;
            }
            let front = r.conflicts.remove(0);
            r.resolved.insert(front.path, text);
            r.editing = None;
            if !r.conflicts.is_empty() {
                return; // more to resolve
            }
        } else {
            return;
        }
        self.finalize_conflict_resolution();
    }

    /// Land every resolved chapter as the completed sync (#963): write the chosen
    /// text to each conflicting chapter and record the two-parent merge commit via
    /// [`crate::vcs::finalize_sync`], so both histories stay reachable. Reloads the
    /// active chapter afterwards (its file may have changed) and returns to the
    /// editor. A failure leaves the safety milestone and both versions intact and
    /// surfaces the cause rather than a half-applied merge.
    fn finalize_conflict_resolution(&mut self) {
        let resolved = match &self.mode {
            Mode::ConflictReview(r) => r.resolved.clone(),
            _ => return,
        };
        match crate::vcs::finalize_sync(&self.project.root, &resolved) {
            Ok(summary) => {
                self.sync_conflicts.clear();
                let _ = self.reload_active_chapter();
                self.refresh_vc_status();
                self.mode = self.editing_mode();
                self.message = Some(summary);
            }
            Err(e) => {
                // A rare failure (a tree gone dirty, a network drop): return to the
                // editor with the cause. Nothing was committed, and the safety
                // milestone plus both committed sides survive, so re-running Sync
                // re-detects the same conflicts to resolve — no side is ever lost.
                self.mode = self.editing_mode();
                self.message = Some(format!("Couldn't complete the sync: {e}"));
            }
        }
    }

    /// Handle a key while resolving sync conflicts (#963). Outside the merge editor:
    /// `l` keeps local, `r` takes remote, `m` opens a scratch buffer seeded with both
    /// versions to hand-merge, and Esc abandons the flow (the safety milestone and
    /// both committed versions survive). While merging: the usual text keys apply,
    /// Ctrl-S accepts the merged text, and Esc returns to the choice.
    pub(crate) fn on_key_conflict_review(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let editing = matches!(&self.mode, Mode::ConflictReview(r) if r.editing.is_some());

        if editing {
            match (ctrl, key.code) {
                (true, KeyCode::Char('s')) => {
                    let text = match &self.mode {
                        Mode::ConflictReview(r) => r.editing.as_ref().map(Buffer::text),
                        _ => None,
                    };
                    if let Some(text) = text {
                        self.resolve_front_conflict(text);
                    }
                }
                (_, KeyCode::Esc) => {
                    if let Mode::ConflictReview(r) = &mut self.mode {
                        r.editing = None; // back to the choice for this conflict
                    }
                }
                _ => {
                    self.handle_text_edit_key(key);
                }
            }
            return;
        }

        match key.code {
            KeyCode::Char('l') => {
                if let Mode::ConflictReview(r) = &self.mode {
                    if let Some(front) = r.conflicts.first() {
                        let text = front.local.clone();
                        self.resolve_front_conflict(text);
                    }
                }
            }
            KeyCode::Char('r') => {
                if let Mode::ConflictReview(r) = &self.mode {
                    if let Some(front) = r.conflicts.first() {
                        let text = front.remote.clone();
                        self.resolve_front_conflict(text);
                    }
                }
            }
            KeyCode::Char('m') => {
                if let Mode::ConflictReview(r) = &mut self.mode {
                    if let Some(front) = r.conflicts.first() {
                        // Seed the merge buffer with both versions, clearly marked, so
                        // the author blends them and deletes what they don't want —
                        // neither side is lost going in.
                        let seed = format!(
                            "<<<<<<< local\n{}\n=======\n{}\n>>>>>>> remote\n",
                            front.local.trim_end(),
                            front.remote.trim_end(),
                        );
                        r.editing = Some(Buffer::from_text(&seed));
                    }
                }
            }
            KeyCode::Esc => {
                self.mode = self.editing_mode();
                self.message =
                    Some("Conflict resolution cancelled — both versions are safe.".into());
            }
            _ => {}
        }
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
