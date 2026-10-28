//! Resume the last editing position (#588): a per-project sidecar that records
//! where the author left off — the active chapter and the cursor's byte offset
//! within it — written on a clean save or quit and read at the next open to
//! restore that spot, so reopening a novel lands where you were, not at chapter
//! 0, top of file.
//!
//! This is deliberately separate from crash recovery (`src/recovery.rs`). Where
//! recovery protects unsaved *text* after an unclean exit, this remembers the
//! last *location* after a clean one — a convenience, never a source of truth.
//! A missing, corrupt, or now-stale record (the chapter is gone, or the offset
//! is past the shrunken file) simply falls back to chapter 0, top of file.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic;

const DIR: &str = ".quill";
const FILE: &str = "position.json";

/// The last editing position: the active chapter's file (relative to the project
/// root, so it survives reordering) and the cursor's absolute byte offset within
/// that chapter's prose body. Stored by file identity and byte offset so it stays
/// valid across edits to *other* chapters.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub chapter_file: String,
    pub cursor: usize,
}

fn path(root: &Path) -> PathBuf {
    root.join(DIR).join(FILE)
}

/// Write the position sidecar (best-effort), creating `.quill/` if needed.
///
/// Goes through [`atomic::write_hint`] — temp + rename, so a crash of Quill can't
/// leave a torn record over a previous good one, but *without* the two `fsync`s a
/// durable [`atomic::write`] pays (several ms each on macOS). This record is a
/// convenience rewritten on every save, never a source of truth: if a power cut
/// leaves it missing, stale, or empty, [`read`] treats that as "no record" and the
/// next open simply starts at the top — the author loses a cursor position, never
/// a word. Chapters, the manifest, and annotations stay on the durable path.
pub fn write(root: &Path, pos: &Position) -> std::io::Result<()> {
    let json = serde_json::to_string(pos).map_err(std::io::Error::other)?;
    // Only `.quill/` itself: a background write that lands after the project
    // folder was renamed or removed fails instead of recreating it.
    atomic::create_leaf_dir(&root.join(DIR))?;
    atomic::write_hint(&path(root), json.as_bytes())
}

/// The recorded last position, if a parseable sidecar is present. A missing or
/// corrupt sidecar reads as `None` — the open just starts at the top.
pub fn read(root: &Path) -> Option<Position> {
    let text = fs::read_to_string(path(root)).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        crate::testutil::scratch_dir("lastpos", name)
    }

    fn sample() -> Position {
        Position {
            chapter_file: "three.md".into(),
            cursor: 42,
        }
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
}
