//! Books kept in a synced folder, iCloud Drive in particular (#1298), written
//! on more than one device. Two rules keep a sync from doing damage:
//!
//! - **Per-device state stays on its device.** The editor's state folder (the
//!   project lock, crash recovery, writing stats, the resume position) is
//!   `.quill.nosync/` in a synced folder instead of `.quill/`. iCloud never
//!   syncs an item whose name ends `.nosync`, so a lock taken on the Mac never
//!   reaches the phone and locks the author out there.
//! - **Nothing git-related goes in the shared folder.** iCloud delivers files
//!   one by one and out of order, which corrupts a repository. Each device keeps
//!   the book's history in its own storage (`<data dir>/histories/<id>.git`,
//!   keyed by the `id` in `book.toml`) with the shared folder as its working
//!   tree. See [`crate::vcs`].
//!
//! When the same chapter changes on both devices before they sync, iCloud
//! keeps both: the other device's version arrives as a sibling copy named like
//! `ledger 2.md`. [`conflict_copies`] finds them, so the author can be told which
//! chapter needs a look, and no words are lost.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::config;

/// The editor's state folder in an ordinary book.
pub const STATE_DIR: &str = ".quill";

/// The editor's state folder in a synced book: iCloud skips `.nosync` items.
pub const SYNCED_STATE_DIR: &str = ".quill.nosync";

/// Whether the book at `root` is in a synced folder: under iCloud Drive's
/// `Mobile Documents` (on a Mac, `~/Library/Mobile Documents`; on the phone, the
/// app's iCloud container), or marked synced by a test.
pub fn is_synced(root: &Path) -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if test_seam::is_marked(root) {
        return true;
    }
    let path = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    path.components()
        .any(|c| matches!(c, Component::Normal(name) if name == "Mobile Documents"))
}

/// The editor's state folder for the book at `root`: [`STATE_DIR`], or
/// [`SYNCED_STATE_DIR`] in a synced folder.
pub fn state_dir(root: impl AsRef<Path>) -> PathBuf {
    let root = root.as_ref();
    root.join(if is_synced(root) {
        SYNCED_STATE_DIR
    } else {
        STATE_DIR
    })
}

/// The book's `id` from its `book.toml`, if it has one.
pub fn project_id(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join(crate::project::MANIFEST_NAME)).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table.get("id")?.as_str().map(str::to_string)
}

/// The book's `id`, writing a new one into `book.toml` when it has none. The
/// line goes first in the file, where a top-level key is always valid TOML,
/// and nothing else in the file changes.
pub fn ensure_project_id(root: &Path) -> io::Result<String> {
    if let Some(id) = project_id(root) {
        return Ok(id);
    }
    let path = root.join(crate::project::MANIFEST_NAME);
    let text = fs::read_to_string(&path)?;
    let id = new_id();
    crate::atomic::write(&path, format!("id = \"{id}\"\n{text}").as_bytes())?;
    Ok(id)
}

/// Where this device keeps the history of the synced book at `root`, or
/// `None` when the book isn't synced, has no id yet, or there's no data
/// directory.
pub fn history_dir(root: &Path) -> Option<PathBuf> {
    if !is_synced(root) {
        return None;
    }
    let id = project_id(root)?;
    Some(
        config::data_dir()?
            .join("histories")
            .join(format!("{id}.git")),
    )
}

/// The copies a sync left beside the active book's chapters when the same
/// chapter changed on two devices: for chapter file `ledger.md`, a sibling
/// `ledger 2.md` (or `3`, and so on). Returns `(chapter file, copy)` pairs,
/// both relative to `root`.
pub fn conflict_copies(root: &Path, chapter_files: &[String]) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for file in chapter_files {
        let path = Path::new(file);
        let (Some(stem), Some(ext)) = (path.file_stem(), path.extension()) else {
            continue;
        };
        let dir = path.parent().unwrap_or_else(|| Path::new(""));
        let Ok(entries) = fs::read_dir(root.join(dir)) else {
            continue;
        };
        let prefix = format!("{} ", stem.to_string_lossy());
        let suffix = format!(".{}", ext.to_string_lossy());
        let mut copies: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|name| {
                name.strip_prefix(&prefix)
                    .and_then(|rest| rest.strip_suffix(&suffix))
                    .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            })
            .map(|name| dir.join(name).to_string_lossy().into_owned())
            .collect();
        copies.sort();
        found.extend(copies.into_iter().map(|copy| (file.clone(), copy)));
    }
    found
}

/// Keep `text`, another device's version of the chapter at `path`, beside it as
/// `<stem> 2.<ext>` (or the next free number, the way iCloud names its own
/// conflict copies, so [`conflict_copies`] finds it too), and return the copy's
/// path. Used when the chapter changed elsewhere while this device had unsaved
/// words in it: the author's words stay, and the other version is kept.
pub fn keep_copy(path: &Path, text: &str) -> io::Result<PathBuf> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let copy = (2..1000)
        .map(|n| dir.join(format!("{stem} {n}.{ext}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| dir.join(format!("{stem} copy.{ext}")));
    crate::atomic::write(&copy, text.as_bytes())?;
    Ok(copy)
}

/// A random version-4 UUID, from the OS's randomness by way of the standard
/// library's hasher keys and the time.
fn new_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let half = |salt: u64| {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.finish()
    };
    let (a, b) = (half(1), half(2));
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xfff,
        ((b >> 48) & 0x3fff) | 0x8000,
        b & 0xffff_ffff_ffff
    )
}

/// Tests mark a scratch folder as synced, rather than writing under a real
/// `Mobile Documents`.
#[cfg(any(test, feature = "test-support"))]
mod test_seam {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    thread_local! {
        static SYNCED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn is_marked(root: &Path) -> bool {
        SYNCED.with(|s| s.borrow().iter().any(|p| root.starts_with(p)))
    }

    pub(super) fn mark(root: &Path) {
        SYNCED.with(|s| s.borrow_mut().push(root.to_path_buf()));
    }
}

/// Treat `root` (and everything under it) as a synced folder on this thread.
#[cfg(any(test, feature = "test-support"))]
pub fn mark_synced_for_test(root: &Path) {
    test_seam::mark(root);
}

#[cfg(test)]
mod tests;
