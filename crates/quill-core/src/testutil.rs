//! Shared test scaffolding (#428). Only compiled for tests: this crate's own, and
//! the `quill` binary's through the `test-support` feature (#1229).
//!
//! The "unique-per-process scratch directory" contract is load-bearing for
//! parallel-test isolation — every test binary runs concurrently, so two tests
//! must never share a path. Keeping it in one place means the contract (and its
//! cleanup) can't drift across the dozen modules that need a scratch dir.

#![allow(
    clippy::unwrap_used,
    reason = "test scaffolding: a scratch dir that can't be made should fail the test loudly"
)]

use std::path::PathBuf;

/// A unique-per-process scratch directory for a test:
/// `<tmp>/quill-<prefix>-<pid>-<name>`, emptied if it lingered from a prior run
/// and freshly created. The pid isolates parallel test binaries; `prefix`
/// (the module) and `name` (the test) keep tests from colliding. Callers that own
/// a `TempProject` get Drop cleanup on top; the rest remove it at the end.
pub fn scratch_dir(prefix: &str, name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quill-{prefix}-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The CI slack factor for the wall-clock perf guards (see the budget helpers
/// below). The timing thresholds are tuned for real hardware. A GitHub-hosted
/// runner is a shared 2-vCPU VM that runs this CPU-bound work ~2.5–3x slower than
/// a dev machine (the continuous-view window reparse measured ~7ms locally vs
/// ~20ms on CI), so a budget honest on a real box false-fails there. When the
/// `CI` env var is set — GitHub Actions sets it — the budget is tripled, so the
/// informational `cargo perf` job only reddens on a gross (>3×) regression. Off
/// CI the factor is 1, keeping `cargo perf` on real hardware the strict,
/// authoritative pre-release gate (see `docs/performance.md`). Scale-independence
/// guards (a ratio of a large run to a small one) already cancel the hardware
/// factor and deliberately don't use this.
fn perf_slack() -> u128 {
    if std::env::var_os("CI").is_some() {
        3
    } else {
        1
    }
}

/// A perf-guard budget in milliseconds, widened on CI ([`perf_slack`]). Compare a
/// measured `Duration` against it via `.as_millis()`.
pub fn perf_budget_ms(base_ms: u128) -> u128 {
    base_ms * perf_slack()
}

/// A perf-guard budget in microseconds, widened on CI ([`perf_slack`]). Compare a
/// measured `Duration` against it via `.as_micros()`.
pub fn perf_budget_us(base_us: u128) -> u128 {
    base_us * perf_slack()
}
