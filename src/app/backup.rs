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

use std::time::{Duration, Instant};

use super::task::Poll;
use super::App;

/// The outcome of an automatic backup push, sent from the worker thread back to
/// the UI thread (#887): the pushed branch on success, or the author-facing cause
/// on failure (already run through the VCS error surfacing).
pub type BackupOutcome = Result<String, String>;

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
        if !self.backup.enabled || self.backup_task.is_busy() {
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
}
