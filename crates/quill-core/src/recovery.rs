//! Crash recovery (#263): a per-project sidecar that holds the active chapter's
//! *unsaved* prose, written on the editor's idle cadence while the buffer is
//! dirty. After an unclean exit (crash, kill, power loss) the next launch finds
//! the sidecar and offers to restore the work; a normal save — or an explicit
//! discard — clears it.
//!
//! Where atomic save (#262) protects what's *saved*, this protects what *isn't*.
//! The sidecar is a best-effort safety copy, not a source of truth, but it is
//! still written through the audited atomic path (#262/#567): a crash mid-write
//! must never leave a torn or zero-length sidecar over a previous good one,
//! which would defeat recovery exactly when it is needed.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic;

pub mod writer;

const DIR: &str = ".quill";
const FILE: &str = "recovery.json";

/// One chapter's unsaved prose captured for recovery: its file (relative to the
/// project root) and the prose body as of the last cadence write. Front matter
/// is not captured — it is re-attached from the chapter file on save.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Chapter {
    pub chapter_file: String,
    pub body: String,
}

/// The unsaved work captured for recovery. The single-chapter view captures one
/// chapter; the continuous view captures every dirty segment in its window, each
/// attributed to its own chapter file (#566). At least one chapter is present
/// whenever a sidecar exists.
///
/// The on-disk JSON stays backward-compatible with the original single-chapter
/// sidecar: `chapter_file`/`body` carry the first (or only) chapter, and any
/// further dirty chapters go in `extra`, which is omitted entirely when there
/// are none — so a single-chapter sidecar is byte-for-byte what it was (#263).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Recovery {
    pub chapter_file: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<Chapter>,
}

impl Recovery {
    /// A single-chapter recovery record (the single-chapter view's snapshot).
    pub const fn single(chapter_file: String, body: String) -> Self {
        Self {
            chapter_file,
            body,
            extra: Vec::new(),
        }
    }

    /// Build a record from one or more dirty chapters (the continuous view's
    /// snapshot). Returns `None` for an empty list — nothing to recover. The
    /// first chapter fills the legacy fields; the rest go in `extra`.
    pub fn from_chapters(mut chapters: Vec<Chapter>) -> Option<Self> {
        if chapters.is_empty() {
            return None;
        }
        let first = chapters.remove(0);
        Some(Self {
            chapter_file: first.chapter_file,
            body: first.body,
            extra: chapters,
        })
    }

    /// Every captured chapter in order — the legacy first chapter followed by any
    /// extras — so single- and multi-chapter records restore uniformly.
    pub fn chapters(&self) -> impl Iterator<Item = Chapter> + '_ {
        std::iter::once(Chapter {
            chapter_file: self.chapter_file.clone(),
            body: self.body.clone(),
        })
        .chain(self.extra.iter().cloned())
    }
}

fn path(root: &Path) -> PathBuf {
    root.join(DIR).join(FILE)
}

/// Write the sidecar (best-effort), creating `.quill/` if needed. Goes through
/// [`atomic::write`] — a PID-keyed sibling temp file, `fsync`ed and renamed into
/// place — so an interrupted write can't leave a torn or empty sidecar over a
/// previous good one, and concurrent processes never collide on the temp name.
///
/// The editor calls this from its background [`writer`], so the write may land
/// a moment after the capture: only `.quill/` itself is created, never the
/// project root, so a late write for a project folder that has since been
/// renamed or removed fails instead of resurrecting the folder.
pub fn write(root: &Path, rec: &Recovery) -> io::Result<()> {
    let json = serde_json::to_string(rec).map_err(io::Error::other)?;
    match fs::create_dir(root.join(DIR)) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    atomic::write(&path(root), json.as_bytes())
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
        crate::testutil::scratch_dir("recovery", name)
    }

    fn sample() -> Recovery {
        Recovery::single("one.md".into(), "unsaved prose\n".into())
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tmpdir("round_trip");
        write(&dir, &sample()).unwrap();
        assert_eq!(read(&dir), Some(sample()));
    }

    #[test]
    fn a_multi_chapter_record_round_trips_and_lists_every_chapter() {
        let dir = tmpdir("multi_round_trip");
        let rec = Recovery::from_chapters(vec![
            Chapter {
                chapter_file: "one.md".into(),
                body: "edit A\n".into(),
            },
            Chapter {
                chapter_file: "two.md".into(),
                body: "edit B\n".into(),
            },
        ])
        .unwrap();
        write(&dir, &rec).unwrap();
        let back = read(&dir).expect("a multi-chapter sidecar round-trips");
        assert_eq!(back, rec);
        let files: Vec<_> = back.chapters().map(|c| c.chapter_file).collect();
        assert_eq!(files, vec!["one.md", "two.md"]);
    }

    #[test]
    fn from_chapters_is_none_for_an_empty_list() {
        assert!(Recovery::from_chapters(Vec::new()).is_none());
    }

    #[test]
    fn a_single_chapter_sidecar_serializes_without_the_extra_key() {
        // Backward compatibility: a one-chapter record must produce exactly the
        // legacy JSON shape, so an old reader still parses it (#263/#566).
        let json = serde_json::to_string(&sample()).unwrap();
        assert!(
            !json.contains("extra"),
            "no empty `extra` array leaks: {json}"
        );
    }

    #[test]
    fn a_legacy_single_chapter_sidecar_still_parses() {
        let dir = tmpdir("legacy");
        fs::create_dir_all(dir.join(DIR)).unwrap();
        // The exact bytes the pre-#566 sidecar wrote (no `extra` field).
        fs::write(
            path(&dir),
            r#"{"chapter_file":"one.md","body":"old work\n"}"#,
        )
        .unwrap();
        assert_eq!(
            read(&dir),
            Some(Recovery::single("one.md".into(), "old work\n".into()))
        );
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
        // The sidecar itself must be the only file in `.quill/` — no fixed-name
        // (`recovery.json.tmp`) nor PID-keyed (`.recovery.json.<pid>.tmp`) temp
        // staged by the atomic path may linger after a clean write.
        let strays: Vec<_> = fs::read_dir(dir.join(DIR))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "temp file left behind: {strays:?}");
        // And the file that does land is a readable, parseable sidecar.
        assert_eq!(read(&dir), Some(sample()));
    }

    // A failed write must not clobber a previously good sidecar: routing through
    // the atomic path means an interruption after staging but before the rename
    // leaves the prior sidecar byte-for-byte intact (#567). We drive the same
    // fault-injection seam `atomic` uses for its own crash test (#265).
    #[test]
    fn failed_write_preserves_previous_sidecar() {
        let dir = tmpdir("preserve");
        write(&dir, &sample()).unwrap();

        let doomed = Recovery::single("two.md".into(), "half-written\n".into());
        atomic::fail_point::arm_pre_rename_failure();
        assert!(write(&dir, &doomed).is_err());

        // The first, good sidecar must still be the one on disk.
        assert_eq!(read(&dir), Some(sample()));
    }
}
