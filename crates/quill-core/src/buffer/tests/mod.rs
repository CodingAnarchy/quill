//! Tests for the `src/buffer/mod.rs` module, split by concern to keep each file
//! under the ~1,000-line convention (#404, #896). Shared file helpers live here;
//! each area-specific submodule pulls them in with `use super::*`, mirroring the
//! `ui/tests/` split (#792). Pure move — no behavior change.

use super::*;
use std::path::{Path, PathBuf};

mod editing;
mod graphemes;
mod motions;
mod save_load;
mod utf16;
mod word_count;

/// A unique-per-process path for a test file, inside a fresh scratch dir (#428).
/// `name` keeps its extension (some tests assert `.md`-vs-other handling), so it
/// names both the dir and the file within it.
fn temp_path(name: &str) -> PathBuf {
    crate::testutil::scratch_dir("buffer", name).join(name)
}

fn write(path: &Path, bytes: &str) {
    fs::write(path, bytes).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}
