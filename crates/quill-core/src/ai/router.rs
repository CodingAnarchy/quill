//! Hybrid AI routing (#372): a pure policy that picks the local or the remote
//! backend per action, from the remote cost estimate, the action's local
//! capability (bounded by the on-device model's practical context), and the
//! author's stated preference. It turns the cost estimator from something that
//! only *warns* about a price into something that can *act* on it — running the
//! cheap, frequent work on-device and reserving the paid cloud model for the
//! passes whose size genuinely needs it.
//!
//! [`route`] is a pure function of its inputs (no I/O, no `self`), so the policy
//! is fully unit-testable. The host (`src/app`) resolves the live inputs
//! (estimate, readiness, the `offline` pin) and applies the decision. Routing
//! only ever changes *where* an action runs — never *what* it does, and never the
//! manuscript-prose boundary.

use super::ModelWeight;
use crate::ai::cost::Estimate;
use crate::ai::model_select::STANDARD_CONTEXT_TOKENS;
use crate::ai::BackendKind;

/// The author's routing preference, from `[ai] routing`. It's a config *value*, so
/// the enum now lives in [`config::Policy`](crate::config::Policy) alongside the
/// rest of the `[ai]` data (#809); re-exported here so `router::Policy` is
/// unchanged for callers and the routing *logic* below still reads it as `Policy`.
pub use crate::config::Policy;

/// The live inputs the host resolves before calling [`route`].
#[derive(Clone, Copy, Debug)]
pub struct Inputs {
    pub policy: Policy,
    /// The session backend (what `[ai] backend` / the in-app switch selected),
    /// before the `offline` pin is applied.
    pub configured: BackendKind,
    /// The hard privacy pin — when set, everything stays local regardless of policy.
    pub offline: bool,
    /// Whether the local backend is usable (compiled in via `--features local`).
    pub local_available: bool,
    /// Whether an available local model's context window can hold this pass
    /// (#378) — resolved by the host from the configured model set. A long-context
    /// model makes this true for a whole-manuscript pass the standard ones can't.
    pub local_fits: bool,
    /// The estimated input-token count for this pass (#849) — reused from the same
    /// `context_len / 4` estimate that feeds `local_fits`. When the pass is pinned
    /// `offline` and this exceeds the on-device window, routing can't send it to the
    /// cloud, so it surfaces the size (~N tokens) in a graceful refusal instead.
    pub est_tokens: u32,
    /// The remote (Claude) estimate for this pass.
    pub est: Estimate,
    /// The author's cost-confirm threshold, in dollars.
    pub threshold: f64,
}

/// The router's decision for one action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// Where to run it (the default if a confirm is offered and declined-to-run).
    pub backend: BackendKind,
    /// When `Some`, confirm with the author first; the field carries whether the
    /// local backend is offered as a third choice (`true` ⇒ cloud / local / cancel,
    /// `false` ⇒ the plain spend / cancel confirm).
    pub confirm: Option<Confirm>,
    /// A one-line rationale, surfaced to the author so the choice is never hidden.
    pub reason: String,
    /// When `Some`, the pass cannot run as routed and must be *refused* with this
    /// actionable message rather than sent to a backend (#849). The one case today:
    /// an `offline`-pinned pass too big for the on-device window, where there is no
    /// cloud to fall back to — the host surfaces the message and runs nothing, so
    /// the pass never reaches candle to die with an opaque `forward:` error.
    pub refuse: Option<String>,
}

/// A pending routing confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirm {
    /// Whether "run locally instead" is one of the offered choices.
    pub offer_local: bool,
}

impl Route {
    fn run(backend: BackendKind, reason: impl Into<String>) -> Self {
        Self {
            backend,
            confirm: None,
            reason: reason.into(),
            refuse: None,
        }
    }

    /// A backend run gated behind a cost confirm (spend / cancel, optionally
    /// offering local as a third choice).
    fn confirm(backend: BackendKind, offer_local: bool, reason: impl Into<String>) -> Self {
        Self {
            backend,
            confirm: Some(Confirm { offer_local }),
            reason: reason.into(),
            refuse: None,
        }
    }

    /// A refusal: the pass can't run as routed. The default backend is still Local
    /// (the offline pin), but `refuse` carries the message the host surfaces instead
    /// of running it (#849).
    fn refuse(message: String) -> Self {
        Self {
            backend: BackendKind::Local,
            confirm: None,
            reason: "offline — pass too large for the on-device model".into(),
            refuse: Some(message),
        }
    }
}

/// The graceful "too big for on-device, offline" refusal message (#849): names the
/// size (~N tokens) against the model's window and the two remedies — disable
/// `offline` to use Claude, or narrow the pass's scope. Kept next to `route` so the
/// pure policy owns the wording; the host surfaces it verbatim.
pub fn offline_too_big_message(est_tokens: u32) -> String {
    format!(
        "This pass needs ~{est_tokens} tokens, but the on-device model's window holds \
         {STANDARD_CONTEXT_TOKENS}. With `offline` set there's no cloud to fall back to — \
         turn off `[ai] offline` for this pass to run it on Claude, or narrow its scope \
         (analyze fewer chapters / a single book)."
    )
}

/// Decide local vs remote for one action — pure, total, and independent of any
/// I/O. See [`Policy`] for what each preference does.
pub fn route(i: &Inputs) -> Route {
    // The hard offline pin wins over every policy — but never at the cost of
    // feeding the on-device model a pass it can't hold (#849). Offline has no cloud
    // to fall back to, so a too-big pass is *refused* with an actionable message
    // instead of being sent to candle to die with an opaque `forward:` error. A
    // foldable pass is dispatched incrementally upstream; this is the floor.
    if i.offline {
        return if i.local_fits {
            Route::run(BackendKind::Local, "offline mode — on-device")
        } else {
            Route::refuse(offline_too_big_message(i.est_tokens))
        };
    }
    let over = i.est.dollars > i.threshold;
    // Local can run this only if a downloaded model's context can hold it (#378);
    // compiled-in but no model fits ⇒ "too big" → cloud with a notice.
    let local_fits = i.local_fits;
    let too_big = i.local_available && !i.local_fits;

    match i.policy {
        // Router off: the configured backend, with the historical over-threshold
        // confirm when that backend is the paid one.
        Policy::Configured => cloudish(i.configured, over),
        Policy::PreferCloud => cloudish(BackendKind::Claude, over),

        // Lean on-device, but never feed the local model a pass it can't hold.
        Policy::PreferLocal => {
            if local_fits {
                Route::run(BackendKind::Local, "prefer-local — free, on-device")
            } else if too_big {
                cloudish_reason(over, "too large for the local model — using Claude")
            } else {
                cloudish_reason(over, "local backend unavailable — using Claude")
            }
        }

        // Spend on the cloud only up to the ceiling; past it, drop to local.
        Policy::CostCeiling => {
            if over && local_fits {
                Route::run(
                    BackendKind::Local,
                    "over the cost ceiling — routed on-device",
                )
            } else if over && too_big {
                cloudish_reason(
                    over,
                    "over the ceiling but too large for local — using Claude",
                )
            } else {
                // Under the ceiling (or no local): cloud, no confirm needed.
                Route::run(BackendKind::Claude, "under the cost ceiling — using Claude")
            }
        }

        // Offer the choice when it actually costs over the threshold.
        Policy::Ask => {
            if over {
                Route::confirm(
                    BackendKind::Claude,
                    local_fits,
                    "over the threshold — choose a backend",
                )
            } else {
                Route::run(BackendKind::Claude, "under the threshold — using Claude")
            }
        }
    }
}

/// The configured-backend path: run `backend`, but if it's the paid cloud model
/// and the estimate is over the threshold, confirm first (spend / cancel — the
/// historical behavior). A local session backend just runs (free).
fn cloudish(backend: BackendKind, over: bool) -> Route {
    match backend {
        BackendKind::Local => Route::run(BackendKind::Local, "local backend — free, on-device"),
        BackendKind::Claude => cloudish_reason(over, "using Claude"),
    }
}

/// Run Claude, asking first (spend / cancel) when the estimate is over the
/// threshold. Used both directly and as the fallback when a local-leaning policy
/// can't use local for this pass. Readiness (an API key) is checked separately at
/// run, so the confirm reflects only cost intent — as it always has.
fn cloudish_reason(over: bool, reason: &str) -> Route {
    if over {
        Route::confirm(BackendKind::Claude, false, reason)
    } else {
        Route::run(BackendKind::Claude, reason)
    }
}

/// The default heavy-tier Claude model (#930). Matches
/// [`config::ai::default_ai_model`] — a pass that runs on the *unchanged* default
/// `[ai] model` is a candidate for the light step-down; a pass on an author-chosen
/// model is not (the author's choice is the override, and it wins). Kept in sync
/// with the config default: if the default lineup changes, update both.
pub const DEFAULT_CLOUD_MODEL: &str = "claude-opus-4-8";

/// The cheaper/faster Claude model a light, small pass steps down to (#930) — the
/// Haiku tier, the same model the naming pass already pins. Chosen for the
/// high-frequency light passes (style flags, marketing copy) when the estimate is
/// below [`LIGHT_STEPDOWN_TOKENS`], where the top model's depth isn't needed.
pub const LIGHT_CLOUD_MODEL: &str = "claude-haiku-4-5";

/// Below this many estimated input tokens a [`Light`](ModelWeight::Light) pass is
/// small enough to run on the cheaper [`LIGHT_CLOUD_MODEL`] without a meaningful
/// quality loss (#930). A larger light pass — a whole-manuscript style scan —
/// keeps the top model, since the extra context is where a cheaper model would
/// start to miss things. Deliberately generous: a single-chapter or scene-sized
/// pass (≈ a few thousand tokens) drops to the cheap tier; a book-sized one does not.
pub const LIGHT_STEPDOWN_TOKENS: u32 = 12_000;

/// Pick *which* cloud (Claude) model runs an action once routing has chosen the
/// cloud backend (#930) — pure, so it's table-tested alongside [`route`]. The
/// choice is the author's configured `[ai] model` (`configured`) by default, and
/// steps down to the cheaper [`LIGHT_CLOUD_MODEL`] only when **all** hold:
///
/// - the pass is [`Light`](ModelWeight::Light) (a naming/style-flag-class scan,
///   not a heavy continuity/characterization reasoning pass), and
/// - its estimate is under [`LIGHT_STEPDOWN_TOKENS`] (small enough that the top
///   model's depth isn't needed), and
/// - the author hasn't overridden `[ai] model` — `configured` is still the
///   [`DEFAULT_CLOUD_MODEL`]. An explicit model choice is a deliberate override
///   and wins for every pass; only the *default* is eligible to be stepped down.
///
/// Heavy passes always run on `configured`. The picker never changes *what* a pass
/// produces — only which model runs it — and it's independent of the offline/local
/// pins and the cost ceiling (those are [`route`]'s job; this is called only after
/// [`route`] has already resolved the backend to Claude).
pub fn pick_cloud_model(weight: ModelWeight, est_tokens: u32, configured: &str) -> &str {
    let can_step_down = matches!(weight, ModelWeight::Light)
        && est_tokens < LIGHT_STEPDOWN_TOKENS
        && configured == DEFAULT_CLOUD_MODEL;
    if can_step_down {
        LIGHT_CLOUD_MODEL
    } else {
        configured
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn est(dollars: f64, tokens: u32) -> Estimate {
        Estimate { tokens, dollars }
    }

    fn inputs(policy: Policy, est: Estimate) -> Inputs {
        Inputs {
            policy,
            configured: BackendKind::Claude,
            offline: false,
            local_available: true,
            local_fits: true, // a downloaded model can hold it (host-resolved, #378)
            est_tokens: est.tokens,
            est,
            threshold: 0.20,
        }
    }

    #[test]
    fn offline_pins_local_regardless_of_policy() {
        for policy in [
            Policy::Configured,
            Policy::PreferCloud,
            Policy::Ask,
            Policy::CostCeiling,
        ] {
            // A fitting pass pins on-device under every policy (local_fits defaults true).
            let mut i = inputs(policy, est(99.0, 1000));
            i.offline = true;
            assert_eq!(route(&i).backend, BackendKind::Local, "{policy:?}");
            assert!(route(&i).confirm.is_none());
            assert!(route(&i).refuse.is_none(), "a fitting pass isn't refused");
        }
    }

    #[test]
    fn offline_refuses_a_pass_too_big_for_the_on_device_window() {
        // The #849 floor: offline-pinned but the pass exceeds the local window, so
        // there's no cloud to fall back to → a graceful refusal, never a backend run
        // (which would feed candle a pass it can't hold).
        for policy in [
            Policy::Configured,
            Policy::PreferCloud,
            Policy::Ask,
            Policy::CostCeiling,
            Policy::PreferLocal,
        ] {
            let mut i = inputs(policy, est(0.0, 120_000));
            i.offline = true;
            i.local_fits = false; // no downloaded model can hold it
            i.est_tokens = 120_000;
            let r = route(&i);
            let msg = r
                .refuse
                .as_deref()
                .unwrap_or_else(|| panic!("{policy:?}: an oversized offline pass must be refused"));
            assert!(r.confirm.is_none(), "{policy:?}: a refusal isn't a confirm");
            // The message names the size, the window, and both remedies.
            assert!(msg.contains("120000"), "states the size: {msg}");
            assert!(msg.contains("40960"), "states the window: {msg}");
            assert!(msg.contains("offline"), "names the offline remedy: {msg}");
            assert!(msg.contains("narrow"), "offers the scope remedy: {msg}");
        }
    }

    #[test]
    fn configured_preserves_the_historical_confirm() {
        // Cheap Claude pass: runs, no confirm.
        let cheap = route(&inputs(Policy::Configured, est(0.05, 1000)));
        assert_eq!(cheap.backend, BackendKind::Claude);
        assert!(cheap.confirm.is_none());
        // Expensive Claude pass: spend/cancel confirm, no local option.
        let dear = route(&inputs(Policy::Configured, est(0.80, 1000)));
        assert_eq!(dear.confirm, Some(Confirm { offer_local: false }));
        // A local session backend just runs (free).
        let mut local = inputs(Policy::Configured, est(99.0, 1000));
        local.configured = BackendKind::Local;
        assert_eq!(
            route(&local),
            Route::run(BackendKind::Local, "local backend — free, on-device")
        );
    }

    #[test]
    fn prefer_local_runs_local_when_it_fits_and_falls_back_when_too_big() {
        let fits = route(&inputs(Policy::PreferLocal, est(0.80, 10_000)));
        assert_eq!(fits.backend, BackendKind::Local);
        assert!(fits.confirm.is_none(), "local is free — nothing to confirm");

        // A pass no downloaded model can hold routes to Claude with a notice (and
        // the usual confirm since it's over threshold).
        let mut big = inputs(Policy::PreferLocal, est(0.80, 100_000));
        big.local_fits = false;
        let big = route(&big);
        assert_eq!(big.backend, BackendKind::Claude);
        assert!(big.reason.contains("too large"));
        assert_eq!(big.confirm, Some(Confirm { offer_local: false }));

        // No local backend compiled in → Claude (nothing fits when none is available).
        let mut nolocal = inputs(Policy::PreferLocal, est(0.05, 1000));
        nolocal.local_available = false;
        nolocal.local_fits = false;
        assert_eq!(route(&nolocal).backend, BackendKind::Claude);
    }

    #[test]
    fn cost_ceiling_routes_local_only_past_the_threshold() {
        // Under the ceiling → Claude, no confirm.
        let under = route(&inputs(Policy::CostCeiling, est(0.05, 5000)));
        assert_eq!(under.backend, BackendKind::Claude);
        assert!(under.confirm.is_none());
        // Over the ceiling and it fits → local (free), no confirm.
        let over = route(&inputs(Policy::CostCeiling, est(0.80, 5000)));
        assert_eq!(over.backend, BackendKind::Local);
        assert!(over.confirm.is_none());
        // Over the ceiling but no downloaded model fits → Claude with a notice.
        let mut huge = inputs(Policy::CostCeiling, est(0.80, 100_000));
        huge.local_fits = false;
        let huge = route(&huge);
        assert_eq!(huge.backend, BackendKind::Claude);
        assert!(huge.reason.contains("too large"));
    }

    #[test]
    fn ask_offers_local_only_when_over_threshold_and_local_fits() {
        let under = route(&inputs(Policy::Ask, est(0.05, 5000)));
        assert!(under.confirm.is_none(), "cheap passes just run");

        let over = route(&inputs(Policy::Ask, est(0.80, 5000)));
        assert_eq!(over.confirm, Some(Confirm { offer_local: true }));

        // Over threshold but no downloaded model fits → ask without the local option.
        let mut big = inputs(Policy::Ask, est(0.80, 100_000));
        big.local_fits = false;
        assert_eq!(route(&big).confirm, Some(Confirm { offer_local: false }));
    }

    // --- per-action cloud model picker (#930) --------------------------------

    #[test]
    fn heavy_passes_always_use_the_configured_model() {
        // A heavy reasoning pass keeps the top model regardless of size — even a
        // tiny one, where a light pass would step down.
        for tokens in [
            500,
            LIGHT_STEPDOWN_TOKENS - 1,
            LIGHT_STEPDOWN_TOKENS,
            100_000,
        ] {
            assert_eq!(
                pick_cloud_model(ModelWeight::Heavy, tokens, DEFAULT_CLOUD_MODEL),
                DEFAULT_CLOUD_MODEL,
                "heavy @ {tokens} tokens keeps the top model",
            );
        }
    }

    #[test]
    fn light_small_passes_step_down_to_the_cheaper_tier() {
        // A light, small pass on the default model drops to the cheaper/faster tier.
        assert_eq!(
            pick_cloud_model(ModelWeight::Light, 2_000, DEFAULT_CLOUD_MODEL),
            LIGHT_CLOUD_MODEL,
        );
        // Just under the step-down ceiling still steps down…
        assert_eq!(
            pick_cloud_model(
                ModelWeight::Light,
                LIGHT_STEPDOWN_TOKENS - 1,
                DEFAULT_CLOUD_MODEL
            ),
            LIGHT_CLOUD_MODEL,
        );
    }

    #[test]
    fn light_large_passes_keep_the_top_model() {
        // A big light pass (a whole-manuscript style scan) is where a cheaper model
        // would start to miss things, so it keeps the top model. At the ceiling and
        // above it does not step down.
        assert_eq!(
            pick_cloud_model(
                ModelWeight::Light,
                LIGHT_STEPDOWN_TOKENS,
                DEFAULT_CLOUD_MODEL
            ),
            DEFAULT_CLOUD_MODEL,
        );
        assert_eq!(
            pick_cloud_model(ModelWeight::Light, 80_000, DEFAULT_CLOUD_MODEL),
            DEFAULT_CLOUD_MODEL,
        );
    }

    #[test]
    fn an_author_model_override_wins_for_every_pass() {
        // When the author has pinned a non-default `[ai] model`, that choice is the
        // override and is used verbatim — a light, small pass does NOT step down
        // away from it (the override is deliberate).
        let pinned = "claude-sonnet-4-6";
        assert_eq!(
            pick_cloud_model(ModelWeight::Light, 1_000, pinned),
            pinned,
            "a small light pass respects the author's pinned model",
        );
        assert_eq!(pick_cloud_model(ModelWeight::Heavy, 1_000, pinned), pinned,);
        // Even if the author pins the Haiku tier itself, it's returned as-is.
        assert_eq!(
            pick_cloud_model(ModelWeight::Heavy, 100_000, LIGHT_CLOUD_MODEL),
            LIGHT_CLOUD_MODEL,
        );
    }

    #[test]
    fn the_picker_is_independent_of_the_offline_and_ceiling_route() {
        // The picker only chooses *which* cloud model; it never decides local-vs-
        // cloud. That stays `route`'s job — a smoke check that the two are separate
        // functions with no shared branch: a light small pass steps down here even
        // though `route` (given the same size, over threshold) would send it local.
        let stepped = pick_cloud_model(ModelWeight::Light, 2_000, DEFAULT_CLOUD_MODEL);
        assert_eq!(stepped, LIGHT_CLOUD_MODEL);
        // And the cost estimate reflects the chosen model: the cheaper tier quotes
        // strictly less than the top model for the identical request.
        let chars = 2_000 * 4;
        let light = crate::ai::cost::estimate(chars, 4096, LIGHT_CLOUD_MODEL, false);
        let heavy = crate::ai::cost::estimate(chars, 4096, DEFAULT_CLOUD_MODEL, false);
        assert!(
            light.dollars < heavy.dollars,
            "cost reflects the model: {} (light) < {} (heavy)",
            light.dollars,
            heavy.dollars,
        );
    }

    #[test]
    fn the_default_cloud_model_matches_the_config_default() {
        // The step-down only applies to the *unchanged* default `[ai] model`, so
        // this constant must equal the config default or the step-down never fires.
        assert_eq!(
            DEFAULT_CLOUD_MODEL,
            crate::config::AiConfig::default().model
        );
    }
}
