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

use crate::text::count_matches;

/// How the two panes are arranged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitDir {
    /// Stacked top/bottom (Vim `:split`).
    Stacked,
    /// Side by side left/right (Vim `:vsplit`).
    SideBySide,
}

/// What the non-focused pane holds. Only ever one instance (in `App::split`), so
/// the variant size gap doesn't matter — no boxing.
#[allow(clippy::large_enum_variant)]
pub enum OtherPane {
    /// A second editor on a chapter (#163).
    Editor(Buffer),
    /// A docked read-only surface — report / analysis / AI answer (#164).
    /// `focused` is true while it's taking scroll input instead of the editor.
    Surface { overlay: Overlay, focused: bool },
    /// The authorial assistant pane (#307): its conversation and whether it has
    /// key focus. Like [`Editor`](Self::Editor) and [`Surface`](Self::Surface), the
    /// pane's state lives in the variant, so closing the split drops it.
    Assistant { conv: Assistant, focused: bool },
}

pub struct Split {
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
pub struct SplitView<'a> {
    pub(crate) dir: SplitDir,
    /// Percentage of the split axis given to the first (top/left) pane.
    pub(crate) first_ratio: u16,
    /// Whether the focused editor (`App::buffer`) is the first (top/left) pane.
    pub(crate) focused_first: bool,
    pub(crate) content: SplitContent<'a>,
}

/// What the non-focused pane shows the renderer (mirrors [`OtherPane`]).
pub enum SplitContent<'a> {
    /// A second editor pane (#163/#165).
    Editor(&'a Buffer),
    /// A docked read-only surface — report / analysis / AI answer (#164).
    Surface(&'a Overlay),
    /// The authorial assistant pane (#307): its conversation and whether focused.
    Assistant { conv: &'a Assistant, focused: bool },
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
            KeyCode::Char('a') => self.open_assistant(),
            KeyCode::Char('w' | 'h' | 'j' | 'k' | 'l') => self.focus_other_pane(),
            KeyCode::Char('c' | 'q') => self.close_split(),
            // Resize the split (#166): grow or shrink the focused pane along the
            // split axis. `+`/`>` grow, `-`/`<` shrink — both spellings either way.
            KeyCode::Char('+' | '>') => self.resize_split(true),
            KeyCode::Char('-' | '<') => self.resize_split(false),
            // Reset a resized split back to an even balance (#173), Vim's `Ctrl-w =`.
            KeyCode::Char('=') => self.equalize_split(),
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
    #[allow(clippy::needless_pass_by_value)] // callers (nav.rs) hand over an owned path
    pub(super) fn open_file_in_pane(&mut self, path: PathBuf) {
        self.place_editor_pane(
            &path,
            SplitDir::SideBySide,
            "Opened in a pane — Ctrl-w w to focus it, Ctrl-w c to close.",
        );
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
        let Some(overlay) = self.overlay.take() else {
            return;
        };
        if self.manuscript.is_some() {
            self.overlay = Some(overlay);
            self.message = Some("Leave the continuous view (Ctrl-l) to dock a report.".into());
            return;
        }
        self.close_split_if_any(); // two panes max — replace any existing split
        self.split = Some(Split {
            other: OtherPane::Surface {
                overlay,
                focused: false,
            },
            dir,
            focused_first: true,
            first_ratio: EVEN_SPLIT,
        });
        self.message = Some("Docked — Ctrl-w w to scroll it, Ctrl-w c to close.".into());
    }

    /// Open the authorial assistant pane (#307), docked beside the editor. Already
    /// open → just give it focus. Single-chapter view only, like the other panes.
    /// Reached from `Ctrl-W a` and the `Ctrl-K` run-action menu (#399).
    pub(super) fn open_assistant(&mut self) {
        if self.manuscript.is_some() {
            self.message =
                Some("Leave the continuous view (Ctrl-l) before opening the assistant.".into());
            return;
        }
        if let Some(Split {
            other: OtherPane::Assistant { focused, .. },
            ..
        }) = self.split.as_mut()
        {
            *focused = true; // already open — just give it focus
            self.message = Some("Assistant — type a request; Ctrl-w c closes it.".into());
            return;
        }
        if self.flush_active().is_err() {
            return;
        }
        self.close_split_if_any(); // two panes max — replace any existing split
        let mut conv = Assistant::new();
        // Open even with AI off so the pane can explain why it's unavailable (#307)
        // rather than failing obscurely; the first request reports the same.
        if let Err(e) = self.ai.ready_for(ai::BackendKind::Claude) {
            conv.note(&format!("The assistant is unavailable: {e}"));
        }
        self.split = Some(Split {
            other: OtherPane::Assistant {
                conv,
                focused: true,
            },
            dir: SplitDir::SideBySide,
            focused_first: true,
            first_ratio: EVEN_SPLIT,
        });
        self.message = Some("Assistant — type a request; Ctrl-w c closes it.".into());
    }

    /// Move focus to the other pane: swap editors (#163), hand scroll input to a
    /// docked surface (#164), or focus the assistant pane (#307). Reached from
    /// `Ctrl-w w` and a mouse click in the other pane (#163 follow-up).
    pub(super) fn focus_other_pane(&mut self) {
        let is_editor = matches!(
            self.split.as_ref(),
            Some(Split {
                other: OtherPane::Editor(_),
                ..
            })
        );
        if is_editor {
            self.swap_editor_panes();
        } else if let Some(s) = self.split.as_mut() {
            // A docked surface or the assistant: toggle which pane keys go to.
            match &mut s.other {
                OtherPane::Surface { focused, .. } | OtherPane::Assistant { focused, .. } => {
                    *focused = !*focused;
                }
                OtherPane::Editor(_) => {}
            }
        }
    }

    /// Swap which editor is focused (#163). Flush the leaving pane first, then —
    /// if the now-focused pane shows the same chapter — reload it from disk so two
    /// panes on one chapter accumulate edits rather than clobbering.
    fn swap_editor_panes(&mut self) {
        let _ = self.flush_active();
        let other_path = {
            let Some(split) = self.split.as_mut() else {
                return;
            };
            let OtherPane::Editor(other) = &mut split.other else {
                return;
            };
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
        let Some(Split {
            other: OtherPane::Surface { overlay, focused },
            ..
        }) = self.split.as_mut()
        else {
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
            self.close_focused_pane(false);
            self.message = Some("Closed the pane.".into());
        }
    }

    /// Close the currently focused pane, keeping the other — Vim's `:q`/`Ctrl-w c`
    /// (#163/#165). For two editors the *other* editor becomes the sole editor
    /// (the focused pane is flushed first, unless `discard`, so its edits persist);
    /// a read-only surface / the assistant can't be a sole view, so closing with one
    /// of those focused just drops it and keeps the editor. A no-op when not split.
    pub(super) fn close_focused_pane(&mut self, discard: bool) {
        let Some(split) = self.split.as_ref() else {
            return;
        };
        // A docked surface / assistant is never `App::buffer`, so "keep the other"
        // would mean keeping a non-editor as the sole view — instead keep the editor
        // and drop the surface/assistant, exactly as replacing a pane does.
        if !matches!(split.other, OtherPane::Editor(_)) {
            self.close_split_if_any();
            return;
        }
        // Two editors: discard the focused buffer (flushed so edits persist unless
        // `discard`) and promote the other editor to the sole focused pane.
        if !discard {
            let _ = self.flush_active();
        }
        let closed_path = self.buffer.path.clone();
        let Some(split) = self.split.take() else {
            return;
        };
        let OtherPane::Editor(other) = split.other else {
            return;
        };
        self.buffer = other;
        // If the closed pane was the project dictionary's edit pane (#935), re-read it
        // into the checker so a removed/fixed word takes effect right away.
        if closed_path.as_deref() == Some(self.project.dictionary_path().as_path()) {
            self.project.reload_dictionary();
            self.refresh_spell_flags();
        }
        self.sync_active_to_focus();
    }

    /// Drop the split, flushing an editor pane. Quiet (no message) — for callers
    /// that take over the editor region, like the continuous view.
    pub(super) fn close_split_if_any(&mut self) {
        if let Some(split) = self.split.take() {
            if let OtherPane::Editor(mut other) = split.other {
                if other.dirty {
                    let _ = other.save(); // best effort; the focused pane is unaffected
                }
                // If this was the project dictionary's own edit pane (#935), re-read
                // it into the checker so an edit made here — a removed or fixed word —
                // takes effect right away. Off the idle path: only on this close.
                if other.path.as_deref() == Some(self.project.dictionary_path().as_path()) {
                    self.project.reload_dictionary();
                    self.refresh_spell_flags();
                }
            }
            // A surface is read-only and the assistant's conversation lives in the
            // variant, so both drop with the split — nothing else to clean up.
        }
    }

    /// Resize the split (#166): move the boundary so the focused pane grows or
    /// shrinks, clamped so the other pane keeps at least `MIN_RATIO`. A no-op when
    /// not split. Which physical side is focused depends on the pane kind — a
    /// docked surface taking scroll focus is the *second* pane.
    fn resize_split(&mut self, grow: bool) {
        let Some(split) = self.split.as_mut() else {
            return;
        };
        // A scroll-focused surface is the *second* pane; otherwise the focused
        // editor is the first pane when `focused_first`.
        let focused_is_first = match &split.other {
            OtherPane::Surface { focused: true, .. }
            | OtherPane::Assistant { focused: true, .. } => !split.focused_first,
            _ => split.focused_first,
        };
        // Grow the focused pane → enlarge whichever side it's on (XNOR). The
        // `.clamp` bounds both ends, so plain signed arithmetic is safe.
        let step = if grow == focused_is_first {
            i32::from(RESIZE_STEP)
        } else {
            -i32::from(RESIZE_STEP)
        };
        let next = i32::from(split.first_ratio) + step;
        split.first_ratio = next.clamp(i32::from(MIN_RATIO), 100 - i32::from(MIN_RATIO)) as u16;
    }

    /// Reset the split to an even balance (#173). A no-op when not split or already
    /// even.
    const fn equalize_split(&mut self) {
        if let Some(split) = self.split.as_mut() {
            split.first_ratio = EVEN_SPLIT;
        }
    }

    /// Whether a docked surface currently has scroll focus (#164) — keys go to it.
    pub(crate) const fn surface_pane_focused(&self) -> bool {
        matches!(
            &self.split,
            Some(Split {
                other: OtherPane::Surface { focused: true, .. },
                ..
            })
        )
    }

    /// Whether the other pane is a second editor (#163/#165) rather than a docked
    /// surface / assistant — an editor pane is never "focused" in place (the focused
    /// editor is always `App::buffer`), so a click on it always swaps focus in.
    pub(super) const fn other_pane_is_editor(&self) -> bool {
        matches!(
            &self.split,
            Some(Split {
                other: OtherPane::Editor(_),
                ..
            })
        )
    }

    /// Whether the assistant pane currently has key focus (#307) — keys go to it.
    pub(crate) const fn assistant_pane_focused(&self) -> bool {
        matches!(
            &self.split,
            Some(Split {
                other: OtherPane::Assistant { focused: true, .. },
                ..
            })
        )
    }

    /// What the renderer needs to lay out the split (#163/#164). `None` when not split.
    pub(crate) fn split_view(&self) -> Option<SplitView<'_>> {
        let s = self.split.as_ref()?;
        let content = match &s.other {
            OtherPane::Editor(other) => SplitContent::Editor(other),
            OtherPane::Surface { overlay, .. } => SplitContent::Surface(overlay),
            OtherPane::Assistant { conv, focused } => SplitContent::Assistant {
                conv,
                focused: *focused,
            },
        };
        Some(SplitView {
            dir: s.dir,
            first_ratio: s.first_ratio,
            focused_first: s.focused_first,
            content,
        })
    }

    /// Refresh the cached word count and TODO count for the *focused* buffer's own
    /// chapter (#165, #867). A reference pane on a bible file or note has no manifest
    /// entry, so caching the active chapter's counts from it would corrupt the cache
    /// — this is a no-op unless the focused buffer is itself a chapter. For the
    /// ordinary single-chapter case the focused buffer is the active chapter, so the
    /// cached counts are refreshed exactly as before. Keeping the cached TODO count
    /// current here is what lets the save-path roll-up ([`Self::count_todo`]) stay
    /// off the per-chapter disk reads.
    pub(super) fn cache_focus_words(&mut self) {
        if let Some(i) = self.chapter_index_of(&self.buffer) {
            let todo = count_matches(&self.buffer.text(), self.project.manifest.todo_marker());
            self.project.set_chapter_words(i, self.buffer.word_count());
            self.project.set_chapter_todo(i, todo);
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
        if let Some(title) = self
            .chapter_index_of(buffer)
            .and_then(|i| self.project.chapter_title(i))
        {
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
        if let Some(Split {
            other: OtherPane::Editor(other),
            ..
        }) = &mut self.split
        {
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
