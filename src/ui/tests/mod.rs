//! Tests for the `src/ui/mod.rs` module, split by concern to keep each file
//! under the ~1,000-line convention (#404, #792). Shared render helpers live
//! here; each area-specific submodule pulls them in with `use super::*`. Pure
//! move — no behavior change.

use super::render_to_string as render;
use super::*;
use crate::buffer::Buffer;
use crate::project::Project;

mod annotation_margin;
mod corkboard_outline;
mod editor_render;
mod fenced_code;
mod folds;
mod overlays;
mod perf;

fn buffer_with(text: &str) -> Buffer {
    let mut b = Buffer::empty();
    for c in text.chars() {
        b.insert_char(c);
    }
    b.refresh_syntax();
    b
}

/// An App over an in-memory scratch project, for render tests.
fn app_with(text: &str) -> App {
    App::new(buffer_with(text), Project::scratch())
}

/// An App over an in-memory project with several chapters (no files needed
/// for rendering, which only reads chapter titles + the active buffer).
fn app_with_chapters(titles: &[&str], active_text: &str) -> App {
    App::new(buffer_with(active_text), Project::scratch_with(titles))
}

/// The screen column of the first non-space character on the first rendered row
/// whose text contains `needle` — how far that row is indented on screen.
fn indent_of_row(app: &App, needle: &str, w: u16, h: u16) -> Option<usize> {
    render(app, w, h)
        .lines()
        .find(|l| l.contains(needle))
        .map(|l| l.len() - l.trim_start().len())
}
