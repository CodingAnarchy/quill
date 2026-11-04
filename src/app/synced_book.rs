//! A book written on more than one device (#1298), through iCloud Drive or any
//! other way its files change on disk under the editor. On the idle pass the
//! editor notices the open chapter changing on disk:
//!
//! - when the buffer has no unsaved edits, it shows the new version at once,
//!   with a note, and snapshots it into this device's history;
//! - when it has, the author's words stay in the buffer and the other version
//!   is kept beside the chapter as a copy (`ledger 2.md`, the way iCloud names
//!   its own conflict copies), so no words are lost either way.
//!
//! In a synced folder it also looks for the copies iCloud leaves when the same
//! chapter changed on two devices, and names each once.

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use super::App;
use crate::{frontmatter, synced};

/// How often a synced book is scanned for conflict copies.
const CONFLICT_SCAN: Duration = Duration::from_secs(30);

/// A file's modification time and size: enough to notice it changing.
pub type DiskStamp = Option<(SystemTime, u64)>;

/// `path`'s [`DiskStamp`], or `None` when it can't be read.
fn disk_stamp(path: &Path) -> DiskStamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

impl App {
    /// Notice the open chapter changing on disk, and conflict copies a sync
    /// left. Called on the idle pass, never per keystroke: a stat of one file,
    /// and a read only when it changed.
    pub(super) fn notice_changes_on_disk(&mut self) {
        self.notice_chapter_change();
        if synced::is_synced(&self.project.root)
            && self.last_conflict_scan.elapsed() >= CONFLICT_SCAN
        {
            self.last_conflict_scan = Instant::now();
            self.notice_conflict_copies();
        }
    }

    /// The open chapter's file changed since the editor last read or wrote it.
    fn notice_chapter_change(&mut self) {
        // The continuous view, a save in flight, or a reference pane in focus:
        // the buffer isn't simply this one file, or the change may be our own.
        if self.manuscript.is_some()
            || !self.pending_saves.is_empty()
            || !self.focus_shows_active_chapter()
        {
            return;
        }
        let path = self.project.active_path();
        let stamp = disk_stamp(&path);
        match &self.active_disk_stamp {
            Some((known, seen)) if *known == path && *seen == stamp => return,
            Some((known, _)) if *known == path => {}
            // A chapter just opened: remember it as it is.
            _ => {
                self.active_disk_stamp = Some((path, stamp));
                return;
            }
        }
        self.active_disk_stamp = Some((path.clone(), stamp));
        let Ok(disk) = std::fs::read_to_string(&path) else {
            return; // gone or unreadable: the next save writes it back
        };
        let theirs = frontmatter::split(&disk).1;
        if theirs == self.buffer.text() {
            return; // our own save, or the same words
        }
        let title = self
            .project
            .chapter_title(self.project.active_index())
            .unwrap_or("This chapter")
            .to_string();
        if self.edit_dirty() {
            match synced::keep_copy(&path, &disk) {
                Ok(copy) => {
                    self.message = Some(format!(
                        "\u{201c}{title}\u{201d} also changed on another device. Your words are \
                         kept here; the other version is in {}.",
                        file_name(&copy)
                    ));
                }
                Err(e) => {
                    self.message = Some(format!(
                        "\u{201c}{title}\u{201d} also changed on another device, and Quill \
                         couldn't keep a copy of that version: {e}. Your words are kept here."
                    ));
                }
            }
            return;
        }
        match self.reload_active_chapter() {
            Ok(()) => {
                self.message = Some(format!(
                    "\u{201c}{title}\u{201d} changed on another device. This is the new version."
                ));
                // Record the change in this device's history now, not after the
                // usual pause: it's a version from somewhere else.
                if let Some(after) = self.history.snapshot_after() {
                    self.snapshot_owed_since = Instant::now().checked_sub(after);
                } else {
                    self.mark_snapshot_owed();
                }
            }
            Err(e) => {
                self.message = Some(format!(
                    "\u{201c}{title}\u{201d} changed on another device, and Quill couldn't \
                     read it: {e}"
                ));
            }
        }
    }

    /// Name each conflict copy a sync left beside the active book's chapters,
    /// once per copy.
    fn notice_conflict_copies(&mut self) {
        let files: Vec<String> = self
            .project
            .manifest
            .chapters
            .iter()
            .map(|c| c.file.clone())
            .collect();
        let copies = synced::conflict_copies(&self.project.root, &files);
        let new: Vec<(String, String)> = copies
            .into_iter()
            .filter(|(_, copy)| self.conflicts_noted.insert(copy.clone()))
            .collect();
        let Some((file, copy)) = new.first() else {
            return;
        };
        let title = self
            .project
            .manifest
            .chapters
            .iter()
            .find(|c| &c.file == file)
            .map_or(file.as_str(), |c| c.title.as_str())
            .to_string();
        let more = match new.len() {
            1 => String::new(),
            n => format!(" ({} more chapters too)", n - 1),
        };
        self.message = Some(format!(
            "\u{201c}{title}\u{201d} was changed on two devices before they synced. Both \
             versions are kept: the other is in {copy}{more}."
        ));
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}
