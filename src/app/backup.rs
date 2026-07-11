//! Automatic off-site backup to the configured Git remote (#887): an opt-in,
//! per-project cadence that transparently pushes to `origin` so the remote is an
//! always-current backup, rather than a manual `git push` the author has to
//! remember.
//!
//! Two independent triggers drive it, both configured under `[backup]`
//! ([`BackupConfig`](crate::config::BackupConfig)): a push after a save reaches
//! disk (`on_save`), and a timed interval (`interval_mins`) that catches a long
//! session with few saves. Both are gated behind `enabled` (off by default) and,
//! at run time, behind the project being under version control with a remote set
//! — with nothing to push *to*, the hook is a no-op.
//!
//! The push runs off the UI thread on its own [`Task`](super::task::Task) (never
//! the shared push/pull `vc_task`, so an automatic backup never collides with an
//! author's manual push or steals its result), and [`poll_backup`](App::poll_backup)
//! lands the outcome each loop tick. A failure — offline, auth, a rejected
//! non-fast-forward push — surfaces its cause (reusing the VCS error surfacing and
//! the #855 dead-worker cause), is never silently lost, and is retried on the next
//! cadence: the "last attempt" clock advances on every attempt (success or not),
//! so a failed push simply waits out one more interval and tries again. Only a
//! *successful* push stamps `last_backup`, so the author can see when their prose
//! was last safely off-machine.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::task::Poll;
use super::App;
use crate::archive;
use crate::config;

/// The outcome of an automatic backup push, sent from the worker thread back to
/// the UI thread (#887): the pushed branch on success, or the author-facing cause
/// on failure (already run through the VCS error surfacing).
pub type BackupOutcome = Result<String, String>;

/// The outcome of a scheduled *local* archive backup (#888), sent from the worker
/// thread back to the UI thread: the archive path written on success, or the
/// author-facing cause on failure. Mirrors [`BackupOutcome`] for the remote push.
pub type LocalBackupOutcome = Result<PathBuf, String>;

/// The literal `.zip` suffix a local archive carries — matched case-sensitively
/// on our own output ([`archive::default_name`]), so an author-named `.ZIP` in the
/// backups dir is left alone.
const ZIP_SUFFIX: &str = ".zip";

/// One local archive backup, as listed by the backup browser (#934): its file
/// name, full path, byte size, and last-modified time. A returned list is ordered
/// newest-first, so the most recent restore target leads. Built from the backups
/// directory #888 writes into, never the project itself.
#[derive(Clone, Debug)]
pub struct BackupEntry {
    /// The archive's file name (the timestamped [`archive::default_name`] form).
    pub name: String,
    /// The archive's full path — the restore source.
    pub path: PathBuf,
    /// The archive's size in bytes, for the human-readable "how big" column.
    pub size: u64,
    /// The archive's last-modified time — the honest "when it was written", the
    /// same signal [`latest_archive_mtime`] judges staleness by.
    pub modified: SystemTime,
}

impl App {
    /// Note that a save just reached disk, so an on-save backup is due (#887).
    /// Called from the shared post-save tail. Cheap and a no-op when backup is
    /// off or on-save is disabled: it only sets a flag the cadence hook reads, so
    /// the save path never touches the network. Whether a backup actually fires
    /// (a remote must exist, no push already in flight) is decided later, in
    /// [`tick_backup`](Self::tick_backup), off the save's critical path.
    pub(crate) const fn mark_backup_on_save(&mut self) {
        if self.backup.backs_up_on_save() {
            self.backup_pending = true;
        }
    }

    /// Drive the automatic-backup cadence (#887), called each loop tick like
    /// [`tick_autosave`](Self::tick_autosave). Spawns an off-thread push when one
    /// is due and none is already running; a no-op when backup is disabled, there's
    /// no remote, or a push is in flight. Never blocks editing.
    pub(crate) fn tick_backup(&mut self) {
        // Don't start an auto-push while any repo op is in flight (#871): the manual
        // push/pull/sync run on `vc_task`, and a pull/sync force-checks-out the working
        // tree, so a concurrent auto-backup push would race it into a spurious-failure /
        // inconsistent-ref window. Gate on both the backup task (one push at a time) and
        // the shared VC task (no push while a manual op holds the repo).
        if !self.backup.enabled || self.repo_busy() {
            return;
        }
        // Read-only instance (#871): a secondary Quill holding the project read-only
        // must not push on the owning instance's behalf — mirror the refusal every
        // manual VC op (push/pull/sync/draft) carries rather than auto-pushing.
        if self.project.is_read_only() {
            return;
        }
        if !self.backup_due(Instant::now()) {
            return;
        }
        // A backup needs somewhere to go: skip silently when the project isn't
        // under version control or has no remote (no behavior change #887). Kept
        // here, off the save path, so the check doesn't cost a save a repo open.
        if !self.under_vc || crate::vcs::remote_url(&self.project.root).is_none() {
            // Clear the on-save flag so it doesn't spuriously fire the instant a
            // remote is later configured; the interval clock still governs timed
            // backups once a remote exists.
            self.backup_pending = false;
            return;
        }
        self.spawn_backup();
    }

    /// Whether any repository operation is currently in flight (#871): the shared
    /// manual push/pull/sync task or the automatic-backup push. The auto-backup
    /// cadence gates on this so it never starts a push while a manual VC op (whose
    /// pull/sync force-checks-out the tree) holds the repo, nor doubles up its own push.
    const fn repo_busy(&self) -> bool {
        self.vc_task.is_busy() || self.backup_task.is_busy()
    }

    /// Whether an automatic backup is due as of `now` (#887): the on-save flag is
    /// set, or the configured interval has elapsed since the last attempt. Pure
    /// (time is injected), so the cadence is unit-testable without a wall clock.
    /// Assumes backup is enabled — [`tick_backup`](Self::tick_backup) gates that.
    fn backup_due(&self, now: Instant) -> bool {
        if self.backup_pending {
            return true;
        }
        self.backup
            .interval()
            .is_some_and(|interval| now.duration_since(self.last_backup_attempt) >= interval)
    }

    /// Spawn the off-thread backup push (#887): clear the pending flag, advance the
    /// attempt clock (so a failure waits out a full interval before retrying, and a
    /// success times the next interval from now), and run [`crate::vcs::push`] on
    /// the dedicated backup task. The result lands via [`poll_backup`](Self::poll_backup).
    fn spawn_backup(&mut self) {
        self.backup_pending = false;
        self.last_backup_attempt = Instant::now();
        let root = self.project.root.clone();
        self.backup_task.spawn(move || crate::vcs::push(&root));
    }

    /// Land a finished automatic backup (#887), called each loop tick. On success
    /// stamps `last_backup` so the author can see when their work was last backed
    /// up, and notes it quietly. A failure surfaces its cause — never silently
    /// lost — and leaves `last_backup` untouched; the cadence will retry. A dead
    /// worker (a panic in the push) reports its cause via #855's cause-carrying,
    /// falling back to a generic message only when none is available.
    pub fn poll_backup(&mut self) {
        match self.backup_task.poll() {
            Poll::Ready(Ok(branch)) => {
                self.last_backup = Some(Instant::now());
                self.message = Some(format!("Backed up {branch} to the remote."));
            }
            Poll::Ready(Err(cause)) => {
                self.message = Some(format!("Automatic backup failed: {cause} — will retry."));
            }
            Poll::Pending => {}
            Poll::Died(cause) => {
                self.message = Some(cause.map_or_else(
                    || "Automatic backup failed — will retry.".into(),
                    |cause| format!("Automatic backup failed: {cause} — will retry."),
                ));
            }
        }
    }

    /// How long ago the last successful automatic backup was, or `None` if none
    /// has succeeded this session (#887). Exposed for the status bar and tests.
    pub(crate) fn last_backup_age(&self) -> Option<Duration> {
        self.last_backup.map(|at| at.elapsed())
    }

    // --- Scheduled local archive backups (#888) ---------------------------

    /// Drive the scheduled local-backup cadence (#888), called each loop tick
    /// alongside [`tick_backup`](Self::tick_backup). Spawns an off-thread archive
    /// when one is due and none is already running; a no-op when local backups are
    /// disabled, no data dir resolves, or an archive is in flight. Never blocks
    /// editing — the zip runs on a worker thread. Additive to the remote-push
    /// cadence: the two are independent triggers on their own tasks.
    pub(crate) fn tick_local_backup(&mut self) {
        if self.backup.local_interval().is_none() || self.local_backup_task.is_busy() {
            return;
        }
        if !self.local_backup_due(Instant::now()) {
            return;
        }
        self.spawn_local_backup();
    }

    /// Whether a scheduled local backup is due as of `now` (#888): the configured
    /// hourly interval has elapsed since the last attempt. Pure (time is injected),
    /// so the cadence is unit-testable without a wall clock. Assumes local backups
    /// are on — [`tick_local_backup`](Self::tick_local_backup) gates that.
    fn local_backup_due(&self, now: Instant) -> bool {
        self.backup
            .local_interval()
            .is_some_and(|interval| now.duration_since(self.last_local_backup_attempt) >= interval)
    }

    /// Spawn the off-thread local archive (#888): advance the attempt clock (so a
    /// failure waits out a full interval before retrying, and a success times the
    /// next interval from now), then run the archive-and-prune off the UI thread.
    /// The result lands via [`poll_local_backup`](Self::poll_local_backup). A no-op
    /// when no backups directory resolves.
    fn spawn_local_backup(&mut self) {
        self.last_local_backup_attempt = Instant::now();
        let Some(dir) = config::backups_dir(&self.project.root) else {
            return;
        };
        let root = self.project.root.clone();
        let retention = self.backup.local_retention();
        self.local_backup_task
            .spawn(move || run_local_backup(&root, &dir, retention));
    }

    /// Land a finished scheduled local backup (#888), called each loop tick. On
    /// success stamps `last_local_backup` and notes the archive quietly; a failure
    /// surfaces its cause — never silently lost — and leaves the stamp untouched so
    /// the cadence retries. A dead worker (a panic in the zip) reports its cause via
    /// #855's cause-carrying, falling back to a generic message.
    pub fn poll_local_backup(&mut self) {
        match self.local_backup_task.poll() {
            Poll::Ready(Ok(path)) => {
                self.last_local_backup = Some(Instant::now());
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into(),
                );
                self.message = Some(format!("Backed up locally to {name}."));
            }
            Poll::Ready(Err(cause)) => {
                self.message = Some(format!("Local backup failed: {cause} — will retry."));
            }
            Poll::Pending => {}
            Poll::Died(cause) => {
                self.message = Some(cause.map_or_else(
                    || "Local backup failed — will retry.".into(),
                    |cause| format!("Local backup failed: {cause} — will retry."),
                ));
            }
        }
    }

    /// Warn on open when the project's newest local archive is older than the
    /// configured staleness window (#888), so the author learns their exposure the
    /// moment they open the book rather than discovering a long gap later. Returns
    /// the warning to fold into the open message, or `None` when local backups are
    /// off, the warning is disabled, no backups dir resolves, or the latest archive
    /// is recent enough (including a never-backed-up project, which the cadence will
    /// itself address on its first tick). Reads only the backups dir — never the
    /// project — so it costs an open nothing proportional to the novel.
    pub(crate) fn stale_local_backup_warning(&self) -> Option<String> {
        let window = self.backup.stale_after()?;
        // Only warn once local backups are actually turned on for this project;
        // a project the author hasn't opted in for is not "stale".
        if !self.backup.local_enabled {
            return None;
        }
        let dir = config::backups_dir(&self.project.root)?;
        let latest = latest_archive_mtime(&dir)?;
        let age = SystemTime::now()
            .duration_since(latest)
            .unwrap_or(Duration::ZERO);
        (age >= window).then(|| {
            format!(
                "Local backup is stale: last archive was {} ago.",
                stale_age_label(age)
            )
        })
    }

    /// How long ago the last successful *local* backup was this session, or `None`
    /// if none has succeeded (#888). Exposed for the status bar and tests.
    pub(crate) fn last_local_backup_age(&self) -> Option<Duration> {
        self.last_local_backup.map(|at| at.elapsed())
    }

    // --- Browse & restore local backups (#934) ----------------------------

    /// The project's local archive backups, newest first (#934): each entry's file
    /// name, path, size, and last-modified time, read from the backups dir #888
    /// writes into. Empty when no data dir resolves (hermetically stubbed off in
    /// tests) or the directory is absent — a clean no-op the browser reports as
    /// "no backups yet". Reads only the backups dir (a `stat` per archive), never
    /// the project, so it costs nothing proportional to the novel.
    pub(crate) fn list_local_backups(&self) -> Vec<BackupEntry> {
        let Some(dir) = config::backups_dir(&self.project.root) else {
            return Vec::new();
        };
        list_backups_in(&dir)
    }
}

/// The archive backups directly under `dir`, newest first (#934): only our own
/// `.zip` output ([`archive_names`]), each stamped with its size and mtime. An
/// archive whose metadata can't be read is skipped (best-effort, like the rest of
/// the backup path) rather than failing the whole listing. Pure over a directory,
/// so the browser's list is unit-testable against a seeded fixture.
fn list_backups_in(dir: &Path) -> Vec<BackupEntry> {
    let mut entries: Vec<BackupEntry> = archive_names(dir)
        .into_iter()
        .filter_map(|name| {
            let path = dir.join(&name);
            let meta = fs::metadata(&path).ok()?;
            Some(BackupEntry {
                name,
                path,
                size: meta.len(),
                modified: meta.modified().ok()?,
            })
        })
        .collect();
    // Newest first: the most recent archive is the likeliest restore target and
    // leads the list. `archive_names` sorts by timestamped name (oldest first), so
    // sort by mtime and reverse to put the freshest write on top.
    entries.sort_by_key(|e| e.modified);
    entries.reverse();
    entries
}

/// Archive the project at `root` into `dir` under a timestamped name, then prune
/// older archives to `keep` (`None` keeps everything). Runs on a worker thread;
/// returns the archive's path on success or an author-facing cause on failure
/// (#888). Prune failures are swallowed — a stale extra archive is harmless and
/// must never fail an otherwise-good backup.
fn run_local_backup(root: &Path, dir: &Path, keep: Option<usize>) -> LocalBackupOutcome {
    let dest = dir.join(archive::default_name(root));
    archive::write_archive(root, &dest).map_err(|e| e.to_string())?;
    if let Some(keep) = keep {
        prune_archives(dir, keep);
    }
    Ok(dest)
}

/// Every `.zip` archive directly under `dir`, by file name, sorted ascending — so
/// the timestamped [`archive::default_name`] names sort oldest-first. An unreadable
/// directory yields an empty list (best-effort). Only our own `.zip` output is
/// considered; other files are ignored.
fn archive_names(dir: &Path) -> Vec<String> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(ZIP_SUFFIX))
        .collect();
    names.sort();
    names
}

/// Which archive names to prune to keep only the newest `keep` (#888): pure over a
/// sorted (oldest-first) list, so the retention rule is unit-testable without a
/// filesystem. Returns the oldest names beyond the retention count; keeps all when
/// there are `keep` or fewer.
fn to_prune(sorted_names: &[String], keep: usize) -> &[String] {
    let excess = sorted_names.len().saturating_sub(keep);
    &sorted_names[..excess]
}

/// Delete all but the newest `keep` archives under `dir` (#888). Best-effort: a
/// failed delete is ignored, since a lingering extra archive is harmless and must
/// never fail the backup that produced it.
fn prune_archives(dir: &Path, keep: usize) {
    let names = archive_names(dir);
    for name in to_prune(&names, keep) {
        let _ = fs::remove_file(dir.join(name));
    }
}

/// The newest local archive's last-modified time under `dir` (#888), or `None` when
/// the directory is absent/empty or no archive's mtime is readable. Used on open to
/// judge staleness. Names sort by timestamp, but mtime is the honest "when it was
/// written", so this reads it rather than parsing the name.
fn latest_archive_mtime(dir: &Path) -> Option<SystemTime> {
    archive_names(dir)
        .iter()
        .filter_map(|name| fs::metadata(dir.join(name)).and_then(|m| m.modified()).ok())
        .max()
}

/// A compact "how old the last backup is" label for the staleness warning (#888):
/// hours under a day, then days. Coarse on purpose — the warning is "this is old",
/// not a precise timestamp.
fn stale_age_label(age: Duration) -> String {
    let hours = age.as_secs() / 3600;
    if hours < 24 {
        format!("{hours}h")
    } else {
        format!("{}d", hours / 24)
    }
}

#[cfg(test)]
impl App {
    /// Force the on-save backup flag for a test (#887), standing in for a save
    /// having just landed without driving the whole save path.
    pub(crate) fn mark_backup_pending_for_test(&mut self) {
        self.backup_pending = true;
    }

    /// Whether an on-save backup is still pending (#887) — lets a test assert the
    /// hook cleared it (fired, or was gated off) rather than leaving it stuck on.
    pub(crate) const fn backup_pending_for_test(&self) -> bool {
        self.backup_pending
    }

    /// Whether a backup push is currently in flight (#887).
    pub(crate) const fn backup_in_flight_for_test(&self) -> bool {
        self.backup_task.is_busy()
    }

    /// Whether a successful backup has stamped `last_backup` this session (#887).
    pub(crate) const fn has_backed_up_for_test(&self) -> bool {
        self.last_backup.is_some()
    }

    /// Evaluate the cadence's due-decision directly (#887), so a test drives the
    /// interval logic with an injected `now` and no wall clock.
    pub(crate) fn backup_due_for_test(&self, now: Instant) -> bool {
        self.backup_due(now)
    }

    /// Backdate the last-attempt clock by `secs` (#887), so a test can make the
    /// timed interval due without waiting real seconds.
    pub(crate) fn age_backup_clock_for_test(&mut self, secs: u64) {
        self.last_backup_attempt = Instant::now()
            .checked_sub(Duration::from_secs(secs))
            .unwrap_or_else(Instant::now);
    }

    /// Spawn a stand-in backup push whose worker returns `outcome` (#887), so a
    /// hermetic test drives [`poll_backup`](Self::poll_backup) — success or a
    /// failing push that surfaces its cause — without a real remote or network.
    /// Mirrors [`spawn_backup`](Self::spawn_backup): it clears the pending flag and
    /// advances the attempt clock, so the reschedule-on-failure path is exercised too.
    pub(crate) fn spawn_backup_with_outcome_for_test(&mut self, outcome: BackupOutcome) {
        self.backup_pending = false;
        self.last_backup_attempt = Instant::now();
        self.backup_task.spawn(move || outcome);
    }

    /// Drive [`poll_backup`](Self::poll_backup) to completion for a test (#887):
    /// tick until the worker's outcome lands. Bounded so a stuck worker fails fast.
    pub(crate) fn settle_backup_for_test(&mut self) {
        for _ in 0..10_000 {
            self.poll_backup();
            if !self.backup_task.is_busy() {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("backup task never settled");
    }

    /// Evaluate the local cadence's due-decision directly (#888), so a test drives
    /// the interval logic with an injected `now` and no wall clock.
    pub(crate) fn local_backup_due_for_test(&self, now: Instant) -> bool {
        self.local_backup_due(now)
    }

    /// Backdate the last local-attempt clock by `secs` (#888), so a test can make
    /// the local cadence due without waiting real seconds.
    pub(crate) fn age_local_backup_clock_for_test(&mut self, secs: u64) {
        self.last_local_backup_attempt = Instant::now()
            .checked_sub(Duration::from_secs(secs))
            .unwrap_or_else(Instant::now);
    }

    /// Fire a scheduled local backup synchronously for a test (#888): run the
    /// archive-and-prune the worker would, against the given `dir`, and land the
    /// success stamp — no thread, so the test is deterministic. Returns the outcome.
    pub(crate) fn run_local_backup_for_test(
        &mut self,
        dir: &Path,
        keep: Option<usize>,
    ) -> LocalBackupOutcome {
        self.last_local_backup_attempt = Instant::now();
        let out = run_local_backup(&self.project.root, dir, keep);
        if out.is_ok() {
            self.last_local_backup = Some(Instant::now());
        }
        out
    }

    /// Whether a successful local backup has stamped `last_local_backup` this
    /// session (#888).
    pub(crate) const fn has_backed_up_locally_for_test(&self) -> bool {
        self.last_local_backup.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_the_newest_and_drops_the_oldest() {
        // Names sort oldest-first (the timestamped default_name form); pruning to
        // keep 2 drops all but the two newest.
        let names: Vec<String> = ["book-a.zip", "book-b.zip", "book-c.zip", "book-d.zip"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert_eq!(
            to_prune(&names, 2),
            &names[..2],
            "the two oldest are pruned"
        );
        assert_eq!(to_prune(&names, 4), &[] as &[String], "keep == len: none");
        assert_eq!(to_prune(&names, 9), &[] as &[String], "keep > len: none");
    }

    #[test]
    fn stale_label_reads_hours_then_days() {
        assert_eq!(stale_age_label(Duration::from_secs(5 * 3600)), "5h");
        assert_eq!(stale_age_label(Duration::from_secs(23 * 3600)), "23h");
        assert_eq!(stale_age_label(Duration::from_secs(50 * 3600)), "2d");
    }

    #[test]
    fn listing_orders_newest_first_and_skips_non_archives() {
        // Three archives at distinct mtimes plus a non-`.zip` file: the listing is
        // newest-first and includes only our own archives (#934).
        let dir = crate::testutil::scratch_dir("backup", "list-order");
        fs::create_dir_all(&dir).unwrap();
        for (name, ago) in [
            ("Book-2000-01-01-000000.zip", 3 * 3600),
            ("Book-2000-01-02-000000.zip", 3600),
            ("Book-2000-01-03-000000.zip", 60),
        ] {
            let path = dir.join(name);
            fs::write(&path, b"zip").unwrap();
            let when = SystemTime::now() - Duration::from_secs(ago);
            fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(when)
                .unwrap();
        }
        fs::write(dir.join("notes.txt"), b"not an archive").unwrap();

        let entries = list_backups_in(&dir);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Book-2000-01-03-000000.zip",
                "Book-2000-01-02-000000.zip",
                "Book-2000-01-01-000000.zip",
            ],
            "newest-first, .zip only"
        );
        assert!(entries.iter().all(|e| e.size > 0), "sizes read");

        let _ = fs::remove_dir_all(&dir);
    }
}
