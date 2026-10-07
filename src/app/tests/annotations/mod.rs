//! Inline author annotations (#302): a note anchored to a chapter byte range,
//! marked in the editor, re-anchored as the prose is edited, listed/navigable,
//! and kept out of the chapter `.md` (so exports stay clean).
//!
//! Split by feature area into submodules — [`lifecycle`] (creation, anchoring,
//! and the review list), [`threads`] (reply/resolve, #889), [`remap`] (the
//! batched re-anchor and its perf guard, #893), [`triage`] (multi-reviewer
//! sort/filter/jump, #933), [`motion`] (the `]a` / `[a` open-annotation motions,
//! #1009), [`reanchor`] (persists on the keystroke path), [`persist`] (when the
//! side file is written), and [`remove`] (removing notes from the editor). The helpers more
//! than one area uses live here; each submodule pulls them in with
//! `use super::*`.

use super::*;
use crate::mode::{AnnoShow, Mode};

mod lifecycle;
mod motion;
mod persist;
mod reanchor;
mod remap;
mod remove;
mod threads;
mod triage;

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// The review list's resolved/open filter state.
fn anno_show(app: &App) -> AnnoShow {
    match &app.mode {
        Mode::Annotations { show, .. } => *show,
        _ => panic!("expected the annotation list"),
    }
}
