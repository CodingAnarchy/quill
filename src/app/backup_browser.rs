//! Browse and restore the project's local backups (#934): the run-action overlay
//! that lists the timestamped `.zip` archives #888 wrote (newest first, with sizes
//! and dates), and the confirmed, non-destructive restore of a chosen one over the
//! open project. The archive listing lives in [`super::backup`]; the zip/unzip work
//! in [`crate::archive`]; this wires them to the browser mode and the restore flow.
//!
//! Restore is non-destructive to the current working state (#934): before the
//! chosen archive is unpacked, the open project is itself archived into the backups
//! dir (a "safety net" the author can browse back to), and the swap goes through a
//! temp folder so a failed unpack leaves the working project untouched rather than
//! half-overwritten. Only once the restored copy is fully on disk does it move into
//! place, and the project reopens from it.

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};

use crate::archive;
use crate::config;
use crate::mode::{BackupBrowser, Mode, Pending};
use crate::project::Project;

use super::App;

impl App {
    /// Open the backup browser (#934): the project's local archives listed newest
    /// first, each with its size and date, selectable to restore. Reports and stays
    /// in the editor when there are no backups (or none can resolve) — a clean
    /// no-op, so the author isn't dropped into an empty overlay.
    pub(crate) fn open_backup_browser(&mut self) {
        let entries = self.list_local_backups();
        if entries.is_empty() {
            self.message = Some(
                "No local backups yet — enable scheduled local backups in [backup] to build some."
                    .into(),
            );
            return;
        }
        self.mode = Mode::BackupBrowser(Box::new(BackupBrowser {
            entries,
            selected: 0,
        }));
    }

    /// Key handling for the backup browser (#934): move (arrows / j-k), restore the
    /// selected backup (Enter — opens a y/N confirm), close (Esc). Read-only over
    /// the working project until the confirm is answered.
    pub(crate) fn on_key_backup_browser(&mut self, key: KeyEvent) {
        let Mode::BackupBrowser(browser) = &mut self.mode else {
            return;
        };
        if let Some(s) = super::assist::list_move(browser.selected, browser.entries.len(), key.code)
        {
            browser.selected = s;
            return;
        }
        match key.code {
            KeyCode::Enter => self.confirm_restore_selected_backup(),
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }

    /// Open the y/N confirm before restoring the selected backup (#934). The
    /// destructive swap it warns about is spelled out — the archive replaces the
    /// working project (which is itself backed up first) — so the author restores
    /// deliberately, never by a stray Enter.
    fn confirm_restore_selected_backup(&mut self) {
        let Mode::BackupBrowser(browser) = &self.mode else {
            return;
        };
        let Some(entry) = browser.entries.get(browser.selected) else {
            return;
        };
        let name = entry.name.clone();
        self.pending_restore_backup = Some(entry.path.clone());
        self.open_prompt(
            Pending::ConfirmRestoreBackup,
            format!("Restore {name} over the current project? It's backed up first. (y/N)"),
            "",
            None,
        );
    }

    /// Restore the backup the confirm targeted over the open project (#934), from
    /// [`Pending::ConfirmRestoreBackup`]. Non-destructive to the working state: the
    /// current project is archived into the backups dir first, then the swap goes
    /// through a temp folder so a failed unpack can't leave a half-overwritten
    /// project. Reopens the project from the restored copy and reports either way.
    pub(crate) fn restore_pending_backup(&mut self) {
        let Some(src) = self.pending_restore_backup.take() else {
            self.mode = self.editing_mode();
            return;
        };
        // Flush unsaved work so the pre-restore safety archive reflects the screen,
        // then drop the recovery sidecar — the restore supersedes it.
        if self.flush_active().is_err() {
            self.mode = self.editing_mode();
            return; // save failed — message already set
        }
        self.leave_continuous();
        // Settled before the swap, so a queued sidecar write can't land in (or
        // its clear remove from) the restored project.
        self.settle_recovery_cleared();

        let root = self.project.root.clone();
        match restore_over(&root, &src) {
            Ok(n) => {
                // Reopen the (now restored) project fresh from disk, like a switch;
                // replace the "Opened …" message it sets with the restore count.
                self.switch_to(root.clone());
                if self.project.root == root {
                    self.message = Some(format!("Restored {n} files from the backup."));
                }
            }
            Err(e) => {
                self.mode = self.editing_mode();
                self.message = Some(format!("Restore failed: {e}"));
            }
        }
    }

    /// Recover a project stranded by a crash mid restore-swap (#1179), run at
    /// startup *before* the open-vs-create decision. [`restore_over`] swaps the
    /// restored copy into place with two renames — move `root` aside to
    /// `<name>.quill-restore-old`, then move the staged copy from
    /// `<name>.quill-restore-staging` in — and its rollback only runs on an `Err`,
    /// not on a crash or power loss *between* the renames. Interrupted there, `root`
    /// is gone and the real project is left in a hidden sibling; nothing else
    /// notices, because a missing `root` reads as a brand-new directory and startup
    /// would scaffold a fresh empty project over the author's book (which then
    /// "appears gone"). This finishes the interrupted swap by moving the stranded
    /// project back to `root`.
    ///
    /// It acts only when `root` has no manifest *and* a leftover sibling holds one,
    /// so it's a cheap no-op on every normal launch (a clean run leaves no temp
    /// dirs; a successful restore removes both). A manifest that is present but
    /// unreadable is left alone — that's the damaged-project case
    /// (`offer_backup_restore_on_damage`), not an interrupted swap. When both
    /// leftovers exist (the crash landed squarely between the two renames), the
    /// `.quill-restore-old` copy wins: it's the author's original that the first
    /// rename moved away, so undoing the swap is the safe recovery. Only if just
    /// `.quill-restore-staging` remains is the interrupted new content recovered
    /// instead. Either way the leftover temp dirs are cleaned up.
    pub(crate) fn reconcile_interrupted_restore(root: &Path) {
        // A present manifest means the swap completed (or never ran): leave any stale
        // temp dirs for the restore path itself to clear, and don't touch `root`.
        if Project::manifest_path(root).exists() {
            return;
        }
        // No manifest at `root`. Recover from a leftover sibling if one holds a real
        // project, preferring the original that was renamed away over staged content.
        let old = old_dir(root);
        let staging = staging_dir(root);
        let source = if Project::manifest_path(&old).exists() {
            old.clone()
        } else if Project::manifest_path(&staging).exists() {
            staging.clone()
        } else {
            return; // nothing stranded — an ordinary launch into a missing/empty root
        };
        // Clear a stray (manifest-less, hence not-a-project) `root` so the rename can
        // land, then move the stranded project home and drop the leftover temp dirs.
        let _ = fs::remove_dir_all(root);
        if fs::rename(&source, root).is_ok() {
            let _ = fs::remove_dir_all(&old);
            let _ = fs::remove_dir_all(&staging);
        }
    }
}

/// Restore the archive at `src` over the project at `root`, non-destructively
/// (#934). The current project is archived into the backups dir first (best-effort
/// — a lost safety net must never block a restore the author asked for), the
/// archive is unpacked into a fresh sibling temp folder (so a bad zip / zip bomb /
/// missing manifest fails there, leaving `root` untouched), and only a fully
/// unpacked copy is swapped into place. Returns the number of files restored.
fn restore_over(root: &Path, src: &Path) -> std::io::Result<usize> {
    // A pre-restore safety archive of the working state, so the author can browse
    // back to exactly what they had before the restore. Best-effort.
    if let Some(dir) = config::backups_dir(root) {
        let _ = fs::create_dir_all(&dir);
        let safety = dir.join(archive::default_name(root));
        let _ = archive::write_archive(root, &safety);
    }

    // Unpack into a fresh sibling temp folder; `restore_archive` refuses to clobber
    // an existing project, so the temp must not already hold a manifest. A prior
    // interrupted restore's leftover is cleared first.
    let staging = staging_dir(root);
    let _ = fs::remove_dir_all(&staging);
    let written = archive::restore_archive(src, &staging)?;

    // The unpack succeeded — now swap the restored copy into place. Move the old
    // project aside, move the staged copy in, then drop the old copy. Ordering it
    // this way keeps a readable project at `root` for all but the two rename
    // instants, rather than emptying it up front.
    let old = old_dir(root);
    let _ = fs::remove_dir_all(&old);
    fs::rename(root, &old)?;
    if let Err(e) = fs::rename(&staging, root) {
        // The swap-in failed: put the original project back so the author isn't
        // left with an empty root, then surface the error.
        let _ = fs::rename(&old, root);
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    let _ = fs::remove_dir_all(&old);
    Ok(written)
}

/// The temp folder a restore unpacks into before swapping it over `root` (#934) — a
/// hidden sibling so it never reads as a project itself and is easy to clean up.
fn staging_dir(root: &Path) -> PathBuf {
    sibling(root, ".quill-restore-staging")
}

/// The temp folder the working project is moved aside to during the swap (#934),
/// removed once the restored copy is in place.
fn old_dir(root: &Path) -> PathBuf {
    sibling(root, ".quill-restore-old")
}

/// A hidden sibling of `root` named `<root-name><suffix>`, used for the restore
/// swap's temp folders. Falls back to a name under the parent for an oddly named
/// root so the swap still has somewhere to stage.
fn sibling(root: &Path, suffix: &str) -> PathBuf {
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "project".into());
    let parent = root.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{name}{suffix}"))
}

#[cfg(test)]
impl App {
    /// Restore a chosen backup synchronously for a test (#934), standing in for the
    /// browser's Enter → confirm → restore path without driving keys. Sets the
    /// pending target and runs the restore, returning to the editor. Returns whether
    /// the project reopened at its original root (a successful restore).
    pub(crate) fn restore_backup_for_test(&mut self, src: PathBuf) -> bool {
        let root = self.project.root.clone();
        self.pending_restore_backup = Some(src);
        self.restore_pending_backup();
        self.project.root == root
    }
}
