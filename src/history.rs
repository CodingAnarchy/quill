//! The per-day writing history: an append-only JSONL log at
//! `.quill/history.jsonl`, recording each day's totals and deltas. It lets the
//! progress report show "today's word delta" and survive a reopen (the day's
//! running counters are seeded from today's record).
//!
//! One *logical* record per day: [`persist`] genuinely appends, so re-saving the
//! same day writes another line rather than rewriting the file (#439) — cheap on
//! the save/idle cadence. [`load`] coalesces those lines, keeping the latest per
//! date, and [`compact`] rewrites the file one-line-per-day at startup so the
//! within-session duplicates never accumulate across sessions.

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic;

const DIR: &str = ".quill";
const FILE: &str = "history.jsonl";

/// One day's writing snapshot. `date` is an ISO `YYYY-MM-DD` string.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub date: String,
    /// Total manuscript words at snapshot time.
    pub words: usize,
    /// Words inserted today.
    pub added: usize,
    /// Words deleted today.
    pub removed: usize,
    /// TODO markers in the manuscript at snapshot time.
    pub todo: usize,
    /// Words relocated out of the manuscript today (deleted chapters).
    pub moved_to_unused: usize,
    /// Per-part word totals at snapshot time, in manifest part order. Defaulted
    /// so records written before parts existed still parse.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<usize>,
    /// Per-part words inserted today, in manifest part order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub part_added: Vec<usize>,
    /// Per-part words deleted today, in manifest part order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub part_removed: Vec<usize>,
}

fn log_path(root: &Path) -> PathBuf {
    root.join(DIR).join(FILE)
}

/// Read the recorded snapshots as one logical record per day, newest line for a
/// date winning (since [`persist`] appends and may write several lines for a day
/// within a session). Malformed lines are skipped; records keep first-seen
/// (chronological) order.
pub fn load(root: &Path) -> Vec<Snapshot> {
    coalesce(&fs::read_to_string(log_path(root)).unwrap_or_default())
}

/// Parse the log text into one logical record per day — the latest line for a
/// date supersedes earlier ones, malformed lines are skipped, and first-seen
/// (chronological) order is kept. Shared by [`load`] and [`compact`] so the file
/// is read once per call.
fn coalesce(text: &str) -> Vec<Snapshot> {
    let mut out: Vec<Snapshot> = Vec::new();
    for snap in text
        .lines()
        .filter_map(|line| serde_json::from_str::<Snapshot>(line).ok())
    {
        match out.iter_mut().find(|s| s.date == snap.date) {
            Some(existing) => *existing = snap,
            None => out.push(snap),
        }
    }
    out
}

/// The snapshot recorded for `date`, if any.
pub fn for_date(root: &Path, date: &str) -> Option<Snapshot> {
    load(root).into_iter().find(|s| s.date == date)
}

/// Append `snap` as a new line — O(1), with no read or full rewrite, so it stays
/// cheap on the save/idle cadence (#439). Several lines for the same day may
/// accumulate within a session; [`load`] coalesces them (latest wins) and
/// [`compact`] tidies the file. Creates `.quill/` if needed.
pub fn persist(root: &Path, snap: &Snapshot) -> std::io::Result<()> {
    fs::create_dir_all(root.join(DIR))?;
    let mut line = serde_json::to_string(snap).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(root))?;
    file.write_all(line.as_bytes())
}

/// Rewrite the log as one line per day (the latest record per date), dropping the
/// within-day duplicate lines [`persist`] appends. Called at startup so the file
/// stays compact across sessions without any per-save rewrite (#439). A no-op
/// when the log is empty or already one line per day.
pub fn compact(root: &Path) -> std::io::Result<()> {
    let text = fs::read_to_string(log_path(root)).unwrap_or_default();
    let raw_lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    let snaps = coalesce(&text);
    if snaps.is_empty() || snaps.len() == raw_lines {
        return Ok(()); // empty, or already one line per day (incl. no malformed lines)
    }
    let mut out = String::new();
    for s in &snaps {
        out.push_str(&serde_json::to_string(s).map_err(std::io::Error::other)?);
        out.push('\n');
    }
    atomic::write(&log_path(root), out.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        crate::testutil::scratch_dir("hist", name)
    }

    fn snap(date: &str, added: usize) -> Snapshot {
        Snapshot {
            date: date.into(),
            words: 1000,
            added,
            removed: 10,
            todo: 3,
            moved_to_unused: 0,
            ..Snapshot::default()
        }
    }

    #[test]
    fn persist_then_load_round_trips() {
        let dir = temp_dir("round-trip");
        persist(&dir, &snap("2026-06-16", 200)).unwrap();
        let loaded = load(&dir);
        assert_eq!(loaded, vec![snap("2026-06-16", 200)]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn one_record_per_day_replaces_in_place() {
        let dir = temp_dir("coalesce");
        persist(&dir, &snap("2026-06-15", 100)).unwrap();
        persist(&dir, &snap("2026-06-16", 100)).unwrap();
        persist(&dir, &snap("2026-06-16", 350)).unwrap(); // re-save today
        let loaded = load(&dir);
        assert_eq!(loaded.len(), 2); // one per day, not three
        assert_eq!(for_date(&dir, "2026-06-16").unwrap().added, 350); // latest wins
        assert_eq!(for_date(&dir, "2026-06-15").unwrap().added, 100); // older untouched
        fs::remove_dir_all(&dir).ok();
    }

    /// Raw physical line count of the log (before `load` coalesces) — to assert
    /// `persist` appends and `compact` tidies.
    fn raw_line_count(dir: &Path) -> usize {
        fs::read_to_string(log_path(dir))
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count()
    }

    #[test]
    fn persist_appends_rather_than_rewriting() {
        let dir = temp_dir("append");
        persist(&dir, &snap("2026-06-16", 100)).unwrap();
        persist(&dir, &snap("2026-06-16", 250)).unwrap(); // re-save the same day
                                                          // Two physical lines (it appended)...
        assert_eq!(raw_line_count(&dir), 2);
        // ...but one logical record per day, latest winning.
        assert_eq!(load(&dir).len(), 1);
        assert_eq!(for_date(&dir, "2026-06-16").unwrap().added, 250);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compact_coalesces_duplicate_day_lines() {
        let dir = temp_dir("compact");
        persist(&dir, &snap("2026-06-15", 100)).unwrap();
        persist(&dir, &snap("2026-06-16", 100)).unwrap();
        persist(&dir, &snap("2026-06-16", 350)).unwrap();
        assert_eq!(raw_line_count(&dir), 3, "appended three lines");

        compact(&dir).unwrap();
        assert_eq!(raw_line_count(&dir), 2, "coalesced to one line per day");
        // Values preserved: latest per day, older day untouched.
        assert_eq!(for_date(&dir, "2026-06-16").unwrap().added, 350);
        assert_eq!(for_date(&dir, "2026-06-15").unwrap().added, 100);

        // Idempotent: compacting an already-tidy log changes nothing.
        compact(&dir).unwrap();
        assert_eq!(raw_line_count(&dir), 2);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compact_on_a_missing_log_is_a_noop() {
        let dir = temp_dir("compact-missing");
        compact(&dir).unwrap();
        assert!(!log_path(&dir).exists(), "no empty file created");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn per_part_fields_round_trip() {
        let dir = temp_dir("per-part");
        let snap = Snapshot {
            date: "2026-06-16".into(),
            words: 1000,
            added: 200,
            removed: 10,
            todo: 3,
            moved_to_unused: 0,
            parts: vec![600, 400],
            part_added: vec![150, 50],
            part_removed: vec![4, 6],
        };
        persist(&dir, &snap).unwrap();
        assert_eq!(load(&dir), vec![snap]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_without_part_fields_still_parses() {
        // A line written before parts existed (no part_* keys) must load with
        // empty per-part vecs, not fail.
        let dir = temp_dir("old-line");
        fs::create_dir_all(dir.join(DIR)).unwrap();
        fs::write(
            log_path(&dir),
            "{\"date\":\"2026-06-16\",\"words\":1000,\"added\":5,\"removed\":0,\"todo\":2,\"moved_to_unused\":0}\n",
        )
        .unwrap();
        let snap = for_date(&dir, "2026-06-16").unwrap();
        assert_eq!(snap.added, 5);
        assert!(snap.part_added.is_empty());
        assert!(snap.parts.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_log_loads_empty() {
        let dir = temp_dir("missing");
        assert!(load(&dir).is_empty());
        assert_eq!(for_date(&dir, "2026-06-16"), None);
        fs::remove_dir_all(&dir).ok();
    }
}
