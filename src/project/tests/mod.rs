//! Tests for the `project` module, split by area into submodules to keep each
//! file under the ~1,000-line convention (#1190; the module itself was split out
//! in #404). The shared scratch-dir helper lives here; each submodule pulls it in
//! with `use super::*`.

use super::*;

mod chapters;
mod dictionary;
mod failed_reads;
mod goals;
mod manifest;
mod parts;
mod perf;

/// A fresh scratch directory for a hermetic project test, namespaced under
/// `proj`. Shared by every submodule so each test builds its project in
/// isolation.
pub(super) fn temp_dir(name: &str) -> PathBuf {
    crate::testutil::scratch_dir("proj", name)
}
