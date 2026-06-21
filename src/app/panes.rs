//! Window splits (#163): a second editor pane beside the first, each an
//! independent editor on a chapter. A first cut of the multi-pane epic (#110) —
//! two panes, Vim-style window commands.
//!
//! Asymmetric model: the focused pane is always `App::buffer` (so all existing
//! editing/navigation works unchanged), and the other pane's buffer is parked in
//! `App::split`. Focusing the other pane swaps the two buffers and re-points the
//! project's active chapter at whatever is now focused, so the rest of the app
//! (save, chapter nav, the outline) keeps treating `App::buffer` as "the chapter
//! you're working on".

use super::*;

/// How the two panes are arranged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SplitDir {
    /// Stacked top/bottom (Vim `:split`).
    Stacked,
    /// Side by side left/right (Vim `:vsplit`).
    SideBySide,
}

/// The second pane and how it sits relative to the focused one.
pub(crate) struct Split {
    /// The non-focused pane's editor.
    pub(crate) other: Buffer,
    pub(crate) dir: SplitDir,
    /// Whether `other` is the top/left pane (the focused `App::buffer` is then the
    /// bottom/right one). Flips on every focus swap so positions stay put.
    pub(crate) other_first: bool,
}

impl App {
    /// Handle the key after a `Ctrl-w` window prefix.
    pub(super) fn window_command(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('s') => self.open_split(SplitDir::Stacked),
            KeyCode::Char('v') => self.open_split(SplitDir::SideBySide),
            KeyCode::Char('w' | 'h' | 'j' | 'k' | 'l') => self.focus_other_pane(),
            KeyCode::Char('c' | 'q') => self.close_split(),
            _ => {} // unknown window command — cancel silently
        }
    }

    /// Split the editor into two panes (#163). Both start on the current chapter
    /// (independent buffers); navigate one elsewhere to compare. Only from the
    /// single-chapter editor — not the continuous view, and not when already split.
    fn open_split(&mut self, dir: SplitDir) {
        if self.split.is_some() {
            self.message = Some("Already split — Ctrl-w c to close a pane.".into());
            return;
        }
        if self.manuscript.is_some() {
            self.message = Some("Leave the continuous view (Ctrl-l) before splitting.".into());
            return;
        }
        // Flush so the new pane loads the same content the focused one shows.
        if self.flush_active().is_err() {
            return;
        }
        match Buffer::from_path(self.project.active_path()) {
            Ok(other) => {
                self.split = Some(Split { other, dir, other_first: false });
                self.message = Some("Split — Ctrl-w w to switch, Ctrl-w c to close.".into());
            }
            Err(e) => self.message = Some(format!("Split failed: {e}")),
        }
    }

    /// Move focus to the other pane: swap which buffer is `App::buffer`, then
    /// re-point the active chapter at it so save / nav follow focus.
    fn focus_other_pane(&mut self) {
        let Some(split) = &mut self.split else { return };
        std::mem::swap(&mut self.buffer, &mut split.other);
        split.other_first = !split.other_first;
        self.sync_active_to_focus();
    }

    /// Close the split, keeping the focused pane. The other pane is flushed first
    /// so its edits aren't lost.
    fn close_split(&mut self) {
        let Some(mut split) = self.split.take() else { return };
        if split.other.dirty {
            let _ = split.other.save(); // best effort; the focused pane is unaffected
        }
        self.message = Some("Closed the split.".into());
    }

    /// Drop the split (flushing the other pane) without a status message — for
    /// callers that take over the editor region, like the continuous view.
    pub(super) fn close_split_if_any(&mut self) {
        if let Some(mut split) = self.split.take() {
            if split.other.dirty {
                let _ = split.other.save();
            }
        }
    }


    /// What the renderer needs to lay out a split (#163): the direction, whether
    /// the focused pane (`App::buffer`) is the top/left one, and the other pane's
    /// buffer. `None` when not split.
    pub(crate) fn split_render(&self) -> Option<(SplitDir, bool, &Buffer)> {
        let s = self.split.as_ref()?;
        Some((s.dir, !s.other_first, &s.other))
    }

    /// The title of the chapter a pane's `buffer` is showing, for its title bar.
    pub(crate) fn chapter_title_for(&self, buffer: &Buffer) -> Option<&str> {
        let file = buffer.path.as_ref()?.file_name()?.to_str()?;
        let i = self.project.manifest.chapters.iter().position(|c| c.file == file)?;
        self.project.chapter_title(i)
    }

    /// Point the project's active chapter at the focused buffer's file, so the
    /// active-chapter machinery (save, Ctrl-n/p, the outline marker) follows focus.
    fn sync_active_to_focus(&mut self) {
        let file = self.buffer.path.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str());
        if let Some(file) = file {
            if let Some(i) = self.project.manifest.chapters.iter().position(|c| c.file == file) {
                self.project.set_active(i);
            }
        }
    }

    /// After saving the focused pane, refresh a clean sibling pane showing the same
    /// chapter so it reflects the write (the common case right after a split).
    pub(super) fn refresh_split_sibling(&mut self) {
        let saved = self.buffer.path.clone();
        if let Some(split) = &mut self.split {
            if !split.other.dirty && split.other.path == saved {
                if let Some(path) = saved {
                    if let Ok(reloaded) = Buffer::from_path(path) {
                        split.other = reloaded;
                    }
                }
            }
        }
    }
}
