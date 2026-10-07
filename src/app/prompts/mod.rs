//! The prompt / confirmation state machine: `on_key_prompt` and the helpers
//! that open, route, and close prompts. Extracted from `mod.rs` (#314).
//!
//! The single-key confirmations live in [`confirm`], the text prompts (shared
//! line editing + what each one does on Enter) in [`text`].
//!
//! The Vim Ex command engine (`:` commands, the go-to-line jump, `:%s///`) lives
//! in the [`ex`] child module; its entry points hang off `App` too, so callers
//! elsewhere in `app` are unaffected by the split.

mod confirm;
mod ex;
mod text;

use crossterm::event::KeyEvent;

use crate::mode::{ListFilter, Mode, Pending, PromptState};

use super::App;

impl App {
    /// Where an overlay (prompt or metadata editor) returns when it closes
    /// without acting: back to the chapter browser at `selected`, or to the
    /// editor's resting mode.
    pub(super) const fn closed_mode(&self, nav_return: Option<usize>) -> Mode {
        match nav_return {
            Some(selected) => Mode::Nav {
                selected,
                filter: ListFilter::new(),
            },
            None => self.editing_mode(),
        }
    }

    /// Save the active chapter if it has unsaved edits and refresh its cached
    /// word count. Returns `Err(())` (and sets a status message) if the save
    /// failed, so callers can abort before switching away from it.
    pub(super) fn flush_active(&mut self) -> Result<(), ()> {
        if self.buffer.dirty {
            if let Err(e) = self.buffer.save() {
                self.message = Some(format!("Save failed: {e}"));
                return Err(());
            }
        }
        // The chapter is now on disk; refresh its cached count.
        self.cache_focus_words();
        // Persist annotations in lockstep with the prose, like the real save path
        // `write_active` does (#302/#370) — but re-anchored first, so a flush on the
        // keystroke path (a chapter switch, or entering the continuous view) never
        // writes the pre-edit offset an idle re-anchor would later correct in memory
        // only, leaving the reload to overwrite it with the stale disk value (#11).
        // `reanchor_annotations` is a no-op when the idle tick already folded the
        // burst's edits in, so this never double-counts.
        self.reanchor_annotations();
        self.persist_annotations();
        Ok(())
    }

    /// Open a prompt. `nav_return` is the chapter to reselect when it closes
    /// (`None` returns to insert mode, e.g. the quit prompt).
    pub(super) fn open_prompt(
        &mut self,
        pending: Pending,
        label: impl Into<String>,
        prefill: &str,
        nav_return: Option<usize>,
    ) {
        self.mode = Mode::Prompt(PromptState {
            nav_return,
            ..PromptState::new(pending, label.into(), prefill.to_string())
        });
    }

    /// Enter (or stay in) the chapter browser with `selected` highlighted.
    /// Moving the selection within the browser preserves any active filter /
    /// quick-jump query (#410); entering from another mode starts unfiltered.
    pub(super) fn select(&mut self, selected: usize) {
        if let Mode::Nav { selected: s, .. } = &mut self.mode {
            *s = selected;
        } else {
            self.mode = Mode::Nav {
                selected,
                filter: ListFilter::new(),
            };
        }
    }

    /// The prompt state, while in [`Mode::Prompt`]. Only call from a prompt key
    /// handler, which has already matched the variant.
    fn prompt_mut(&mut self) -> &mut PromptState {
        match &mut self.mode {
            Mode::Prompt(state) => state,
            _ => unreachable!("prompt editing runs only in Prompt mode"),
        }
    }

    /// Route a key in [`Mode::Prompt`]: a confirmation answers on this key, a
    /// text prompt edits its line (and resolves on Enter).
    pub(super) fn on_key_prompt(&mut self, key: KeyEvent) {
        let Mode::Prompt(state) = &self.mode else {
            return;
        };
        let pending = state.pending;
        let nav_return = state.nav_return;
        if pending.is_confirm() {
            self.on_key_confirm(pending, key.code, nav_return);
        } else {
            self.on_key_text_prompt(pending, key, nav_return);
        }
    }
}
