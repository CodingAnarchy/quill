//! The findings the analysis passes produce (#827/#921): [`Finding`], one
//! triageable observation about the manuscript, and its [`FindingKind`] (which
//! pass it speaks for). Data only — the triage list and dashboard that show
//! them live in the editor; moved out of `mode` so the AI layer (and any other
//! front end) can carry findings without the terminal UI.

use super::AiAction;

/// The category / lens a [`Finding`] belongs to (#827) — which analysis pass it
/// speaks for, kept on the finding so the pass survives the map to a summary
/// string and a round-trip through the durable store (where `action` is lost).
/// The manuscript-health dashboard groups by this, and cross-pass dedup keys on
/// it, so two passes flagging the same span with the same text stay distinct.
///
/// [`Other`](Self::Other) is the catch-all for findings not tied to an analysis
/// pass — the "Ask the manuscript" citations and answer rows — and the fallback
/// for a slug the store can't resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FindingKind {
    Continuity,
    Setting,
    Plot,
    Theme,
    Scaffold,
    Naming,
    Style,
    Editorial,
    /// Cultural & historical accuracy findings (#973) — inaccuracy flags for an
    /// author-named concern. Accuracy, not sensitivity / representation.
    Accuracy,
    /// Foreshadow / payoff findings (#1047) — concrete seeded details (an object,
    /// a named threat, a promised meeting, a planted clue) set up then never paid
    /// off. Distinct from the plot pass's thematic-thread tracking.
    Foreshadow,
    /// Research-notebook fact-check findings (#1049) — passages that contradict a
    /// note the author wrote in `bible/research.md`. Grounded in the author's own
    /// ledger; not sensitivity / representation, and distinct from Accuracy.
    FactCheck,
    Voice,
    /// Echo / near-duplicate passage pairs (#920) — the semantic-repetition scan.
    Echo,
    /// Relationship-trajectory inconsistencies (#1046) — a chosen pair's arc
    /// contradicting itself or reversing off-page with no on-page setup.
    Relationship,
    /// Pacing & tension observations (#1048) — located, plain-language readings of
    /// the scene-level tension/quiet rhythm (a sagging middle, a run of high-tension
    /// chapters with no breath, a flat stretch). Observation only; no suggested change.
    Pacing,
    /// Not a re-runnable analysis pass (Q&A citations / answer rows), or an
    /// unrecognized persisted slug.
    Other,
}

impl FindingKind {
    /// Every kind, in a stable, meaningful sequence: the continuity/structure passes
    /// first, prose/voice next, and [`Other`](Self::Other) last (it collects the
    /// non-pass findings — Q&A citations — that rarely reach triage but shouldn't be
    /// dropped if they do). The single enumeration of the set: the dashboard's group
    /// order (#827), the health trend (#1052), and the revision-plan tie-break (#1055)
    /// all derive from it (`health.rs`), so a new kind joins them automatically rather
    /// than being silently dropped by a hand-maintained order list. [`from_slug`](Self::from_slug)
    /// folds over it too, so slug resolution can't drift from this list; [`slug`](Self::slug)
    /// and [`label`](Self::label) are independent exhaustive `match` arms (not folds
    /// over `ALL`), so the compiler forces them to cover every variant directly.
    pub(crate) const ALL: &'static [Self] = &[
        Self::Continuity,
        Self::Setting,
        Self::Plot,
        Self::Theme,
        Self::Scaffold,
        Self::Naming,
        Self::Style,
        Self::Editorial,
        Self::Accuracy,
        Self::Relationship,
        Self::Foreshadow,
        Self::FactCheck,
        Self::Pacing,
        Self::Voice,
        Self::Echo,
        Self::Other,
    ];

    /// The stable slug persisted in the durable store and matched back on load —
    /// never change one without a migration. Also the dashboard's group key.
    pub(crate) const fn slug(self) -> &'static str {
        match self {
            Self::Continuity => "continuity",
            Self::Setting => "setting",
            Self::Plot => "plot",
            Self::Theme => "theme",
            Self::Scaffold => "scaffold",
            Self::Naming => "naming",
            Self::Style => "style",
            Self::Editorial => "editorial",
            Self::Accuracy => "accuracy",
            Self::Foreshadow => "foreshadow",
            Self::FactCheck => "factcheck",
            Self::Voice => "voice",
            Self::Echo => "echo",
            Self::Relationship => "relationship",
            Self::Pacing => "pacing",
            Self::Other => "other",
        }
    }

    /// The human label shown in the dashboard's group headers.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Continuity => "Continuity",
            Self::Setting => "Setting",
            Self::Plot => "Plot & arc",
            Self::Theme => "Theme & motif",
            Self::Scaffold => "Scaffold / outline",
            Self::Naming => "Names & terms",
            Self::Style => "Prose & style",
            Self::Editorial => "Editorial",
            Self::Accuracy => "Cultural & historical accuracy",
            Self::Foreshadow => "Foreshadow & payoff",
            Self::FactCheck => "Research-notebook fact-check",
            Self::Voice => "Character voice",
            Self::Echo => "Echoes & near-duplicates",
            Self::Relationship => "Relationship trajectory",
            Self::Pacing => "Pacing & tension",
            Self::Other => "Other",
        }
    }

    /// Resolve a persisted slug back to a kind; an unknown slug (a future or
    /// corrupt value) falls back to [`Other`](Self::Other) rather than failing.
    /// Folds over [`ALL`](Self::ALL), so a new kind is resolvable the moment it
    /// joins the enum's ordering — no separate list to keep in sync.
    pub(crate) fn from_slug(slug: &str) -> Self {
        Self::ALL
            .iter()
            .copied()
            .find(|k| k.slug() == slug)
            .unwrap_or(Self::Other)
    }
}

/// One triageable finding (a continuity contradiction, style note, …). UI-shaped
/// — the AI layer maps its typed findings into this for display.
#[derive(Clone, Debug)]
pub struct Finding {
    /// One-line summary shown in the list (e.g. "object: the locket …").
    pub(crate) summary: String,
    /// The category / lens this finding belongs to (#827) — which pass it speaks
    /// for. Survives the map to a summary string and the durable-store round-trip
    /// (where `action` is lost), so the dashboard can group by it and cross-pass
    /// dedup can key on it.
    pub(crate) kind: FindingKind,
    /// Chapters involved; the first is the jump target.
    pub(crate) chapters: Vec<String>,
    /// The book the finding belongs to in a multi-book series (#341):
    /// `Some(index)` when a series-bible refresh produced it, so triage can
    /// attribute and jump across books (chapter titles can repeat between
    /// books). `None` targets the active book — every standalone-novel and
    /// active-book pass, unchanged.
    pub(crate) book: Option<usize>,
    /// The pass that produced this finding, so the author can re-check just this
    /// one after a fix by re-running its originating pass scoped to `chapters`
    /// (#608). `None` for findings not tied to a re-runnable pass — Q&A citations
    /// and history rows — and for findings restored from the durable store, which
    /// records content, not the pass (a fresh pass re-attributes them).
    pub(crate) action: Option<AiAction>,
    /// Other passes that independently raised the *same span* (#921): cross-pass
    /// consensus. `kind` is this finding's own pass; `corroborators` holds the
    /// additional passes that flagged the same chapters + summary, so the health
    /// view can weight a finding by agreement ("flagged by continuity + plot").
    /// Empty for a lone finding. Not part of identity — merging corroborators
    /// never changes which finding this *is* — so triage dedup and dismiss-matching
    /// stay keyed on content alone.
    pub(crate) corroborators: Vec<FindingKind>,
}

impl Finding {
    /// Every pass that agrees on this finding (#921): its own `kind` first, then
    /// the corroborating passes, de-duplicated and in first-seen order. One pass
    /// means no consensus; two or more is cross-pass agreement.
    pub(crate) fn agreement(&self) -> Vec<FindingKind> {
        let mut passes = vec![self.kind];
        for &k in &self.corroborators {
            if !passes.contains(&k) {
                passes.push(k);
            }
        }
        passes
    }

    /// Fold `other`'s passes into this finding's corroborators (#921) when a second
    /// pass raises the same span. Records `other`'s own kind and any consensus it
    /// already carried, skipping this finding's own kind and duplicates, so the
    /// agreement set stays a clean union.
    pub(crate) fn corroborate(&mut self, other: &Self) {
        for k in other.agreement() {
            if k != self.kind && !self.corroborators.contains(&k) {
                self.corroborators.push(k);
            }
        }
    }

    /// The cross-pass agreement signal (#921): `None` when a single pass raised
    /// this finding, or `Some("flagged by continuity + plot")` when passes agree —
    /// the consensus weight the findings list and health dashboard show. Passes are
    /// listed in agreement order (the primary first), lower-cased to read as a
    /// fragment. The single place the signal's wording lives, so the list, the
    /// dashboard, and its export stay identical.
    pub(crate) fn agreement_signal(&self) -> Option<String> {
        let passes = self.agreement();
        if passes.len() < 2 {
            return None;
        }
        let labels: Vec<String> = passes.iter().map(|k| k.label().to_lowercase()).collect();
        Some(format!("flagged by {}", labels.join(" + ")))
    }
}

/// Identity is the finding's *content* — kind, summary, chapters, and book — not
/// the originating `action` handle (#608). This mirrors the durable store's
/// content key ([`crate::app`]): a re-produced finding is the same finding, and a
/// finding restored from disk (which carries no `action`, but does carry its
/// persisted `kind`) still matches the one a fresh pass produces, so triage dedup
/// and dismiss-matching stay intact. `kind` sharpens cross-pass dedup (#827): two
/// passes flagging the same span with coincidentally identical text stay distinct.
/// `corroborators` (#921) is *not* part of identity — folding in an agreeing pass
/// weights a finding by consensus without changing which finding it is, so re-check
/// and dismiss-matching keep working on the merged finding.
impl PartialEq for Finding {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.summary == other.summary
            && self.chapters == other.chapters
            && self.book == other.book
    }
}
