//! The AI actions reachable from the AI palette: the [`AiAction`] enum, its cost
//! profile ([`AiCost`]), and the [`AiActionSpec`] descriptor table (`SPECS`) that
//! is the single source for each action's slug, cost, bible key, finding kind,
//! labels, assistant-runnability, and free-text argument (#563/#1069). Split out of
//! `action.rs` to keep each file under the ~1,000-line convention; re-exported from
//! the module root, so callers are unaffected.

use crate::mode::FindingKind;

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
    /// Foreshadow / payoff scan — author-invoked (#1047): pair a concrete seeded
    /// detail (an object, a named threat, a promised meeting, a planted clue) with
    /// its resolution and flag the UNPAID ones, naming the setup passage + chapter.
    /// Findings only; author-invoked. Narrower than — and distinct from — the plot
    /// pass's thematic-thread tracking; it never drafts the payoff.
    Foreshadow,
    /// Research-notebook fact-check — author-invoked (#1049): flag manuscript
    /// passages that CONTRADICT a note the author wrote in `bible/research.md`
    /// (e.g. a date, name, or rule that disagrees with a note). Findings only;
    /// grounded solely in the author's own notes (no external facts); NOT a
    /// sensitivity review, and distinct from Accuracy (which checks a named
    /// *external* concern — this checks the author's own ledger).
    FactCheck,
    /// Pacing & tension reading — author-invoked (#1048): read the manuscript's
    /// scene-level tension/quiet RHYTHM (a sagging middle, a run of high-tension
    /// chapters with no breath, a flat stretch) and report it as located, plain-
    /// language observations, each naming the stretch (chapters/scenes). Findings
    /// only; author-invoked; observes rhythm, never suggests a cut, an addition, a
    /// reorder, or reworded prose — it sits closest to the co-writer line.
    Pacing,
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

/// The single source of truth for an [`AiAction`]'s parallel data (#563): the
/// hand-listed facts that used to live in seven independent, drift-prone tables —
/// the slug, cost profile, model weight, bible key, finding kind, run-action + palette
/// labels, the assistant's runnability (#1069) and its free-text argument. Every
/// derivable table ([`finding_kind`](AiAction::finding_kind),
/// [`Action::label`](super::Action::label), the AI palette entry,
/// [`Action::assistant_runnable`](super::Action::assistant_runnable),
/// [`Action::text_arg`](super::Action::text_arg), the cost gate, the bible trackers)
/// derives from this row rather than re-encoding the set, so a new or renamed action is
/// described in exactly one place and can't silently omit a variant. The per-pass
/// *content* — the system prompt and schema field names — deliberately stays in each
/// `ai::<pass>` module; this table is the mechanism, not the analysis.
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
    /// The [`FindingKind`](crate::mode::FindingKind) a pass's findings carry (#827),
    /// or [`Other`](crate::mode::FindingKind::Other) for the actions that surface no
    /// triage findings — the single place the pass→kind mapping is written, so it
    /// can't drift from the health dashboard's grouping.
    pub(crate) finding_kind: crate::mode::FindingKind,
    /// The full run-action label shown in the unified menu (`"AI · Character bible"`),
    /// the AI half of [`Action::label`](super::Action::label).
    pub(crate) label: &'static str,
    /// The AI palette's short label + one-line blurb (the compact entry the AI
    /// palette lists), or `None` for the actions that never appear there directly.
    /// Every current AI action carries one, so this is `Some` throughout today.
    pub(crate) palette: Option<(&'static str, &'static str)>,
    /// Whether the authorial assistant (#307/#392) may run this action as a tool —
    /// the descriptor field that CLOSES the hand-maintained allowlist (#1069). The
    /// analysis passes that surface findings (no prose) and the arg-taking generative
    /// helpers are runnable; the thesaurus/image passes, the local views, and the
    /// off-manuscript copy generators are not.
    pub(crate) assistant_runnable: bool,
    /// The JSON property name of this action's single free-text argument (#390) for
    /// the actions that take one (the question for Ask, the concern for Accuracy, …),
    /// or `None`. The single source the assistant's tool schema and its dispatch read.
    pub(crate) text_arg: Option<&'static str>,
}

/// Shorthand for a finding-surfacing pass row: manuscript-costed, its finding kind,
/// runnable by the assistant. Keeps the SPECS table readable — the analysis passes
/// share every field except slug/weight/label/palette/kind/arg.
const fn findings_spec(
    action: AiAction,
    slug: &'static str,
    weight: ModelWeight,
    finding_kind: FindingKind,
    label: &'static str,
    palette: (&'static str, &'static str),
    text_arg: Option<&'static str>,
) -> AiActionSpec {
    AiActionSpec {
        action,
        slug,
        cost: AiCost::Manuscript,
        weight,
        bible_key: None,
        finding_kind,
        label,
        palette: Some(palette),
        assistant_runnable: true,
        text_arg,
    }
}

/// A non-findings AI action row (Ask, the generative helpers, the local views, the
/// off-manuscript copy) — no finding kind, not assistant-runnable and taking no
/// argument by default. The handful that ARE runnable / arg-taking (Ask and the
/// generative helpers) set `assistant_runnable`/`text_arg` with a struct update at the
/// call site. Bible passes and findings passes have their own constructors.
const fn plain_spec(
    action: AiAction,
    slug: &'static str,
    cost: AiCost,
    weight: ModelWeight,
    label: &'static str,
    palette: Option<(&'static str, &'static str)>,
) -> AiActionSpec {
    AiActionSpec {
        action,
        slug,
        cost,
        weight,
        bible_key: None,
        finding_kind: FindingKind::Other,
        label,
        palette,
        assistant_runnable: false,
        text_arg: None,
    }
}

impl AiAction {
    /// The descriptor table — every [`AiAction`] with its full row, in palette order.
    /// The single place the action set is enumerated: slug, cost, weight, bible key,
    /// finding kind, run-action + palette labels, assistant-runnability (#1069), and
    /// the free-text argument. Everything derivable reads from here.
    pub(crate) const SPECS: &'static [AiActionSpec] = &[
        AiActionSpec {
            assistant_runnable: true,
            text_arg: Some("question"),
            ..plain_spec(
                Self::Ask,
                "ai:ask",
                AiCost::None,
                ModelWeight::Heavy,
                "AI · Ask the manuscript",
                Some((
                    "Ask the manuscript",
                    "Answer a question grounded in the draft",
                )),
            )
        },
        AiActionSpec {
            bible_key: Some("characters"),
            ..findings_spec(
                Self::Characters,
                "ai:characters",
                ModelWeight::Heavy,
                FindingKind::Other, // writes profiles, not triage findings
                "AI · Character bible",
                ("Character bible", "Profiles, arcs, and drift flags"),
                None,
            )
        },
        AiActionSpec {
            bible_key: Some("timeline"),
            ..findings_spec(
                Self::Continuity,
                "ai:continuity",
                ModelWeight::Heavy,
                FindingKind::Continuity,
                "AI · Timeline & continuity",
                (
                    "Timeline & continuity",
                    "Ordered timeline + contradiction findings",
                ),
                None,
            )
        },
        AiActionSpec {
            bible_key: Some("setting"),
            ..findings_spec(
                Self::Setting,
                "ai:setting",
                ModelWeight::Heavy,
                FindingKind::Setting,
                "AI · Setting & worldbuilding",
                (
                    "Setting & worldbuilding",
                    "Setting bible + rule-violation findings",
                ),
                None,
            )
        },
        AiActionSpec {
            bible_key: Some("plot"),
            ..findings_spec(
                Self::Plot,
                "ai:plot",
                ModelWeight::Heavy,
                FindingKind::Plot,
                "AI · Plot & arc tracker",
                (
                    "Plot & arc tracker",
                    "Thread setup→payoff status + dangling-thread findings",
                ),
                None,
            )
        },
        AiActionSpec {
            bible_key: Some("theme"),
            ..findings_spec(
                Self::Theme,
                "ai:theme",
                ModelWeight::Heavy,
                FindingKind::Theme,
                "AI · Theme & motif tracker",
                (
                    "Theme & motif tracker",
                    "Motif → where it recurs / status + dangling-motif findings",
                ),
                None,
            )
        },
        findings_spec(
            Self::Scaffold,
            "ai:scaffold",
            ModelWeight::Heavy,
            FindingKind::Scaffold,
            "AI · Scaffold / outline",
            ("Scaffold / outline", "Beat outline + structural gaps"),
            None,
        ),
        AiActionSpec {
            cost: AiCost::Naming,
            ..findings_spec(
                Self::Naming,
                "ai:naming",
                ModelWeight::Light,
                FindingKind::Naming,
                "AI · Name & term consistency",
                ("Name & term consistency", "Spelling variants (cheap pass)"),
                None,
            )
        },
        findings_spec(
            Self::Style,
            "ai:style",
            ModelWeight::Light,
            FindingKind::Style,
            "AI · Prose & style",
            (
                "Prose & style",
                "POV / tense / voice findings (analysis only)",
            ),
            None,
        ),
        AiActionSpec {
            assistant_runnable: true,
            text_arg: Some("premise"),
            ..plain_spec(
                Self::Brainstorm,
                "ai:brainstorm",
                AiCost::None,
                ModelWeight::Heavy,
                "AI · Brainstorm ideas",
                Some((
                    "Brainstorm ideas",
                    "Premises, themes & conflicts from a spark (off-manuscript)",
                )),
            )
        },
        AiActionSpec {
            assistant_runnable: true,
            text_arg: Some("concept"),
            ..plain_spec(
                Self::DraftCharacters,
                "ai:draft-characters",
                AiCost::None,
                ModelWeight::Heavy,
                "AI · Draft characters",
                Some((
                    "Draft characters",
                    "Starter cast from a concept (no manuscript needed)",
                )),
            )
        },
        AiActionSpec {
            assistant_runnable: true,
            text_arg: Some("concept"),
            ..plain_spec(
                Self::DraftSetting,
                "ai:draft-setting",
                AiCost::None,
                ModelWeight::Heavy,
                "AI · Draft setting",
                Some((
                    "Draft setting",
                    "Starter worldbuilding bible from a concept",
                )),
            )
        },
        findings_spec(
            Self::Voice,
            "ai:voice",
            ModelWeight::Heavy,
            FindingKind::Voice,
            "AI · Character voice",
            (
                "Character voice",
                "Dialogue & POV voice vs. the character bible",
            ),
            None,
        ),
        plain_spec(
            Self::Submission,
            "ai:submission",
            AiCost::Manuscript,
            ModelWeight::Heavy,
            "AI · Submission material",
            Some((
                "Submission material",
                "Synopsis, blurb & query letter (off-manuscript)",
            )),
        ),
        plain_spec(
            Self::Marketing,
            "ai:marketing",
            AiCost::Manuscript,
            ModelWeight::Light,
            "AI · Marketing copy",
            Some((
                "Marketing copy",
                "Blurb variants & taglines (off-manuscript)",
            )),
        ),
        plain_spec(
            Self::Social,
            "ai:social",
            AiCost::Manuscript,
            ModelWeight::Light,
            "AI · Social posts",
            Some((
                "Social posts",
                "X / Facebook / Instagram copy (off-manuscript)",
            )),
        ),
        plain_spec(
            Self::AdCreative,
            "ai:ad-creative",
            AiCost::Manuscript,
            ModelWeight::Light,
            "AI · Ad creative",
            Some((
                "Ad creative",
                "Amazon / Meta / X ad copy & targeting (off-manuscript)",
            )),
        ),
        plain_spec(
            Self::Portrait,
            "ai:portrait",
            AiCost::None,
            ModelWeight::Heavy,
            "AI · Character portraits",
            Some((
                "Character portraits",
                "Portrait images from the character bible (fal.ai)",
            )),
        ),
        plain_spec(
            Self::Scene,
            "ai:scene",
            AiCost::None,
            ModelWeight::Heavy,
            "AI · Scene illustration",
            Some((
                "Scene illustration",
                "Illustrate the active chapter (fal.ai)",
            )),
        ),
        plain_spec(
            Self::Triage,
            "triage",
            AiCost::None,
            ModelWeight::Heavy,
            "Revision triage",
            Some((
                "Revision triage",
                "All findings so far, in one list (local)",
            )),
        ),
        plain_spec(
            Self::Dashboard,
            "health",
            AiCost::None,
            ModelWeight::Heavy,
            "Manuscript health dashboard",
            Some((
                "Manuscript health",
                "Open findings by kind & chapter, with counts (local)",
            )),
        ),
        plain_spec(
            Self::CoOccurrence,
            "cooccurrence",
            AiCost::None,
            ModelWeight::Heavy,
            "Motif & character co-occurrence",
            Some((
                "Motif & character co-occurrence",
                "Which entries cluster & which are conspicuously apart (local)",
            )),
        ),
        plain_spec(
            Self::AskHistory,
            "ai:ask-history",
            AiCost::None,
            ModelWeight::Heavy,
            "AI · Ask history",
            Some((
                "Ask history",
                "Re-open a past Ask — answer + jumpable citations (local)",
            )),
        ),
        plain_spec(
            Self::SemanticFind,
            "ai:find-similar",
            AiCost::None,
            ModelWeight::Heavy,
            "AI · Find similar scenes",
            Some((
                "Find similar scenes",
                "Ranked, jumpable passages like a selection or a query (local, reads only)",
            )),
        ),
        AiActionSpec {
            finding_kind: FindingKind::Echo,
            ..plain_spec(
                Self::Echo,
                "ai:echo",
                AiCost::None,
                ModelWeight::Heavy,
                "AI · Echo & near-duplicate detection",
                Some((
                    "Echo & near-duplicate detection",
                    "Flag repeated / echoed passage pairs with scores (local, reads only)",
                )),
            )
        },
        plain_spec(
            Self::Thesaurus,
            "ai:thesaurus",
            AiCost::None,
            ModelWeight::Heavy,
            "AI · Word search (thesaurus)",
            Some((
                "Word search",
                "In-context thesaurus for a word or phrase (reference only)",
            )),
        ),
        plain_spec(
            Self::NameGen,
            "ai:name",
            AiCost::None,
            ModelWeight::Heavy,
            "AI · Name generator",
            Some((
                "Name generator",
                "Candidate character / place / term names by constraints (you pick)",
            )),
        ),
        findings_spec(
            Self::Editorial,
            "ai:editorial",
            ModelWeight::Heavy,
            FindingKind::Editorial,
            "AI · Editorial read (flags, no fixes)",
            (
                "Editorial read",
                "Flag structural / continuity / line issues — no fixes",
            ),
            None,
        ),
        findings_spec(
            Self::Accuracy,
            "ai:accuracy",
            ModelWeight::Heavy,
            FindingKind::Accuracy,
            "AI · Cultural & historical accuracy (flags, no fixes)",
            (
                "Cultural & historical accuracy",
                "Flag inaccuracies for a concern you name — accuracy, not sensitivity",
            ),
            // The named period / place / domain to check accuracy against (#973).
            Some("concern"),
        ),
        findings_spec(
            Self::Relationship,
            "ai:relationship",
            ModelWeight::Heavy,
            FindingKind::Relationship,
            "AI · Relationship trajectory (flags, no fixes)",
            (
                "Relationship trajectory",
                "Track a pair's arc per chapter + flag inconsistencies (no fixes)",
            ),
            // The character pair whose relationship trajectory to track (#1046).
            Some("pair"),
        ),
        findings_spec(
            Self::Foreshadow,
            "ai:foreshadow",
            ModelWeight::Heavy,
            FindingKind::Foreshadow,
            "AI · Foreshadow & payoff (flags dangling setups, no fixes)",
            (
                "Foreshadow & payoff",
                "Flag concrete setups that never pay off — flags only, no drafted payoff",
            ),
            None,
        ),
        findings_spec(
            Self::FactCheck,
            "ai:factcheck",
            ModelWeight::Heavy,
            FindingKind::FactCheck,
            "AI · Fact-check vs. research notebook (flags, no fixes)",
            (
                "Fact-check vs. research notebook",
                "Flag prose that contradicts your own research notes — no fixes",
            ),
            None,
        ),
        findings_spec(
            Self::Pacing,
            "ai:pacing",
            ModelWeight::Heavy,
            FindingKind::Pacing,
            "AI · Pacing & tension reading (observations, no changes)",
            (
                "Pacing & tension",
                "Located observations of the scene-level rhythm — observation only, no changes",
            ),
            None,
        ),
    ];

    /// This action's descriptor — its single source of every derivable fact.
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

    /// The full run-action label (`"AI · Character bible"`) — the AI half of
    /// [`Action::label`](super::Action::label), a descriptor lookup (#563).
    pub(crate) fn label(self) -> &'static str {
        self.spec().label
    }

    /// The AI palette's short label + one-line blurb, or `None` for an action that
    /// never lists in the AI palette directly (none today). Descriptor lookup.
    pub(crate) fn palette_entry(self) -> Option<(&'static str, &'static str)> {
        self.spec().palette
    }

    /// Whether the authorial assistant (#392) may run this action as a tool — the
    /// descriptor field that CLOSES the hand-maintained allowlist (#1069).
    pub(crate) fn assistant_runnable(self) -> bool {
        self.spec().assistant_runnable
    }

    /// The JSON property name of this action's free-text argument (#390), or `None`.
    pub(crate) fn text_arg(self) -> Option<&'static str> {
        self.spec().text_arg
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
    /// store, so the health dashboard can group by pass. A descriptor lookup, so the
    /// pass→kind mapping is written exactly once (#563) and can't drift; the passes
    /// that surface no triage findings carry [`Other`](crate::mode::FindingKind::Other)
    /// in the table.
    pub(crate) fn finding_kind(self) -> FindingKind {
        self.spec().finding_kind
    }
}
