//! Shared test scaffolding (#428). Only compiled under `cfg(test)`.
//!
//! The "unique-per-process scratch directory" contract is load-bearing for
//! parallel-test isolation — every test binary runs concurrently, so two tests
//! must never share a path. Keeping it in one place means the contract (and its
//! cleanup) can't drift across the dozen modules that need a scratch dir.

use std::path::PathBuf;

/// A unique-per-process scratch directory for a test:
/// `<tmp>/quill-<prefix>-<pid>-<name>`, emptied if it lingered from a prior run
/// and freshly created. The pid isolates parallel test binaries; `prefix`
/// (the module) and `name` (the test) keep tests from colliding. Callers that own
/// a `TempProject` get Drop cleanup on top; the rest remove it at the end.
pub(crate) fn scratch_dir(prefix: &str, name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quill-{prefix}-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
