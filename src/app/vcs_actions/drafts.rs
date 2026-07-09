//! Branch-as-draft (#965): manage alternate versions of the manuscript — an alt
//! ending, a POV split, an experiment — as first-class *drafts* (git branches),
//! without dropping to the git CLI. Child module of `vcs_actions`, so these
//! `impl App` methods keep access to `App`'s private fields.
//!
//! The overlay ([`Mode::Drafts`](crate::mode::Mode::Drafts)) lists every draft with
//! the current one marked; from it the author creates a draft from the current state
//! (`n`), switches drafts (Enter — the working copy follows and the active chapter
//! reloads), and merges the selected draft into the current one (`m`). Switching and
//! merging guard unsaved work the same way a pull does (#639): they flush the buffer
//! and refuse a still-dirty tree, so a force checkout can never overwrite the
//! author's words.
//!
//! A merge reuses the chapter-level three-way the two-way sync (#962) runs
//! ([`crate::vcs::merge_branch`]): a clean merge lands a two-parent commit; a merge
//! with conflicts changes nothing and routes the conflicting chapters through the
//! same conflict-resolution UI (#963) a sync uses — keep-target / take-source /
//! hand-merge, per chapter — and [`finalize_draft_merge`](App::finalize_draft_merge)
//! lands them once resolved.

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::bible_review::unified_diff;
use crate::mode::{ChapterConflictItem, ConflictReview, DraftsMenu, Mode, Pending};
use crate::vcs::MergeOutcome;

use super::super::*;

impl App {
    /// Open the drafts overlay (#965): the project's drafts (local branches) listed
    /// with the current one marked and selected. Requires a repository with at least
    /// one commit (a branch only exists once something is committed) — reports and
    /// stays in the editor otherwise, so the author is never dropped into an empty
    /// overlay. Flushes unsaved work first so the list reflects the screen.
    pub(crate) fn open_drafts(&mut self) {
        if !self.under_vc {
            self.message = Some("Not under version control — Ctrl-v to initialize.".into());
            return;
        }
        // Refuse while a background VCS op (push/pull/sync) is in flight (bug F): a
        // draft switch/merge from the overlay would race the in-flight op and could
        // land a conflict-resolution flow that a landing sync then clobbers.
        if self.vc_task.is_busy() {
            self.message = Some("A sync is still running — try drafts again in a moment.".into());
            return;
        }
        self.save_edit();
        let Ok(current) = crate::vcs::current_branch(&self.project.root) else {
            self.message =
                Some("Commit something first — drafts branch off the current state.".into());
            return;
        };
        let drafts = match crate::vcs::list_branches(&self.project.root) {
            Ok(drafts) if !drafts.is_empty() => drafts,
            Ok(_) => {
                // A committed repo always has at least the current branch; an empty
                // list means the ref read hiccuped — fall back to the current draft.
                vec![current.clone()]
            }
            Err(e) => {
                self.message = Some(format!("Drafts: {e}"));
                return;
            }
        };
        let selected = drafts.iter().position(|d| *d == current).unwrap_or(0);
        self.mode = Mode::Drafts(Box::new(DraftsMenu {
            drafts,
            current,
            selected,
        }));
    }

    /// Key handling for the drafts overlay (#965): move (arrows / j-k), switch to the
    /// selected draft (Enter), create a new draft from the current state (`n`), merge
    /// the selected draft into the current one (`m`), close (Esc).
    pub(crate) fn on_key_drafts(&mut self, key: KeyEvent) {
        let Mode::Drafts(menu) = &mut self.mode else {
            return;
        };
        if let Some(s) = super::super::assist::list_move(menu.selected, menu.drafts.len(), key.code)
        {
            menu.selected = s;
            return;
        }
        match key.code {
            KeyCode::Enter => self.switch_selected_draft(),
            KeyCode::Char('n') => {
                // Prompt for the new draft's name; create_draft picks it up on Enter.
                self.open_prompt(Pending::CreateDraft, "New draft name: ", "", None);
            }
            KeyCode::Char('m') => self.merge_selected_draft(),
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }

    /// Create a new draft named `name` from the current state and switch to it (#965).
    /// A blank name cancels; an existing name (or another git error) is reported and
    /// the overlay reopens so the author can try again. Because a new draft branches
    /// off the current `HEAD` — which the working copy already matches — switching to
    /// it is a clean no-op checkout, so no reload is needed.
    pub(crate) fn create_draft(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.open_drafts(); // cancelled — back to the list, nothing created
            return;
        }
        match crate::vcs::create_branch(&self.project.root, name) {
            Ok(()) => match crate::vcs::checkout_branch(&self.project.root, name) {
                Ok(()) => {
                    self.message = Some(format!("Created draft '{name}' and switched to it."));
                    self.open_drafts();
                }
                Err(e) => {
                    // The draft exists but the switch failed — report and reopen so the
                    // author can switch by hand; nothing is lost.
                    self.message =
                        Some(format!("Created draft '{name}', but couldn't switch: {e}"));
                    self.open_drafts();
                }
            },
            Err(e) => {
                self.message = Some(format!("Draft: {e}"));
                self.open_drafts();
            }
        }
    }

    /// Switch to the draft selected in the overlay (#965): bring the working copy up
    /// to that draft and reload the active chapter, so the editor now shows it. Drops
    /// to single-chapter view first (the reload targets one chapter) and refuses a
    /// dirty tree (the checkout would clobber uncommitted edits), so unsaved work is
    /// never silently lost — the guard mirrors a pull.
    fn switch_selected_draft(&mut self) {
        let Some(name) = self.selected_draft() else {
            return;
        };
        // Read-only instance (#871): the checkout force-overwrites the whole tree via
        // libgit2, bypassing the write guards on save (bug H). Refuse rather than race
        // the instance that holds the project and clobber its chapter files.
        if self.project.is_read_only() {
            self.message = Some(
                "Read-only: this project is open in another Quill instance; drafts not switched."
                    .into(),
            );
            return;
        }
        if self.vc_task.is_busy() {
            self.message = Some("A sync is still running — try again in a moment.".into());
            return;
        }
        self.leave_continuous(); // the reload targets the single active chapter
        self.save_edit();
        self.refresh_vc_status();
        if self.vc_changes > 0 {
            self.message = Some("Commit your changes before switching drafts.".into());
            return;
        }
        match crate::vcs::checkout_branch(&self.project.root, &name) {
            Ok(()) => {
                let note = match self.reload_active_chapter() {
                    Ok(()) => String::new(),
                    Err(e) => format!(" (reload failed: {e})"),
                };
                self.refresh_vc_status();
                self.mode = self.editing_mode();
                self.message = Some(format!("Switched to draft '{name}'.{note}"));
            }
            Err(e) => {
                // Refused (dirty tree / already here) or failed — report and stay in
                // the overlay so the author can act.
                self.message = Some(format!("Switch draft: {e}"));
                self.open_drafts();
            }
        }
    }

    /// Merge the draft selected in the overlay into the current draft (#965). A clean
    /// merge lands a two-parent commit and reloads the active chapter; a merge with
    /// conflicts changes nothing and routes the conflicting chapters through the
    /// conflict-resolution UI (#963). Guards a dirty tree up front (the merge checkout
    /// would clobber uncommitted edits), like switching.
    fn merge_selected_draft(&mut self) {
        let Some(from) = self.selected_draft() else {
            return;
        };
        // Read-only instance (#871): the merge checkout force-overwrites the tree via
        // libgit2, bypassing the write guards (bug H) — refuse rather than clobber the
        // owning instance's files.
        if self.project.is_read_only() {
            self.message = Some(
                "Read-only: this project is open in another Quill instance; drafts not merged."
                    .into(),
            );
            return;
        }
        // Refuse while a background VCS op is in flight (bug F): a landing sync's
        // conflict-resolution flow would race — and could misroute — this merge's.
        if self.vc_task.is_busy() {
            self.message =
                Some("A sync is still running — try the merge again in a moment.".into());
            return;
        }
        let into = match crate::vcs::current_branch(&self.project.root) {
            Ok(name) => name,
            Err(e) => {
                self.message = Some(format!("Merge draft: {e}"));
                return;
            }
        };
        if from == into {
            self.message = Some("That's the current draft — pick another to merge in.".into());
            self.open_drafts();
            return;
        }
        self.leave_continuous(); // a clean merge reloads the single active chapter
        self.save_edit();
        self.refresh_vc_status();
        if self.vc_changes > 0 {
            self.message = Some("Commit your changes before merging drafts.".into());
            return;
        }
        match crate::vcs::merge_branch(&self.project.root, &into, &from) {
            Ok(MergeOutcome::Clean { merged }) => {
                let note = match self.reload_active_chapter() {
                    Ok(()) => String::new(),
                    Err(e) => format!(" (reload failed: {e})"),
                };
                self.refresh_vc_status();
                self.mode = self.editing_mode();
                self.message = Some(if merged.is_empty() {
                    format!("Merged draft '{from}' into '{into}' — already up to date.{note}")
                } else {
                    format!(
                        "Merged draft '{from}' into '{into}' — {} chapter(s) folded in.{note}",
                        merged.len()
                    )
                });
            }
            Ok(MergeOutcome::Conflicts { conflicts }) => {
                // Both drafts changed the same chapters differently — route them
                // through the same resolution UI a sync uses (#963). Nothing was
                // committed; both drafts survive intact.
                self.open_draft_merge_resolution(into, from, &conflicts);
            }
            Err(e) => {
                self.message = Some(format!("Merge draft: {e}"));
                self.open_drafts();
            }
        }
    }

    /// The name of the draft selected in the overlay, or `None` when not in the
    /// overlay (or the selection is out of range).
    fn selected_draft(&self) -> Option<String> {
        let Mode::Drafts(menu) = &self.mode else {
            return None;
        };
        menu.drafts.get(menu.selected).cloned()
    }

    /// Open the conflict-resolution UI for a draft merge that hit conflicts (#965) —
    /// the branch-merge counterpart to the sync path's `open_conflict_resolution`
    /// (#963). Each conflicting chapter carries both full versions (target ↔ source)
    /// and a pre-rendered diff for the overlay; the resolution flow (keep-target /
    /// take-source / hand-merge, per chapter) is exactly the sync one, and
    /// [`finalize_draft_merge`](App::finalize_draft_merge) lands the result once the
    /// queue drains. The pending `(into, from)` routes the shared finalize step to a
    /// branch merge rather than a sync.
    fn open_draft_merge_resolution(
        &mut self,
        into: String,
        from: String,
        conflicts: &[crate::vcs::ChapterConflict],
    ) {
        let items = conflicts
            .iter()
            .map(|c| ChapterConflictItem {
                path: c.path.clone(),
                diff: unified_diff(&c.local, &c.remote),
                local: c.local.clone(),
                remote: c.remote.clone(),
            })
            .collect::<Vec<_>>();
        let count = items.len();
        self.pending_draft_merge = Some((into, from));
        self.mode = Mode::ConflictReview(Box::new(ConflictReview {
            conflicts: items,
            resolved: BTreeMap::new(),
            editing: None,
        }));
        self.message = Some(format!(
            "{count} chapter(s) changed on both drafts — keep (l), take (r), or merge (m) each."
        ));
    }

    /// Land a draft merge the author resolved by hand (#965), from the shared
    /// conflict-resolution finalize step. Writes the chosen text for each conflicting
    /// chapter and records the two-parent merge commit via
    /// [`crate::vcs::finalize_branch_merge`], reloads the active chapter, and returns
    /// to the editor. A failure leaves both drafts intact and surfaces the cause, so
    /// re-running the merge re-detects the same conflicts — no draft is ever lost.
    pub(super) fn finalize_draft_merge(
        &mut self,
        into: &str,
        from: &str,
        resolved: &BTreeMap<String, String>,
    ) {
        match crate::vcs::finalize_branch_merge(&self.project.root, into, from, resolved) {
            Ok(n) => {
                let note = match self.reload_active_chapter() {
                    Ok(()) => String::new(),
                    Err(e) => format!(" (reload failed: {e})"),
                };
                self.refresh_vc_status();
                self.mode = self.editing_mode();
                self.message = Some(format!(
                    "Merged draft '{from}' into '{into}' — resolved {n} chapter(s).{note}"
                ));
            }
            Err(e) => {
                self.mode = self.editing_mode();
                self.message = Some(format!("Couldn't complete the draft merge: {e}"));
            }
        }
    }
}
