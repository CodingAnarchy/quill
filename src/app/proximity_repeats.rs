//! The near-proximity word-repeat marking (#1002): a toggleable, revision-pass aid
//! that dim-marks a content word echoed twice within a short window of the visible
//! text ("she turned… he turned… the road turned") — the local repetition the
//! analysis report's manuscript-global "most-used words" can't surface.
//!
//! Purely lexical and offline (no embeddings, no AI): the spans come from
//! [`proximity_repeats`], computed on the idle tick over the edit buffer (the
//! visible ~3-chapter window at most) and cached ([`refresh_proximity_repeats`]),
//! never per keystroke or per draw — the renderer reads the cache. Quill never
//! edits the marked prose; the mark only surfaces the echo where it happens so the
//! author decides every change (the AI/analysis boundary). Off by default and
//! toggled deliberately, so it's a pass the author opts into, not always-on noise.

use std::sync::Arc;

use crate::analysis::{proximity_repeats, WordLists};

use super::App;

/// What the proximity scan depends on (#1002): the edit buffer's content revision
/// (process-unique, so it also names which buffer) and the word lists by identity
/// (see [`StyleKey`](super::style_flags::StyleKey)).
pub(super) struct ProximityKey {
    rev: u64,
    lists: Arc<WordLists>,
}

impl App {
    /// Recompute the near-proximity word repeats for the edit buffer (#1002), off
    /// the keystroke path. A no-op (and empty) when the marking is off, so it costs
    /// nothing until the author turns it on — and even then it's window-scoped, like
    /// the spell pass and the #1001 flag sweep. Runs on the idle tick; skips the
    /// rescan when neither the text nor the word lists changed since the last one.
    pub(crate) fn refresh_proximity_repeats(&mut self) {
        if !self.proximity_repeats_on {
            self.proximity_repeats.clear();
            self.proximity_key = None;
            return;
        }
        let rev = self.edit_buffer().revision();
        let lists = self.analysis_word_lists();
        if self
            .proximity_key
            .as_ref()
            .is_some_and(|k| k.rev == rev && Arc::ptr_eq(&k.lists, &lists))
        {
            return;
        }
        let text = self.edit_buffer().text();
        self.proximity_repeats = proximity_repeats(&text, &lists);
        self.proximity_key = Some(ProximityKey { rev, lists });
    }

    /// Toggle the proximity-repeat marking on or off (#1002). Turning it on computes
    /// the marks for the visible window straight away (so they appear without waiting
    /// for the next idle tick) and reports how many near-echoes are in view; turning
    /// it off clears the cache so the marks vanish on the next frame.
    pub(crate) fn toggle_proximity_repeats(&mut self) {
        self.proximity_repeats_on = !self.proximity_repeats_on;
        if self.proximity_repeats_on {
            self.refresh_proximity_repeats();
            let n = self.proximity_repeats.len();
            self.message = Some(if n == 0 {
                "Proximity repeats on — none in view.".into()
            } else {
                format!("Proximity repeats on — {n} marked in view.")
            });
        } else {
            self.proximity_repeats.clear();
            self.proximity_key = None;
            self.message = Some("Proximity repeats off.".into());
        }
    }
}
