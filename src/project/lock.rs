//! Advisory single-instance project lock (#871).
//!
//! Two Quill processes open on one project is last-writer-wins on chapters, and
//! both clobber the shared sidecars (`recovery.json`, `lastpos.json`, `book.toml`)
//! — a crash then loses one instance's unsaved work. This serializes them: at
//! project *open* the first instance takes an exclusive lockfile; a second
//! instance detects it and opens **read-only** rather than racing writes.
//!
//! The lock is an `O_EXCL` (`create_new`) file at `.quill/lock` holding the
//! owner's `pid host started_at`. `O_EXCL` makes "create it or fail" a single
//! atomic step, so two instances can't both believe they hold it. A leftover
//! lock from a *crashed* instance would otherwise wedge the project read-only
//! forever, so a same-host lock whose pid is no longer alive is treated as stale
//! and reclaimed. A different-host lock (a project on a synced/shared filesystem)
//! is never assumed stale — we can't probe another machine's processes — so it is
//! honored conservatively (read-only).
//!
//! The guard is RAII: dropping [`InstanceLock`] removes the file it created, so
//! the lock lifecycle lives with the [`Project`](super::Project) that owns it and
//! never needs a release call in the quit path.

use std::fs::{self, File};
#[cfg(test)]
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const DIR: &str = ".quill";
const FILE: &str = "lock";

/// A process-unique counter distinguishing lockfiles this process holds at the
/// same time (e.g. an in-place reopen of the same project). Written into the
/// lockfile as a token so a guard's `Drop` only removes the file it actually
/// created — never a fresher lock a same-process reopen replaced it with.
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// The outcome of trying to lock a project at open.
pub enum LockOutcome {
    /// This process now holds the lock; the guard releases it on drop.
    Held(InstanceLock),
    /// The project is already open in another live instance (or on another host).
    /// Open read-only rather than racing its writes; `holder` describes the owner
    /// for a warning to the author.
    Busy { holder: String },
}

/// An RAII guard over a project's instance lockfile. Holding this proves this
/// process owns the project; dropping it removes the lockfile so the next open
/// succeeds. The lock lives on the [`Project`](super::Project), so its lifetime
/// is exactly the project's — no explicit release in the quit path (#871).
pub struct InstanceLock {
    path: PathBuf,
    token: u64,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Only remove the lockfile if it's still the one we wrote: read back the
        // token and match it. This makes a same-process reopen safe — if a fresh
        // lock replaced ours (a different token), dropping the old guard leaves the
        // fresh one in place instead of orphaning a live project. A gone or
        // unreadable file is nothing to clean up.
        let mine = fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| Holder::parse(&t))
            .is_some_and(|h| h.token == self.token);
        if mine {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// The lockfile path under `root`.
fn path(root: &Path) -> PathBuf {
    root.join(DIR).join(FILE)
}

/// This machine's hostname, used to scope the staleness check: a pid is only
/// meaningfully probed on the host that owns it. Falls back to a fixed marker
/// when the environment doesn't expose one — that only makes the check more
/// conservative (an unknown host never matches, so its lock is never reclaimed).
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "unknown-host".to_string())
}

/// Whether process `pid` is alive on *this* host. Uses `kill(pid, 0)`, which
/// sends no signal but performs the permission/existence check: `Ok` (or
/// `EPERM`) means the process exists, `ESRCH` means it's gone. On a non-Unix
/// target we can't cheaply probe, so we conservatively report "alive" — a lock
/// is honored (read-only) rather than risk reclaiming one that's still held.
#[cfg(unix)]
fn pid_is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: `kill` with signal 0 only checks for the process; it changes no
    // memory and has no aliasing concerns. Errno is inspected via the safe
    // `io::Error::last_os_error` right after the call.
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    // EPERM: the process exists but we may not signal it — still alive.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: i32) -> bool {
    true // can't probe portably; honor the lock rather than reclaim it
}

/// The lockfile's recorded contents: the owning pid, the host it ran on, and a
/// free-form note. Only the first two lines are load-bearing; anything after is
/// a human-readable description and is ignored when parsing.
struct Holder {
    pid: i32,
    host: String,
    /// Distinguishes concurrent lockfiles this same process created (see
    /// [`NEXT_TOKEN`]); 0 for a hand-written or legacy file with no token line.
    token: u64,
}

impl Holder {
    /// Serialize for the lockfile: pid, host, and token on their own lines, then a
    /// note so an author who opens the file by hand sees what it is.
    fn serialize(&self) -> String {
        format!(
            "{}\n{}\n{}\nquill: project open in this process; remove only if quill isn't running\n",
            self.pid, self.host, self.token
        )
    }

    /// Parse a lockfile body. A malformed or truncated file (a torn write) yields
    /// `None`, which the caller treats as a reclaimable stale lock rather than
    /// wedging the project forever. The token line is optional (0 if absent) so a
    /// hand-written or older lock still parses.
    fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let pid = lines.next()?.trim().parse().ok()?;
        let host = lines.next()?.trim().to_string();
        if host.is_empty() {
            return None;
        }
        let token = lines
            .next()
            .and_then(|l| l.trim().parse().ok())
            .unwrap_or(0);
        Some(Self { pid, host, token })
    }

    /// A short description of the holder for a warning to the author.
    fn describe(&self) -> String {
        format!("another Quill instance (pid {} on {})", self.pid, self.host)
    }

    /// Whether this recorded holder should block us from taking the lock. A lock
    /// from a dead same-host pid is stale (a crashed instance) and reclaimable; a
    /// lock from another host is honored conservatively (we can't probe it). A lock
    /// recording *our own* pid is a same-process reopen (e.g. an in-place project
    /// rename moves the folder and its lockfile, then reopens it): that's us, not a
    /// competitor, so it never blocks — otherwise a rename would deadlock against
    /// its own lock and open the renamed project read-only.
    fn blocks(&self, our_host: &str, our_pid: i32) -> bool {
        if self.host == our_host && self.pid == our_pid {
            return false; // our own lock, reopening in-process
        }
        if self.host != our_host {
            return true; // different machine — assume live, don't reclaim
        }
        pid_is_alive(self.pid)
    }
}

/// Attempt to lock the project at `root` at open (#871). Creates `.quill/` if
/// needed, then tries to claim `.quill/lock` exclusively.
///
/// - No lock present → we create it and return [`LockOutcome::Held`].
/// - A lock from a live instance (or another host) → [`LockOutcome::Busy`]: open
///   read-only.
/// - A lock from a *dead* same-host pid (a crash) or an unparseable/torn lock →
///   reclaimed: we remove it and retry the exclusive create once.
///
/// A filesystem error creating `.quill/` or the lockfile is *not* fatal to
/// opening the project — the lock is advisory. It surfaces as `Busy` with a note,
/// so the author is warned but can still read; we never block opening a project
/// because the lock couldn't be written.
pub fn acquire(root: &Path) -> LockOutcome {
    if fs::create_dir_all(root.join(DIR)).is_err() {
        return LockOutcome::Busy {
            holder: "the lock directory couldn't be created".to_string(),
        };
    }
    let lock_path = path(root);
    let our_host = hostname();
    let our_pid = std::process::id() as i32;

    // First attempt: claim it exclusively.
    match try_create(&lock_path, &our_host) {
        Ok(lock) => return LockOutcome::Held(lock),
        Err(TryError::Fs(e)) => {
            return LockOutcome::Busy {
                holder: format!("the lock couldn't be written ({e})"),
            };
        }
        Err(TryError::Exists) => {} // fall through to inspect the holder
    }

    // Something already holds it. Read and classify.
    let existing = fs::read_to_string(&lock_path).ok();
    let holder = existing.as_deref().and_then(Holder::parse);
    match &holder {
        Some(h) if h.blocks(&our_host, our_pid) => LockOutcome::Busy {
            holder: h.describe(),
        },
        // Dead same-host pid, or an unparseable/torn lock: reclaim it and retry the
        // exclusive create exactly once. A racing reclaimer might win the retry, in
        // which case we honor its fresh lock as busy rather than looping.
        _ => {
            let _ = fs::remove_file(&lock_path);
            match try_create(&lock_path, &our_host) {
                Ok(lock) => LockOutcome::Held(lock),
                Err(TryError::Exists) => LockOutcome::Busy {
                    holder: "another Quill instance (just started)".to_string(),
                },
                Err(TryError::Fs(e)) => LockOutcome::Busy {
                    holder: format!("the lock couldn't be written ({e})"),
                },
            }
        }
    }
}

/// The distinct ways an exclusive-create attempt can fail: the file already
/// exists (someone holds it) vs. a real filesystem error.
enum TryError {
    Exists,
    Fs(std::io::Error),
}

/// Try to create the lockfile exclusively (`O_EXCL`) and stamp it with our pid +
/// host. `AlreadyExists` becomes [`TryError::Exists`] so the caller inspects the
/// holder; any other error is a genuine filesystem fault.
fn try_create(lock_path: &Path, our_host: &str) -> Result<InstanceLock, TryError> {
    match File::options().write(true).create_new(true).open(lock_path) {
        Ok(mut f) => {
            let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
            let holder = Holder {
                pid: std::process::id() as i32,
                host: our_host.to_string(),
                token,
            };
            f.write_all(holder.serialize().as_bytes())
                .map_err(TryError::Fs)?;
            let _ = f.sync_all();
            Ok(InstanceLock {
                path: lock_path.to_path_buf(),
                token,
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(TryError::Exists),
        Err(e) => Err(TryError::Fs(e)),
    }
}

/// Read the pid recorded in the lockfile at `root`, if any. Test-only helper so a
/// test can assert *which* process owns the lock without reaching into internals.
#[cfg(test)]
pub fn recorded_pid(root: &Path) -> Option<i32> {
    let mut buf = String::new();
    File::open(path(root)).ok()?.read_to_string(&mut buf).ok()?;
    Holder::parse(&buf).map(|h| h.pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        crate::testutil::scratch_dir("project-lock", name)
    }

    // A live lock owned by a *different* process on this host must be reported
    // busy, not reclaimed. We can't fork a second Quill in a unit test, so we
    // stand in a lockfile owned by pid 1 (init/launchd), which is always alive on
    // Unix — the same code path a real second instance's lockfile drives.
    #[cfg(unix)]
    #[test]
    fn a_live_other_process_lock_is_busy_not_reclaimed() {
        let root = tmpdir("busy");
        fs::create_dir_all(root.join(DIR)).unwrap();
        let live_other = Holder {
            pid: 1, // init/launchd — always alive, never our pid
            host: hostname(),
            token: 0,
        };
        fs::write(path(&root), live_other.serialize()).unwrap();

        match acquire(&root) {
            LockOutcome::Busy { holder } => {
                assert!(
                    holder.contains("pid 1"),
                    "warning names the holder: {holder}"
                );
            }
            LockOutcome::Held(_) => panic!("a live other-process lock must not be reclaimed"),
        }
        // And the busy outcome left the other instance's lock intact.
        assert_eq!(recorded_pid(&root), Some(1), "the live lock is untouched");
    }

    #[test]
    fn our_own_acquire_holds_and_releases() {
        let root = tmpdir("own");
        let lock = match acquire(&root) {
            LockOutcome::Held(l) => l,
            LockOutcome::Busy { .. } => panic!("first acquire should hold the lock"),
        };
        // A same-process reopen (e.g. an in-place rename) is us, not a competitor:
        // it reclaims rather than opening read-only.
        assert!(
            matches!(acquire(&root), LockOutcome::Held(_)),
            "a same-process reopen must not be blocked by our own lock"
        );
        drop(lock);
    }

    #[test]
    fn dropping_the_guard_removes_the_lockfile() {
        let root = tmpdir("release");
        let lock = acquire(&root);
        assert!(path(&root).exists(), "the lockfile exists while held");
        drop(lock);
        assert!(
            !path(&root).exists(),
            "dropping the guard removes the lockfile"
        );
    }

    #[test]
    fn a_stale_same_host_lock_is_reclaimed() {
        let root = tmpdir("stale");
        fs::create_dir_all(root.join(DIR)).unwrap();
        // A lock left by a crashed instance: our host, a pid that can't be alive.
        // pid 2^30 is far above any live pid on the test host.
        let dead = Holder {
            pid: 1 << 30,
            host: hostname(),
            token: 0,
        };
        fs::write(path(&root), dead.serialize()).unwrap();

        // Opening should reclaim it (the recorded pid isn't alive) and hold it.
        match acquire(&root) {
            LockOutcome::Held(_lock) => {
                assert_eq!(
                    recorded_pid(&root),
                    Some(std::process::id() as i32),
                    "the reclaimed lock now records our pid"
                );
            }
            LockOutcome::Busy { holder } => panic!("a dead same-host lock is stale: {holder}"),
        }
    }

    #[test]
    fn a_different_host_lock_is_honored_not_reclaimed() {
        let root = tmpdir("otherhost");
        fs::create_dir_all(root.join(DIR)).unwrap();
        // A lock owned by another machine (a synced folder). We can't probe its
        // process, so it must be honored (read-only), never reclaimed.
        let other = Holder {
            pid: std::process::id() as i32, // even a "live-looking" pid
            host: "some-other-machine".to_string(),
            token: 0,
        };
        fs::write(path(&root), other.serialize()).unwrap();

        match acquire(&root) {
            LockOutcome::Busy { holder } => {
                assert!(holder.contains("some-other-machine"), "{holder}");
            }
            LockOutcome::Held(_) => panic!("a different-host lock must not be reclaimed"),
        }
    }

    #[test]
    fn a_torn_lockfile_is_reclaimed() {
        let root = tmpdir("torn");
        fs::create_dir_all(root.join(DIR)).unwrap();
        // A truncated/garbage lock (an interrupted write): unparseable → stale.
        fs::write(path(&root), "not-a-pid").unwrap();
        assert!(
            matches!(acquire(&root), LockOutcome::Held(_)),
            "an unparseable lock is reclaimed rather than wedging the project"
        );
    }
}
