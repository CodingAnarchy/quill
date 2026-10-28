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
//!
//! Beside the coalesced per-day log, a separate append-only *session* log at
//! `.quill/sessions.jsonl` records each individual writing session's output
//! (#835) — one line per recorded session, never coalesced. It feeds the
//! calendar heatmap's per-day breakdown (a day written in bursts keeps each
//! burst) while the per-day rollup keeps driving the sparkline and streaks.
//! Appending a session is the same O(1), read-free write as [`persist`], so the
//! idle-tally guard stays flat.

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic;

const DIR: &str = ".quill";
const FILE: &str = "history.jsonl";
const SESSIONS_FILE: &str = "sessions.jsonl";

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

/// One writing *session*'s recorded output (#835): the words added and removed
/// during a single session on `date` (ISO `YYYY-MM-DD`). Unlike [`Snapshot`],
/// which coalesces to one net figure per day, session records accumulate — a day
/// the author wrote in several bursts keeps a line per burst, so the calendar
/// heatmap can break a day down into its sessions.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionRecord {
    pub date: String,
    /// Words inserted during the session.
    pub added: usize,
    /// Words deleted during the session.
    pub removed: usize,
}

impl SessionRecord {
    /// The session's net words: added − removed.
    pub const fn net(&self) -> i64 {
        self.added as i64 - self.removed as i64
    }
}

fn log_path(root: &Path) -> PathBuf {
    root.join(DIR).join(FILE)
}

fn sessions_path(root: &Path) -> PathBuf {
    root.join(DIR).join(SESSIONS_FILE)
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

/// Append one session record as a new line — O(1), no read or rewrite, so it
/// stays cheap on the save/idle cadence like [`persist`] (#835). Session records
/// are never coalesced: every recorded session keeps its own line, in write
/// order. A session with no output (net zero and nothing added or removed) is not
/// worth recording and is skipped. Creates `.quill/` if needed.
pub fn persist_session(root: &Path, session: &SessionRecord) -> std::io::Result<()> {
    if session.added == 0 && session.removed == 0 {
        return Ok(()); // an empty session isn't worth a line
    }
    fs::create_dir_all(root.join(DIR))?;
    let mut line = serde_json::to_string(session).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(sessions_path(root))?;
    file.write_all(line.as_bytes())
}

/// Read the recorded session records in write order (oldest first). Malformed
/// lines are skipped; a missing log loads empty.
pub fn load_sessions(root: &Path) -> Vec<SessionRecord> {
    fs::read_to_string(sessions_path(root))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<SessionRecord>(line).ok())
        .collect()
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

    fn session(date: &str, added: usize, removed: usize) -> SessionRecord {
        SessionRecord {
            date: date.into(),
            added,
            removed,
        }
    }

    #[test]
    fn sessions_accumulate_rather_than_coalescing() {
        // Unlike the per-day Snapshot log (latest wins), each recorded session
        // keeps its own line — a day written in bursts keeps every burst.
        let dir = temp_dir("sessions-accumulate");
        persist_session(&dir, &session("2026-06-16", 200, 10)).unwrap();
        persist_session(&dir, &session("2026-06-16", 150, 0)).unwrap(); // a second burst
        persist_session(&dir, &session("2026-06-17", 90, 5)).unwrap();

        let all = load_sessions(&dir);
        assert_eq!(all.len(), 3, "every session kept, none coalesced");
        assert_eq!(all[0], session("2026-06-16", 200, 10)); // write order preserved
        assert_eq!(all[2], session("2026-06-17", 90, 5));

        let today: Vec<SessionRecord> = all
            .iter()
            .filter(|s| s.date == "2026-06-16")
            .cloned()
            .collect();
        assert_eq!(today.len(), 2, "both of the day's bursts");
        // The day rolls up to the sum of its sessions.
        let added: usize = today.iter().map(|s| s.added).sum();
        let removed: usize = today.iter().map(|s| s.removed).sum();
        assert_eq!((added, removed), (350, 10));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_session_is_not_recorded() {
        // A session that produced nothing isn't worth a line.
        let dir = temp_dir("sessions-empty");
        persist_session(&dir, &session("2026-06-16", 0, 0)).unwrap();
        assert!(load_sessions(&dir).is_empty());
        assert!(!sessions_path(&dir).exists(), "no file created for a no-op");
        // A pure-deletion session still counts (it's output).
        persist_session(&dir, &session("2026-06-16", 0, 40)).unwrap();
        assert_eq!(load_sessions(&dir).len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_session_log_loads_empty() {
        let dir = temp_dir("sessions-missing");
        assert!(load_sessions(&dir).is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn session_net_is_added_minus_removed() {
        assert_eq!(session("2026-06-16", 200, 50).net(), 150);
        assert_eq!(session("2026-06-16", 10, 40).net(), -30);
    }
}
