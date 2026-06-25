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

use serde::Deserialize;

use crate::ai::cost::Estimate;
use crate::ai::BackendKind;

/// The author's routing preference, from `[ai] routing`. The default keeps the
/// historical behavior — always the configured/session backend — so routing is
/// strictly opt-in and an unset config behaves exactly as before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    /// Always the configured (session) backend; the router is off.
    #[default]
    Configured,
    /// Prefer the local model whenever it's available and big enough for the pass.
    PreferLocal,
    /// Prefer the cloud (Claude) backend.
    PreferCloud,
    /// Route to local when the remote estimate exceeds the cost threshold.
    CostCeiling,
    /// When the remote estimate exceeds the threshold, ask which backend to use.
    Ask,
}

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
    /// The remote (Claude) estimate for this pass.
    pub est: Estimate,
    /// The author's cost-confirm threshold, in dollars.
    pub threshold: f64,
}

/// The router's decision for one action.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    /// Where to run it (the default if a confirm is offered and declined-to-run).
    pub backend: BackendKind,
    /// When `Some`, confirm with the author first; the field carries whether the
    /// local backend is offered as a third choice (`true` ⇒ cloud / local / cancel,
    /// `false` ⇒ the plain spend / cancel confirm).
    pub confirm: Option<Confirm>,
    /// A one-line rationale, surfaced to the author so the choice is never hidden.
    pub reason: String,
}

/// A pending routing confirmation.
#[derive(Clone, Debug, PartialEq)]
pub struct Confirm {
    /// Whether "run locally instead" is one of the offered choices.
    pub offer_local: bool,
}

impl Route {
    fn run(backend: BackendKind, reason: impl Into<String>) -> Self {
        Route { backend, confirm: None, reason: reason.into() }
    }
}

/// Decide local vs remote for one action — pure, total, and independent of any
/// I/O. See [`Policy`] for what each preference does.
pub fn route(i: &Inputs) -> Route {
    // The hard offline pin wins over every policy.
    if i.offline {
        return Route::run(BackendKind::Local, "offline mode — on-device");
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
                Route::run(BackendKind::Local, "over the cost ceiling — routed on-device")
            } else if over && too_big {
                cloudish_reason(over, "over the ceiling but too large for local — using Claude")
            } else {
                // Under the ceiling (or no local): cloud, no confirm needed.
                Route::run(BackendKind::Claude, "under the cost ceiling — using Claude")
            }
        }

        // Offer the choice when it actually costs over the threshold.
        Policy::Ask => {
            if over {
                Route {
                    backend: BackendKind::Claude,
                    confirm: Some(Confirm { offer_local: local_fits }),
                    reason: "over the threshold — choose a backend".into(),
                }
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
        Route { backend: BackendKind::Claude, confirm: Some(Confirm { offer_local: false }), reason: reason.into() }
    } else {
        Route::run(BackendKind::Claude, reason)
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
            est,
            threshold: 0.20,
        }
    }

    #[test]
    fn offline_pins_local_regardless_of_policy() {
        for policy in [Policy::Configured, Policy::PreferCloud, Policy::Ask, Policy::CostCeiling] {
            let mut i = inputs(policy, est(99.0, 1000));
            i.offline = true;
            assert_eq!(route(&i).backend, BackendKind::Local, "{policy:?}");
            assert!(route(&i).confirm.is_none());
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
        assert_eq!(route(&local), Route::run(BackendKind::Local, "local backend — free, on-device"));
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
}
