//! Window splits (#163, #164): a second pane beside the focused editor. The
//! other pane holds either another editor (#163) or a read-only surface — a
//! docked report / analysis / AI answer (#164).
//!
//! Asymmetric model: the focused *editor* is always `App::buffer`, so all the
//! existing editing/navigation works unchanged. The other pane is parked in
//! `App::split`. Focusing the other editor swaps the two buffers and re-points
//! the project's active chapter; focusing a docked surface just gives it scroll
//! input (the editor stays `App::buffer`).

use super::*;

use std::path::Path;

/// How the two panes are arranged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SplitDir {
    /// Stacked top/bottom (Vim `:split`).
    Stacked,
    /// Side by side left/right (Vim `:vsplit`).
    SideBySide,
}

/// What the non-focused pane holds. Only ever one instance (in `App::split`), so
/// the variant size gap doesn't matter — no boxing.
#[allow(clippy::large_enum_variant)]
pub(crate) enum OtherPane {
    /// A second editor on a chapter (#163).
    Editor(Buffer),
    /// A docked read-only surface — report / analysis / AI answer (#164).
    /// `focused` is true while it's taking scroll input instead of the editor.
    Surface { overlay: Overlay, focused: bool },
}

pub(crate) struct Split {
    pub(crate) other: OtherPane,
    pub(crate) dir: SplitDir,
    /// Whether the focused editor (`App::buffer`) is the top/left pane.
    pub(crate) focused_first: bool,
    /// Percentage of the split axis given to the first (top/left) pane (#166),
    /// adjusted by `Ctrl-w </>/+/-`. The other pane gets the rest.
    pub(crate) first_ratio: u16,
}

/// What the renderer needs to lay out a split (#163/#164/#166): the shared
/// geometry, plus what the other pane holds.
pub(crate) struct SplitView<'a> {
    pub(crate) dir: SplitDir,
    /// Percentage of the split axis given to the first (top/left) pane.
    pub(crate) first_ratio: u16,
    /// Whether the focused editor (`App::buffer`) is the first (top/left) pane.
    pub(crate) focused_first: bool,
    pub(crate) content: SplitContent<'a>,
}

/// What the non-focused pane shows the renderer (mirrors [`OtherPane`]).
pub(crate) enum SplitContent<'a> {
    /// A second editor pane (#163/#165).
    Editor(&'a Buffer),
    /// A docked read-only surface — report / analysis / AI answer (#164).
    Surface(&'a Overlay),
}

/// The starting (even) split, and the bounds resize keeps the first pane within
/// so neither pane collapses to a sliver (#166).
const EVEN_SPLIT: u16 = 50;
const MIN_RATIO: u16 = 20;
const RESIZE_STEP: u16 = 5;

impl App {
    /// Handle the key after a `Ctrl-w` window prefix.
    pub(super) fn window_command(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('s') => self.split_or_dock(SplitDir::Stacked),
            KeyCode::Char('v') => self.split_or_dock(SplitDir::SideBySide),
            KeyCode::Char('w' | 'h' | 'j' | 'k' | 'l') => self.focus_other_pane(),
            KeyCode::Char('c' | 'q') => self.close_split(),
            // Resize the split (#166): grow or shrink the focused pane along the
            // split axis. `+`/`>` grow, `-`/`<` shrink — both spellings either way.
            KeyCode::Char('+' | '>') => self.resize_split(true),
            KeyCode::Char('-' | '<') => self.resize_split(false),
            _ => {} // unknown window command — cancel silently
        }
    }

    /// `Ctrl-w s`/`v`: dock the open full-screen overlay into a split (#164), or —
    /// when none is open — split into two editors (#163).
    fn split_or_dock(&mut self, dir: SplitDir) {
        if self.overlay.is_some() {
            self.dock_overlay(dir);
        } else {
            self.open_split(dir);
        }
    }

    /// Split the editor into two panes (#163). Both start on the current chapter
    /// (independent buffers); navigate one elsewhere to compare. Single-chapter
    /// view only, and not when already split.
    fn open_split(&mut self, dir: SplitDir) {
        if self.split.is_some() {
            self.message = Some("Already split — Ctrl-w c to close a pane.".into());
            return;
        }
        let path = self.project.active_path();
        self.place_editor_pane(&path, dir, "Split — Ctrl-w w to switch, Ctrl-w c to close.");
    }

    /// Open a project file in a reference pane beside the editor (#165): a chapter
    /// to compare, or a story-bible entry / note to write alongside. It's a normal
    /// editor pane (read, edit, and save independently), reusing the #163 split
    /// machinery. Single-chapter view only.
    pub(super) fn open_file_in_pane(&mut self, path: PathBuf) {
        self.place_editor_pane(&path, SplitDir::SideBySide, "Opened in a pane — Ctrl-w w to focus it, Ctrl-w c to close.");
    }

    /// Load `path` into the other pane as an editor, the shared tail of opening a
    /// split (#163) and a reference pane (#165): leave the continuous view, flush
    /// the focused pane, then replace any existing split (two panes max).
    fn place_editor_pane(&mut self, path: &Path, dir: SplitDir, ok_msg: &str) {
        if self.manuscript.is_some() {
            self.message = Some("Leave the continuous view (Ctrl-l) before opening a pane.".into());
            return;
        }
        if self.flush_active().is_err() {
            return;
        }
        match Buffer::from_path(path) {
            Ok(other) => {
                self.close_split_if_any(); // two panes max — replace any existing split
                self.split = Some(Split {
                    other: OtherPane::Editor(other),
                    dir,
                    focused_first: true,
                    first_ratio: EVEN_SPLIT,
                });
                self.message = Some(ok_msg.into());
            }
            Err(e) => self.message = Some(format!("Open failed: {e}")),
        }
    }

    /// Dock the open full-screen overlay into a pane beside the editor (#164), so
    /// the manuscript stays visible and editable. Single-chapter view only.
    fn dock_overlay(&mut self, dir: SplitDir) {
        let Some(overlay) = self.overlay.take() else { return };
        if self.manuscript.is_some() {
            self.overlay = Some(overlay);
            self.message = Some("Leave the continuous view (Ctrl-l) to dock a report.".into());
            return;
        }
        self.close_split_if_any(); // two panes max — replace any existing split
        self.split = Some(Split {
            other: OtherPane::Surface { overlay, focused: false },
            dir,
            focused_first: true,
            first_ratio: EVEN_SPLIT,
        });
        self.message = Some("Docked — Ctrl-w w to scroll it, Ctrl-w c to close.".into());
    }

    /// Move focus to the other pane: swap editors (#163), or hand scroll input to
    /// a docked surface (#164).
    fn focus_other_pane(&mut self) {
        match self.split.as_ref().map(|s| matches!(s.other, OtherPane::Surface { .. })) {
            None => {}
            Some(true) => {
                if let Some(Split { other: OtherPane::Surface { focused, .. }, .. }) = self.split.as_mut() {
                    *focused = !*focused;
                }
            }
            Some(false) => self.swap_editor_panes(),
        }
    }

    /// Swap which editor is focused (#163). Flush the leaving pane first, then —
    /// if the now-focused pane shows the same chapter — reload it from disk so two
    /// panes on one chapter accumulate edits rather than clobbering.
    fn swap_editor_panes(&mut self) {
        let _ = self.flush_active();
        let other_path = {
            let Some(split) = self.split.as_mut() else { return };
            let OtherPane::Editor(other) = &mut split.other else { return };
            std::mem::swap(&mut self.buffer, other);
            split.focused_first = !split.focused_first;
            other.path.clone()
        };
        if self.buffer.path == other_path {
            if let Some(path) = self.buffer.path.clone() {
                if let Ok(reloaded) = Buffer::from_path(path) {
                    self.buffer = reloaded;
                }
            }
        }
        self.sync_active_to_focus();
    }

    /// Scroll a focused docked surface, or unfocus it on Esc (#164).
    pub(super) fn on_key_surface(&mut self, key: KeyEvent) {
        let height = self.surface_height.get();
        let Some(Split { other: OtherPane::Surface { overlay, focused }, .. }) = self.split.as_mut() else {
            return;
        };
        let max = crate::ui::max_scroll(overlay.lines.len(), height);
        let page = height.saturating_sub(3).max(1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Scroll keys move the surface; Esc hands focus back; anything else is
        // ignored (unlike the full-screen overlay, a docked pane doesn't close).
        match scroll_target(key.code, ctrl, overlay.scroll, max, page) {
            Some(row) => overlay.scroll = row,
            None if key.code == KeyCode::Esc => *focused = false,
            None => {}
        }
    }

    /// Close the split, keeping the focused editor (an editor pane is flushed
    /// first so its edits aren't lost; a surface is read-only).
    fn close_split(&mut self) {
        if self.split.is_some() {
            self.close_split_if_any();
            self.message = Some("Closed the split.".into());
        }
    }

    /// Drop the split, flushing an editor pane. Quiet (no message) — for callers
    /// that take over the editor region, like the continuous view.
    pub(super) fn close_split_if_any(&mut self) {
        if let Some(split) = self.split.take() {
            if let OtherPane::Editor(mut other) = split.other {
                if other.dirty {
                    let _ = other.save(); // best effort; the focused pane is unaffected
                }
            }
        }
    }

    /// Resize the split (#166): move the boundary so the focused pane grows or
    /// shrinks, clamped so the other pane keeps at least `MIN_RATIO`. A no-op when
    /// not split. Which physical side is focused depends on the pane kind — a
    /// docked surface taking scroll focus is the *second* pane.
    fn resize_split(&mut self, grow: bool) {
        let Some(split) = self.split.as_mut() else { return };
        // A scroll-focused surface is the *second* pane; otherwise the focused
        // editor is the first pane when `focused_first`.
        let focused_is_first = match &split.other {
            OtherPane::Surface { focused: true, .. } => !split.focused_first,
            _ => split.focused_first,
        };
        // Grow the focused pane → enlarge whichever side it's on (XNOR). The
        // `.clamp` bounds both ends, so plain signed arithmetic is safe.
        let step = if grow == focused_is_first { RESIZE_STEP as i32 } else { -(RESIZE_STEP as i32) };
        let next = split.first_ratio as i32 + step;
        split.first_ratio = next.clamp(MIN_RATIO as i32, 100 - MIN_RATIO as i32) as u16;
    }

    /// Whether a docked surface currently has scroll focus (#164) — keys go to it.
    pub(crate) fn surface_pane_focused(&self) -> bool {
        matches!(&self.split, Some(Split { other: OtherPane::Surface { focused: true, .. }, .. }))
    }

    /// What the renderer needs to lay out the split (#163/#164). `None` when not split.
    pub(crate) fn split_view(&self) -> Option<SplitView<'_>> {
        let s = self.split.as_ref()?;
        let content = match &s.other {
            OtherPane::Editor(other) => SplitContent::Editor(other),
            OtherPane::Surface { overlay, .. } => SplitContent::Surface(overlay),
        };
        Some(SplitView { dir: s.dir, first_ratio: s.first_ratio, focused_first: s.focused_first, content })
    }

    /// Refresh the cached word count for the *focused* buffer's own chapter
    /// (#165). A reference pane on a bible file or note has no manifest entry, so
    /// caching the active chapter's count from it would corrupt the cache — this
    /// is a no-op unless the focused buffer is itself a chapter. For the ordinary
    /// single-chapter case the focused buffer is the active chapter, so the
    /// cached count is refreshed exactly as before.
    pub(super) fn cache_focus_words(&mut self) {
        if let Some(i) = self.chapter_index_of(&self.buffer) {
            self.project.set_chapter_words(i, self.buffer.word_count());
        }
    }

    /// The manifest index of the chapter a pane's `buffer` is showing.
    fn chapter_index_of(&self, buffer: &Buffer) -> Option<usize> {
        let file = buffer.path.as_ref()?.file_name()?.to_str()?;
        self.project.index_of_file(file)
    }

    /// The title bar for a reference pane (#163/#165): the chapter title if the
    /// pane shows a chapter, otherwise the file's stem (e.g. a bible entry's name),
    /// falling back to "reference" for an unnamed buffer.
    pub(crate) fn pane_title_for(&self, buffer: &Buffer) -> String {
        if let Some(title) = self.chapter_index_of(buffer).and_then(|i| self.project.chapter_title(i)) {
            return title.to_string();
        }
        buffer
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("reference")
            .to_string()
    }

    /// Point the project's active chapter at the focused buffer's file, so the
    /// active-chapter machinery (save, Ctrl-n/p, the outline marker) follows focus.
    fn sync_active_to_focus(&mut self) {
        if let Some(i) = self.chapter_index_of(&self.buffer) {
            self.project.set_active(i);
        }
    }

    /// After saving the focused pane, refresh a clean sibling editor showing the
    /// same chapter so it reflects the write (the common case right after a split).
    pub(super) fn refresh_split_sibling(&mut self) {
        let saved = self.buffer.path.clone();
        if let Some(Split { other: OtherPane::Editor(other), .. }) = &mut self.split {
            if !other.dirty && other.path == saved {
                if let Some(path) = saved {
                    if let Ok(reloaded) = Buffer::from_path(path) {
                        *other = reloaded;
                    }
                }
            }
        }
    }
}
