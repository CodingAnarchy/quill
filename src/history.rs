//! The per-day writing history: an append-style JSONL log at
//! `.quill/history.jsonl`, one record per day, recording the day's totals and
//! deltas. It lets the progress report show "today's word delta" and survive a
//! reopen (the day's running counters are seeded from today's record).
//!
//! One *logical* record per day: writing today's snapshot replaces any existing
//! record for the same date, so the file stays one line per day.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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

/// Read every recorded snapshot, skipping any malformed line.
pub fn load(root: &Path) -> Vec<Snapshot> {
    let text = fs::read_to_string(log_path(root)).unwrap_or_default();
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The snapshot recorded for `date`, if any.
pub fn for_date(root: &Path, date: &str) -> Option<Snapshot> {
    load(root).into_iter().rfind(|s| s.date == date)
}

/// Write `snap`, replacing any existing record for its date so the log keeps
/// one line per day. Creates `.quill/` if needed.
pub fn persist(root: &Path, snap: &Snapshot) -> std::io::Result<()> {
    let mut snaps = load(root);
    snaps.retain(|s| s.date != snap.date);
    snaps.push(snap.clone());

    fs::create_dir_all(root.join(DIR))?;
    let mut out = String::new();
    for s in &snaps {
        let line = serde_json::to_string(s).map_err(std::io::Error::other)?;
        out.push_str(&line);
        out.push('\n');
    }
    fs::write(log_path(root), out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("quill-hist-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
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
