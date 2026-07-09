//! Full-screen overlay state (#404) and the annotation-review display filters:
//! the read-only [`Overlay`] drawn above any mode, plus the [`AnnoShow`] /
//! [`AnnoSort`] cycles the [`Mode::Annotations`](super::Mode) list uses. Split out
//! of `mode.rs` to keep the core enums under the ~1,000-line convention; re-exported
//! from there, so callers (`crate::mode::{Overlay, …}`) are unaffected. Pure move.

use ratatui::text::Line;

/// Which annotations the review list shows (#889), cycled with `f`: every note,
/// only the open ones, or only the resolved ones. Applied on top of the `/` text
/// filter so an author can, e.g., work through only the still-open threads.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AnnoShow {
    /// Open and resolved alike.
    #[default]
    All,
    /// Only threads still open.
    Open,
    /// Only threads marked resolved.
    Resolved,
}

impl AnnoShow {
    /// The next state in the cycle (All → Open → Resolved → All).
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::All => Self::Open,
            Self::Open => Self::Resolved,
            Self::Resolved => Self::All,
        }
    }

    /// Whether an annotation with `resolved` state passes this filter.
    #[must_use]
    pub const fn admits(self, resolved: bool) -> bool {
        match self {
            Self::All => true,
            Self::Open => !resolved,
            Self::Resolved => resolved,
        }
    }

    /// The short label shown in the list help line.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Open => "open",
            Self::Resolved => "resolved",
        }
    }
}

/// How the annotation review list is ordered (#933), cycled with `s`. Position is
/// the historic default — notes in document order, the order they anchor in the
/// prose. Status groups the still-open threads ahead of the resolved ones (each
/// group kept in position order) so triage works top-down; reviewer groups a
/// multi-editor merge by who left each note (alphabetical, then position within a
/// reviewer). The sort is applied after the text/status/reviewer filters, over the
/// notes that pass, so it never changes which rows show — only their order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AnnoSort {
    /// Document order — by anchored byte offset. The historic default.
    #[default]
    Position,
    /// Open threads first, then resolved; position order within each group.
    Status,
    /// Grouped by reviewer (alphabetical); position order within a reviewer.
    Reviewer,
}

impl AnnoSort {
    /// The next state in the cycle (Position → Status → Reviewer → Position).
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Position => Self::Status,
            Self::Status => Self::Reviewer,
            Self::Reviewer => Self::Position,
        }
    }

    /// The short label shown in the list help line.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Position => "position",
            Self::Status => "status",
            Self::Reviewer => "reviewer",
        }
    }
}

/// A full-screen, read-only overlay drawn above any mode: the progress report,
/// the analysis report, or an AI answer. Holds its pre-rendered lines, how to
/// render itself, and the current scroll offset. `App` keeps a single
/// `Option<Overlay>`, so a new overlay is a new value here — not a new field
/// plus a close branch plus a draw arm.
pub struct Overlay {
    pub lines: Vec<Line<'static>>,
    pub kind: OverlayKind,
    /// First visible row; 0 when freshly opened.
    pub scroll: u16,
}

/// How an [`Overlay`] titles and renders itself.
pub enum OverlayKind {
    /// A bordered report box; the label follows the book title in the bar
    /// (e.g. "progress", "analysis"). Not word-wrapped — it holds aligned tables.
    Report(&'static str),
    /// The word-wrapped AI answer / summary panel.
    AiPanel,
    /// The built-in user's guide (#281) — a fixed help document.
    Guide,
    /// The first-run quick-start card (#837): a dismissible tour of the palette,
    /// the core keys, and the AI on-ramp. Shown once on first launch (then never,
    /// tracked by a config flag) and reachable on demand from help. Dismissing it
    /// records the flag, so the key handler needs to tell it apart from the other
    /// press-any-key surfaces.
    Onboarding,
    /// The pre-commit review (#704): the changed-file list and the diff of what a
    /// commit would capture. Unlike the read-only reports, this overlay confirms —
    /// Enter proceeds to the commit-message prompt, Esc (or any other key) cancels.
    CommitReview,
}

impl Overlay {
    /// A bordered report overlay labelled `label`, scrolled to the top.
    pub const fn report(label: &'static str, lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            kind: OverlayKind::Report(label),
            scroll: 0,
        }
    }

    /// The AI answer / summary overlay, scrolled to the top.
    pub const fn ai(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            kind: OverlayKind::AiPanel,
            scroll: 0,
        }
    }

    /// The user's-guide overlay (#281), scrolled to the top.
    pub const fn guide(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            kind: OverlayKind::Guide,
            scroll: 0,
        }
    }

    /// The pre-commit review overlay (#704), scrolled to the top.
    pub const fn commit_review(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            kind: OverlayKind::CommitReview,
            scroll: 0,
        }
    }

    /// The first-run quick-start card overlay (#837), scrolled to the top.
    pub const fn onboarding(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            kind: OverlayKind::Onboarding,
            scroll: 0,
        }
    }

    /// Whether this is the first-run onboarding card (#837), so dismissing it can
    /// record the shown-once config flag before closing.
    pub const fn is_onboarding(&self) -> bool {
        matches!(self.kind, OverlayKind::Onboarding)
    }

    /// Whether this overlay confirms an action (the pre-commit review, #704) rather
    /// than being a press-any-key-to-dismiss read-only surface — so the key handler
    /// knows Enter should proceed and any other key should cancel, not just close.
    pub const fn is_confirm(&self) -> bool {
        matches!(self.kind, OverlayKind::CommitReview)
    }

    /// The report label if this is a [`OverlayKind::Report`] overlay (e.g.
    /// "progress", "analysis") — the base name a saved snapshot (#601) is written
    /// under. `None` for the AI panel and the guide, which don't save to a file.
    pub const fn report_label(&self) -> Option<&'static str> {
        match self.kind {
            OverlayKind::Report(label) => Some(label),
            OverlayKind::AiPanel
            | OverlayKind::Guide
            | OverlayKind::Onboarding
            | OverlayKind::CommitReview => None,
        }
    }

    /// The overlay's rendered rows as plain text, one line per row — the styled
    /// spans flattened to their content. This is what a saved report snapshot
    /// (#601) writes, so the file matches what's shown on screen.
    pub fn plain_text(&self) -> String {
        self.lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
