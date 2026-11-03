//! Automatic version history in the editor (#1296). Opening a book starts a
//! history when it has none ([`crate::vcs::start_auto_history`]), and the editor
//! then snapshots it on its own: once [`HistoryConfig::snapshot_after`](crate::config::HistoryConfig::snapshot_after) has
//! passed since the last save, and when the book is closed. The author never
//! commits, and the history view fills in anyway.
//!
//! Starting a history and taking a snapshot both stage and hash the book, so
//! they run on their own worker tasks, never the UI thread; the typing and
//! autosave "never writes on the UI thread" guards still hold. The snapshot on
//! close is the exception, taken as the editor shuts down, after every save.
//!
//! Snapshots only go into a history Quill started, unless the author turns on
//! `existing_repos`, and never into a book that's part of a larger repository.
//! Nothing is pushed anywhere: that's `[backup]`, with a remote.

use std::time::Instant;

use super::task::Poll;
use super::App;
use crate::vcs::{self, AutoHistory};

/// The note shown the first time a book gets a history.
const STARTED_NOTE: &str = "Quill now keeps version history for this book: Ctrl-V, then History.";

/// What starting a book's history found, sent back from its worker.
pub type StartOutcome = Result<AutoHistory, String>;

/// What a snapshot did, sent back from its worker: the new commit's short id,
/// or `None` when nothing had changed.
pub type SnapshotOutcome = Result<Option<String>, String>;

impl App {
    /// Start automatic history for the open book, off the UI thread, when the
    /// `[history]` config has it on. The result lands in
    /// [`poll_auto_history`](Self::poll_auto_history). Called when the editor
    /// opens a book, and again whenever it opens another in its place.
    pub fn start_auto_history(&mut self) {
        self.auto_history_armed = true;
        if !self.history.automatic || self.project.is_read_only() {
            return;
        }
        let root = self.project.root.clone();
        self.history_start_task
            .spawn(move || vcs::start_auto_history(&root));
    }

    /// Land a finished start (with a one-time note when the history is new) and
    /// a finished snapshot, called each loop tick.
    pub fn poll_auto_history(&mut self) {
        let started = self.history_start_task.poll();
        self.land_start(started);
        match self.snapshot_task.poll() {
            // A snapshot is quiet: the history view shows it, and the status
            // bar's change count drops to zero on its next refresh.
            Poll::Ready(Ok(_)) => self.mark_vc_status_dirty(),
            Poll::Ready(Err(cause)) => {
                self.message = Some(format!("Couldn't save a version history snapshot: {cause}"));
            }
            Poll::Died(cause) => {
                self.message = Some(format!(
                    "Couldn't save a version history snapshot: {}",
                    cause.unwrap_or_else(|| "the worker stopped".into())
                ));
            }
            Poll::Pending => {}
        }
    }

    /// Apply what starting the history found: whether to snapshot, and the
    /// one-time note when the history is new.
    fn land_start(&mut self, started: Poll<StartOutcome>) {
        match started {
            Poll::Ready(Ok(found)) => {
                self.auto_snapshots = found.snapshots(self.history.existing_repos);
                if found == AutoHistory::Started {
                    self.under_vc = true;
                    self.mark_vc_status_dirty();
                    self.message = Some(STARTED_NOTE.into());
                }
            }
            Poll::Ready(Err(cause)) => {
                self.message = Some(format!("Couldn't start version history: {cause}"));
            }
            Poll::Died(cause) => {
                self.message = Some(format!(
                    "Couldn't start version history: {}",
                    cause.unwrap_or_else(|| "the worker stopped".into())
                ));
            }
            Poll::Pending => {}
        }
    }

    /// Re-read `[history]` after a settings change: turning it off stops the
    /// snapshots at once, and turning it on starts the book's history.
    pub(crate) fn apply_history_config(&mut self) {
        let was_on = self.history.automatic;
        self.history = crate::config::HistoryConfig::load();
        if !self.history.automatic {
            self.auto_snapshots = false;
            self.snapshot_owed_since = None;
        } else if !was_on && self.auto_history_armed && !self.history_start_task.is_busy() {
            self.start_auto_history();
        }
    }

    /// Note that a save reached disk, so a snapshot is owed once the author has
    /// paused for [`HistoryConfig::snapshot_after`](crate::config::HistoryConfig::snapshot_after). Each save restarts the
    /// wait, so a snapshot lands in a pause rather than mid-flow.
    pub(crate) fn mark_snapshot_owed(&mut self) {
        if self.auto_snapshots {
            self.snapshot_owed_since = Some(Instant::now());
        }
    }

    /// Take the owed snapshot, off the UI thread, once enough time has passed
    /// since the last save. Called each loop tick; waits while any other
    /// repository work (a push, pull, or backup) is running.
    pub(crate) fn tick_auto_history(&mut self) {
        let Some(after) = self.history.snapshot_after() else {
            return;
        };
        let due = self
            .snapshot_owed_since
            .is_some_and(|since| since.elapsed() >= after);
        if !due || !self.auto_snapshots || self.snapshot_task.is_busy() || self.repo_busy() {
            return;
        }
        self.snapshot_owed_since = None;
        let root = self.project.root.clone();
        self.snapshot_task.spawn(move || vcs::snapshot(&root));
    }

    /// Snapshot the book as it's closed (quitting, or opening another book in
    /// its place), after its saves have landed. Runs here rather than on a
    /// worker because the editor is about to let go of the book.
    pub(crate) fn snapshot_on_close(&mut self) {
        // A start still running decides whether this book snapshots at all;
        // a snapshot still running must finish before another commit begins.
        let started = self.history_start_task.wait();
        self.land_start(started);
        let _ = self.snapshot_task.wait();
        if !self.auto_snapshots || self.project.is_read_only() {
            return;
        }
        if let Err(cause) = vcs::snapshot(&self.project.root) {
            self.message = Some(format!("Couldn't save a version history snapshot: {cause}"));
        }
        self.snapshot_owed_since = None;
    }
}

#[cfg(test)]
impl App {
    /// Wait for a start and a snapshot in flight to finish, and land them as
    /// the event loop's poll would.
    pub(crate) fn settle_auto_history_for_test(&mut self) {
        let started = self.history_start_task.wait();
        self.land_start(started);
        if let Poll::Ready(Err(cause)) = self.snapshot_task.wait() {
            panic!("snapshot failed: {cause}");
        }
    }
}
