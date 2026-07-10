//! The AI actions reachable from the AI palette: the [`AiAction`] enum, its cost
//! profile ([`AiCost`]), and the [`AiActionSpec`] descriptor table (`SPECS`) that
//! is the single source for each action's slug, cost, and story-bible key (#563).
//! Split out of `action.rs` to keep each file under the ~1,000-line convention;
//! re-exported from the module root, so callers are unaffected. Pure move.

/// The AI actions reachable from the AI palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiAction {
    Ask,
    Characters,
    Continuity,
    Setting,
    /// Plot-thread / arc tracker — subplot setup→payoff status + dangling-thread
    /// findings, a fourth persistent bible pass (#606).
    Plot,
    /// Theme / motif tracker — motif → where it recurs / status + dangling-motif
    /// findings, a fifth persistent bible pass (#828).
    Theme,
    Scaffold,
    Naming,
    Style,
    /// Brainstorm story ideas from a premise — generative, off-manuscript (#255).
    Brainstorm,
    /// Draft a starter character cast from a concept — generative (#256).
    DraftCharacters,
    /// Draft a starter setting bible from a concept — generative (#257).
    DraftSetting,
    /// Character voice consistency — dialogue & POV/narration (#183).
    Voice,
    Submission,
    /// Marketing copy — blurb variants & taglines, off-manuscript (#236).
    Marketing,
    /// Social posts — per-platform promo copy, off-manuscript (#237).
    Social,
    /// Platform-ready ad creative & copy, off-manuscript (#251).
    AdCreative,
    /// Character portrait images from the character bible (#238).
    Portrait,
    /// A key-scene illustration of the active chapter (#239).
    Scene,
    /// Open the accumulated revision-triage findings (local; no request).
    Triage,
    /// Open the manuscript-health dashboard (#827): open findings grouped by
    /// kind/pass and chapter with counts, jumpable (local; no request).
    Dashboard,
    /// Open the motif / character co-occurrence report (#922): co-occurring pairs
    /// and notable absences drawn from the character and theme bibles' chapter-
    /// appearance data, jumpable (local; no request).
    CoOccurrence,
    /// Browse the session's "Ask the manuscript" history — past asks, answers, and
    /// their jumpable citations (local; no request) (#609).
    AskHistory,
    /// Semantic "find similar scenes" (#924): from a selected passage or a typed
    /// query, retrieve the top-N most similar passages elsewhere in the manuscript,
    /// ranked with a similarity score and a jumpable citation. Reads + cites; local
    /// (the on-device embedding index), no network request.
    SemanticFind,
    /// Echo / near-duplicate detection (#920): scan the embedding index for
    /// near-duplicate and high-similarity passage pairs and surface them as
    /// findings, each with both citations + a score. Reads + flags; local, no
    /// network request.
    Echo,
    /// In-context thesaurus / word search for a word or phrase (#290).
    Thesaurus,
    /// Name generator — candidate character / place / term names by constraints,
    /// an option-list lookup the author picks from (#610).
    NameGen,
    /// Editorial read — flag structural / continuity / line issues, no fixes (#293).
    Editorial,
    /// Cultural & historical accuracy — author-invoked, per a named concern (#973):
    /// flag passages that read as factually / period / culturally INACCURATE for the
    /// concern the author names (e.g. "1850s London"). Findings only; author-invoked;
    /// accuracy, NOT sensitivity / representation review.
    Accuracy,
    /// Relationship trajectory — author-invoked, per a chosen character pair (#1046):
    /// track the pair's relationship state per chapter (allied → estranged →
    /// reconciled) and flag inconsistencies — a state that contradicts an earlier
    /// established one, or a reversal that happens off-page with no on-page setup.
    /// Findings only; author-invoked; observes the drift, never rewrites the arc.
    Relationship,
}

/// What a network pass costs (#82/#372) — the *data* side of the cost estimate,
/// kept provider-agnostic here so the dispatch ([`crate::app::App::cost_profile`])
/// resolves it to a concrete `(model, max_tokens)` against live settings. `None`
/// is for the local / interactive / image actions that issue no costed
/// whole-manuscript pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiCost {
    /// No costed whole-manuscript pass (interactive, local, or per-image).
    None,
    /// The cheap name/term-consistency pass, billed with its own fixed model.
    Naming,
    /// A standard manuscript pass on the configured model + default output bound.
    Manuscript,
}

/// The reasoning weight of a manuscript pass (#930) — the hint the router uses to
/// pick *which* cloud model runs it, once routing has already decided the cloud
/// backend. A heavy pass (continuity, characterization) wants the author's top
/// configured model (Opus by default); a light, high-frequency scan (naming,
/// style flags) can drop to a cheaper/faster tier when the estimate is small,
/// saving cost without downgrading the hard passes. It never changes *what* a pass
/// produces — only which model runs it. The author's `[ai] model` override always
/// wins for heavy passes (it *is* the heavy tier); light passes step down from it.
/// Passes with no costed manuscript request ([`AiCost::None`]/[`AiCost::Naming`])
/// don't carry a weight — naming already pins its own fixed cheap model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelWeight {
    /// Heavy reasoning — always the author's configured cloud model (the top tier).
    Heavy,
    /// Light, high-frequency analysis — may drop to a cheaper/faster tier when the
    /// pass is small enough that the top model's quality isn't needed.
    Light,
}

/// The single source of truth for an [`AiAction`]'s parallel data (#563): its
/// stable slug, its cost profile, and — for the three persistent-bible passes —
/// the on-disk bible key. The palette registry, [`Action::name`](super::Action::name),
/// the cost gate, and the bible trackers all derive from this table rather than
/// re-encoding the set, so a new or renamed action is described in exactly one
/// place.
pub struct AiActionSpec {
    pub(crate) action: AiAction,
    /// The stable token the run-action registry and workflows reference — the AI
    /// half of [`Action::name`](super::Action::name). Never change without a config
    /// migration.
    pub(crate) slug: &'static str,
    pub(crate) cost: AiCost,
    /// The reasoning weight the router uses to size *which* cloud model runs this
    /// pass (#930) — [`Heavy`](ModelWeight::Heavy) for the top model,
    /// [`Light`](ModelWeight::Light) for a cheaper tier when the pass is small.
    /// Only meaningful for the [`AiCost::Manuscript`] passes; the non-costed and
    /// naming passes carry [`Heavy`](ModelWeight::Heavy) as an inert default (they
    /// never reach the per-action cloud picker).
    pub(crate) weight: ModelWeight,
    /// The story-bible key this pass tracks (the incremental cache key and the
    /// series-rollup label), or `None` for the non-bible passes.
    pub(crate) bible_key: Option<&'static str>,
}

impl AiAction {
    /// The descriptor table — every [`AiAction`] with its slug, cost profile, and
    /// bible key, in palette order. The single place the action set is enumerated.
    pub(crate) const SPECS: &'static [AiActionSpec] = &[
        AiActionSpec {
            action: Self::Ask,
            slug: "ai:ask",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Characters,
            slug: "ai:characters",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Heavy,
            bible_key: Some("characters"),
        },
        AiActionSpec {
            action: Self::Continuity,
            slug: "ai:continuity",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Heavy,
            bible_key: Some("timeline"),
        },
        AiActionSpec {
            action: Self::Setting,
            slug: "ai:setting",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Heavy,
            bible_key: Some("setting"),
        },
        AiActionSpec {
            action: Self::Plot,
            slug: "ai:plot",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Heavy,
            bible_key: Some("plot"),
        },
        AiActionSpec {
            action: Self::Theme,
            slug: "ai:theme",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Heavy,
            bible_key: Some("theme"),
        },
        AiActionSpec {
            action: Self::Scaffold,
            slug: "ai:scaffold",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Naming,
            slug: "ai:naming",
            cost: AiCost::Naming,
            weight: ModelWeight::Light,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Style,
            slug: "ai:style",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Light,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Brainstorm,
            slug: "ai:brainstorm",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::DraftCharacters,
            slug: "ai:draft-characters",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::DraftSetting,
            slug: "ai:draft-setting",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Voice,
            slug: "ai:voice",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Submission,
            slug: "ai:submission",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Marketing,
            slug: "ai:marketing",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Light,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Social,
            slug: "ai:social",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Light,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::AdCreative,
            slug: "ai:ad-creative",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Light,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Portrait,
            slug: "ai:portrait",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Scene,
            slug: "ai:scene",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Triage,
            slug: "triage",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Dashboard,
            slug: "health",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::CoOccurrence,
            slug: "cooccurrence",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::AskHistory,
            slug: "ai:ask-history",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::SemanticFind,
            slug: "ai:find-similar",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Echo,
            slug: "ai:echo",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Thesaurus,
            slug: "ai:thesaurus",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::NameGen,
            slug: "ai:name",
            cost: AiCost::None,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Editorial,
            slug: "ai:editorial",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Accuracy,
            slug: "ai:accuracy",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
        AiActionSpec {
            action: Self::Relationship,
            slug: "ai:relationship",
            cost: AiCost::Manuscript,
            weight: ModelWeight::Heavy,
            bible_key: None,
        },
    ];

    /// This action's descriptor — its single source of slug / cost / bible key.
    pub(crate) fn spec(self) -> &'static AiActionSpec {
        #[allow(
            clippy::expect_used,
            reason = "SPECS is the single hand-maintained table listing every AiAction \
                      variant, so the lookup for `self` always resolves"
        )]
        Self::SPECS
            .iter()
            .find(|s| s.action == self)
            .expect("every AiAction has a descriptor in AiAction::SPECS")
    }

    /// The stable run-action slug for this AI action.
    pub(crate) fn slug(self) -> &'static str {
        self.spec().slug
    }

    /// Resolve an AI action from its stable [`slug`](Self::slug) — the inverse used
    /// to rehydrate a durable pass queue (#923) from the slugs it stores.
    pub(crate) fn from_slug(slug: &str) -> Option<Self> {
        Self::SPECS
            .iter()
            .find(|s| s.slug == slug)
            .map(|s| s.action)
    }

    /// This pass's cost profile — the data the cost gate resolves against live AI
    /// settings.
    pub(crate) fn cost(self) -> AiCost {
        self.spec().cost
    }

    /// This pass's reasoning weight (#930) — the hint the router uses to size which
    /// cloud model runs it (heavy → the configured top model, light → a cheaper
    /// tier when the pass is small). See [`ModelWeight`].
    pub(crate) fn weight(self) -> ModelWeight {
        self.spec().weight
    }

    /// The on-disk story-bible key this pass tracks, or `None` for non-bible
    /// passes. The one place the `Characters→characters` / `Continuity→timeline` /
    /// `Setting→setting` mapping is written; both the incremental cache (#90,
    /// `worker.rs`) and the series-rollup label (#340, `series_bible.rs`) read it.
    pub(crate) fn bible_key(self) -> Option<&'static str> {
        self.spec().bible_key
    }

    /// The [`FindingKind`](crate::mode::FindingKind) a pass's findings carry (#827)
    /// — the category that survives the map to a summary string and the durable
    /// store, so the health dashboard can group by pass. Only the passes that
    /// surface findings have a distinct kind; every other action maps to
    /// [`Other`](crate::mode::FindingKind::Other) (they produce no triage findings).
    pub(crate) const fn finding_kind(self) -> crate::mode::FindingKind {
        use crate::mode::FindingKind;
        match self {
            Self::Continuity => FindingKind::Continuity,
            Self::Setting => FindingKind::Setting,
            Self::Plot => FindingKind::Plot,
            Self::Theme => FindingKind::Theme,
            Self::Scaffold => FindingKind::Scaffold,
            Self::Naming => FindingKind::Naming,
            Self::Style => FindingKind::Style,
            Self::Editorial => FindingKind::Editorial,
            Self::Accuracy => FindingKind::Accuracy,
            Self::Relationship => FindingKind::Relationship,
            Self::Voice => FindingKind::Voice,
            Self::Echo => FindingKind::Echo,
            _ => FindingKind::Other,
        }
    }
}
