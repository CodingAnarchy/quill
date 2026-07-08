//! The cached uncommitted-changes count (#132) and its idle debounce (#779),
//! with the #869 backoff for an untouched tree. Child module of `vcs_actions`, so
//! these `impl App` methods keep access to `App`'s private fields; the
//! `#[cfg(test)]` helpers let the vc-status tests spin the idle path to a settled
//! count without a wall-clock race.

use super::super::*;

/// Base interval the cached uncommitted-changes count may stand before the idle
/// path recomputes it to catch edits made outside Quill (#779). Long enough that
/// pure typing doesn't re-walk the tree every input burst, short enough that an
/// external change surfaces promptly. The fallback backs off past this when
/// successive walks find nothing changed (#869), so an untouched editor doesn't
/// re-walk the whole tree on a fixed short clock — see
/// [`vc_status_idle_interval`](App::vc_status_idle_interval).
pub const VC_STATUS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// The ceiling the idle fallback backs off to when successive walks are unchanged
/// (#869): an idle editor still re-checks for external edits, just at most this
/// often instead of every base interval.
const VC_STATUS_INTERVAL_MAX: std::time::Duration = std::time::Duration::from_secs(300);

impl App {
    /// Refresh the cached uncommitted-changes count (#132) synchronously — a
    /// `.gitignore`-respecting working-tree walk. Called after commit / init /
    /// restore / pull, where the status has demonstrably just changed and the
    /// count must be current before the next frame. The per-idle-tick refresh
    /// takes the debounced, off-thread [`refresh_vc_status_on_idle`] path instead
    /// (#779). Zero when the project isn't under version control.
    pub(crate) fn refresh_vc_status(&mut self) {
        self.vc_changes = if self.under_vc {
            crate::vcs::status(&self.project.root).map_or(0, |s| s.count())
        } else {
            0
        };
        // The count is current now; reset the interval clock and clear the pending
        // recompute so the idle path doesn't immediately redo this walk. A commit /
        // restore / pull is a real change, so drop the idle backoff back to the base
        // interval (#869) — the tree is active again.
        self.last_vc_refresh = std::time::Instant::now();
        self.vc_status_dirty = false;
        self.vc_status_idle_interval = VC_STATUS_INTERVAL;
    }

    /// Mark that the working tree may have changed (#779), so the next idle tick
    /// recomputes the uncommitted-changes count off-thread. Called from the save
    /// path, where an in-app edit was just written to disk — the one moment pure
    /// typing can flip git status — without blocking the save on a status walk.
    pub(crate) const fn mark_vc_status_dirty(&mut self) {
        self.vc_status_dirty = true;
        // A save just changed the tree, so stop backing off — the next walk should
        // run at the base interval, not a stretched idle one (#869).
        self.vc_status_idle_interval = VC_STATUS_INTERVAL;
    }

    /// The idle-tick refresh of the uncommitted-changes count (#779). Unlike the
    /// synchronous [`refresh_vc_status`], this never walks the tree on the UI
    /// thread and never walks it merely because the author typed: it recomputes
    /// only when a save marked the status dirty or the bounded interval has
    /// elapsed (the fallback that catches external edits), and it runs the walk on
    /// a worker thread so a large repository can't stutter the editor.
    pub(crate) fn refresh_vc_status_on_idle(&mut self) {
        // First, land a finished walk's count. An interval-fallback walk that finds
        // the count unchanged means the tree is idle, so back off (double, to a cap)
        // before the next fallback re-walk (#869); any actual change snaps back to
        // the base interval so a freshly-active tree is picked up promptly again.
        if let Poll::Ready(count) = self.vc_status_task.poll() {
            if count == self.vc_changes {
                self.vc_status_idle_interval =
                    (self.vc_status_idle_interval * 2).min(VC_STATUS_INTERVAL_MAX);
            } else {
                self.vc_status_idle_interval = VC_STATUS_INTERVAL;
            }
            self.vc_changes = count;
        }
        if !self.under_vc {
            self.vc_changes = 0;
            return;
        }
        // Only walk when the status can have changed (a save marked it dirty) or
        // the interval fallback is due — never on every idle burst from typing.
        let due =
            self.vc_status_dirty || self.last_vc_refresh.elapsed() >= self.vc_status_idle_interval;
        if !due || self.vc_status_task.is_busy() {
            return;
        }
        self.vc_status_dirty = false;
        self.last_vc_refresh = std::time::Instant::now();
        let root = self.project.root.clone();
        self.vc_status_task
            .spawn(move || crate::vcs::status(&root).map_or(0, |s| s.count()));
    }

    /// Whether an off-thread status walk is in flight or a save has marked the
    /// count stale (#779) — the two states in which [`refresh_vc_status_on_idle`]
    /// still has work to land. Lets a test spin the idle path deterministically to
    /// a settled count without a timing race.
    #[cfg(test)]
    pub(crate) const fn vc_status_settling(&self) -> bool {
        self.vc_status_dirty || self.vc_status_task.is_busy()
    }

    /// Whether the status has been marked stale (a save happened) — the specific
    /// invariant pure typing must NOT trip (#779). Unlike `vc_status_settling` this
    /// ignores an in-flight walk, so a test can assert "typing marked nothing"
    /// without racing the wall-clock interval fallback, which may legitimately
    /// spawn a walk on an idle tick regardless of typing.
    #[cfg(test)]
    pub(crate) const fn vc_status_marked_dirty(&self) -> bool {
        self.vc_status_dirty
    }

    /// The current idle-fallback interval (#869), so a test can assert the backoff
    /// stretches it when successive walks find nothing changed and resets it on a
    /// real change.
    #[cfg(test)]
    pub(crate) const fn vc_status_idle_interval_for_test(&self) -> std::time::Duration {
        self.vc_status_idle_interval
    }

    /// Backdate the interval clock so the next idle refresh's fallback is due
    /// (#779), letting a test exercise the "catch an external edit" path without
    /// waiting real wall-clock seconds.
    #[cfg(test)]
    pub(crate) fn expire_vc_status_interval_for_test(&mut self) {
        // Backdate past the *current* effective interval, not just the base, so the
        // fallback is due even after the idle backoff has stretched it (#869).
        self.last_vc_refresh = std::time::Instant::now()
            .checked_sub(self.vc_status_idle_interval)
            .unwrap_or_else(std::time::Instant::now);
    }

    /// Drive the debounced idle status refresh (#779) to completion for tests:
    /// tick the idle path until a save-marked walk has been spawned and its count
    /// applied, so an assertion sees the settled `vc_changes` rather than racing
    /// the worker thread. Bounded so a stuck walk can't hang the test.
    #[cfg(test)]
    pub(crate) fn settle_vc_status(&mut self) {
        // Sleep (not `yield_now`) between polls: under heavy parallel test load the
        // off-thread git walk's worker can be CPU-starved, and a busy spin never
        // relinquishes the core for it to run. Sleeping yields the CPU so the walk
        // finishes; a 10s ceiling still fails fast on a genuinely stuck walk.
        for _ in 0..10_000 {
            self.refresh_vc_status_on_idle();
            if !self.vc_status_settling() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("vc status never settled");
    }
}
