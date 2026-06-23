//! Crash recovery (#263): a per-project sidecar that holds the active chapter's
//! *unsaved* prose, written on the editor's idle cadence while the buffer is
//! dirty. After an unclean exit (crash, kill, power loss) the next launch finds
//! the sidecar and offers to restore the work; a normal save — or an explicit
//! discard — clears it.
//!
//! Where atomic save (#262) protects what's *saved*, this protects what *isn't*.
//! The sidecar is a best-effort safety copy, not a source of truth, so it is
//! written cheaply (a temp file + rename, no `fsync`): the rename keeps a crash
//! mid-write from truncating a previous good sidecar, while skipping the
//! per-write disk flush that would be wasteful on every idle tick.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DIR: &str = ".quill";
const FILE: &str = "recovery.json";

/// The unsaved work captured for recovery: the active chapter's file (relative
/// to the project root) and its prose body as of the last cadence write. Front
/// matter is not captured — it is re-attached from the chapter file on save.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Recovery {
    pub chapter_file: String,
    pub body: String,
}

fn path(root: &Path) -> PathBuf {
    root.join(DIR).join(FILE)
}

/// Write the sidecar (best-effort), creating `.quill/` if needed. Staged through
/// a sibling temp file and renamed into place so an interrupted write can't
/// leave a torn sidecar over a previous good one.
pub fn write(root: &Path, rec: &Recovery) -> std::io::Result<()> {
    let json = serde_json::to_string(rec).map_err(std::io::Error::other)?;
    fs::create_dir_all(root.join(DIR))?;
    let tmp = path(root).with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path(root))
}

/// The recorded unsaved work, if a parseable sidecar is present. A missing or
/// corrupt sidecar reads as `None` — no recovery is offered rather than failing.
pub fn read(root: &Path) -> Option<Recovery> {
    let text = fs::read_to_string(path(root)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Remove the sidecar. No error if it was already gone.
pub fn clear(root: &Path) {
    let _ = fs::remove_file(path(root));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quill-recovery-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Recovery {
        Recovery { chapter_file: "one.md".into(), body: "unsaved prose\n".into() }
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tmpdir("round_trip");
        write(&dir, &sample()).unwrap();
        assert_eq!(read(&dir), Some(sample()));
    }

    #[test]
    fn read_is_none_without_a_sidecar() {
        let dir = tmpdir("absent");
        assert_eq!(read(&dir), None);
    }

    #[test]
    fn read_is_none_for_a_corrupt_sidecar() {
        let dir = tmpdir("corrupt");
        fs::create_dir_all(dir.join(DIR)).unwrap();
        fs::write(path(&dir), "{ not json").unwrap();
        assert_eq!(read(&dir), None);
    }

    #[test]
    fn clear_removes_the_sidecar_and_is_idempotent() {
        let dir = tmpdir("clear");
        write(&dir, &sample()).unwrap();
        clear(&dir);
        assert_eq!(read(&dir), None);
        clear(&dir); // no panic / error when already gone
    }

    #[test]
    fn write_leaves_no_temp_file_behind() {
        let dir = tmpdir("clean");
        write(&dir, &sample()).unwrap();
        let strays: Vec<_> = fs::read_dir(dir.join(DIR))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "temp file left behind: {strays:?}");
    }
}
