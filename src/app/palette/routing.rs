//! Backend selection for an AI action (#82/#372/#849): given the pass the author
//! chose from the palette, decide *how* it runs — on-device or in the cloud, and
//! whether the cloud estimate is big enough to confirm first — then arm the
//! one-shot routing and start the request.
//!
//! [`AiStart`] is that decision, kept as a value so the palette, the assistant
//! chain, and the workflow runner all gate on the same rule and so the tests can
//! assert the routing without running a pass. The cost side (the estimate label,
//! the threshold, the "run locally instead" offer) and the model pick (the local
//! model sized to the context, the per-action cloud override) live here with it.

use super::super::*;

use crate::ai::{self, cost, naming, router, BackendKind};
use crate::mode::{AiAction, AiCost};

/// The routing decision for starting an AI action (#82/#372): run it now on a
/// chosen backend, or confirm first (the estimate is over the threshold) — the
/// confirm optionally offering "run locally instead" as a third choice.
#[cfg_attr(test, derive(Debug))]
pub enum AiStart {
    Run(BackendKind),
    Confirm {
        label: String,
        offer_local: bool,
    },
    /// The pass can't run as routed and is refused with this actionable message
    /// (#849): an `offline`-pinned pass too big for the on-device window, where
    /// there's no cloud to fall back to. Surfaced as a status message; nothing runs.
    Refuse(String),
}

impl App {
    /// Begin an AI action chosen from the palette: for a network pass, show a
    /// cost confirm first when the estimate exceeds the threshold; the local /
    /// interactive actions (Ask, Triage) run immediately.
    pub(crate) fn start_ai_action(&mut self, action: AiAction, force: bool) {
        match self.ai_action_start(action, force) {
            AiStart::Run(backend) => {
                // Route this request to the chosen backend (consumed by backend_spec).
                self.route_request(backend, action, force);
                self.run_ai_action(action, force);
            }
            AiStart::Confirm { label, offer_local } => self.open_prompt(
                Pending::ConfirmAi {
                    action,
                    force,
                    offer_local,
                },
                label,
                "",
                None,
            ),
            // Offline and too big to run or fold on-device (#849): surface the
            // actionable refusal, run nothing.
            AiStart::Refuse(message) => self.message = Some(message),
        }
    }

    /// Set the one-shot routing for the next AI request (#372/#377): the backend,
    /// plus — when it's local — the model the router picks for this action's size
    /// from the author's configured set. Both are consumed by `backend_spec`.
    pub(crate) fn route_request(&self, backend: BackendKind, action: AiAction, force: bool) {
        self.routed_backend.set(Some(backend));
        if backend == BackendKind::Local {
            self.routed_model.set(self.pick_local_model(action, force));
            // A local pass never uses a per-action cloud model — clear any one-shot
            // a prior (cancelled) cloud route may have armed, so it can't leak.
            self.routed_cloud_model.set(None);
        } else {
            // Cloud: arm the per-action model the pass's weight + estimate resolved
            // to (#930), so the request runs on the chosen tier rather than always
            // the session `[ai] model`. Only for the costed manuscript passes — an
            // interactive/local action has no `cost_profile`, so it keeps the
            // session model (the one-shot stays `None`).
            self.routed_cloud_model
                .set(self.cloud_model_override(action, force));
        }
    }

    /// The per-action cloud model to arm for this request (#930), or `None` when
    /// the pass keeps the session `[ai] model`: `Some` only for a costed manuscript
    /// pass whose picker chose a model *different* from the session one (a light,
    /// small pass stepped down to the cheaper tier). A heavy pass — or any pass the
    /// picker leaves on the configured model — arms nothing, so the existing
    /// `self.ai.model` path is untouched.
    fn cloud_model_override(&self, action: AiAction, force: bool) -> Option<String> {
        if action.cost() != AiCost::Manuscript {
            return None;
        }
        let chosen = self.cloud_model_for(action, self.pass_context_len(action, force));
        (chosen != self.ai.model).then(|| chosen.to_string())
    }

    /// The local model best suited to this action (#377): the router picks from the
    /// author's downloaded set (`[ai] local_models`) by the pass's size. `None`
    /// when no set is configured — the backend then loads the single recommended
    /// model, as before.
    fn pick_local_model(
        &self,
        action: AiAction,
        force: bool,
    ) -> Option<&'static ai::model_select::Model> {
        let available = ai::model_select::available(&self.ai.local_models);
        if available.is_empty() {
            return None;
        }
        let tokens = (self.pass_context_len(action, force) / 4) as u32;
        ai::model_select::pick_for(tokens, &available)
    }

    /// Whether an *actually-present* local model can hold a pass of `input_tokens`
    /// (#378/#725) — the router routes on-device only when one can. Requires both a
    /// fitting context window and a model whose weights are already on disk
    /// (`--features local` compiled in but nothing downloaded ⇒ not eligible, so a
    /// routed pass never kicks off a surprise multi-GB HuggingFace fetch, #725).
    /// With no model set configured, the single recommended model is the candidate;
    /// otherwise it's the configured set, so a long-context model lets a
    /// whole-manuscript pass run locally.
    pub(crate) fn local_fits(&self, input_tokens: u32) -> bool {
        cfg!(feature = "local")
            && input_tokens <= ai::model_select::local_window(&self.ai.local_models)
            && self.local_model_present(input_tokens)
    }

    /// Whether an oversized `offline` pass for `action` can be folded on-device
    /// instead of refused (#849): the persistent-bible passes (character, setting,
    /// timeline-continuity, plot, theme) have an incremental-revise path, so a full
    /// rebuild that doesn't fit is walked chapter-by-chapter through the window.
    /// A one-shot pass with no merge step (whole-manuscript Q&A, the findings-only
    /// scans) can't fold and takes the graceful-report path.
    fn foldable_offline(action: AiAction) -> bool {
        super::worker::bible_tracker(action).is_some()
    }

    /// The cost decision for a network AI action (#82): run it now, or confirm
    /// first because the estimate exceeds the threshold. Shared by the AI palette
    /// ([`start_ai_action`]) and the workflow runner (#227) so a workflow's AI
    /// step follows the same cost policy.
    pub(crate) fn ai_action_start(&self, action: AiAction, force: bool) -> AiStart {
        let Some((model, max_tokens)) = self.cost_profile(action, force) else {
            // Interactive / image action — no costed pass to route; run on the
            // session backend (which an `offline` pin / the in-app switch resolve).
            return AiStart::Run(self.ai.backend());
        };
        // Estimate the remote cost, then let the hybrid router (#372) pick a backend
        // from the policy, the estimate, and each backend's readiness. Interactive
        // passes run synchronously at the standard rate — the Batch API (half price)
        // is reserved for a future opt-in background update — so no batch discount.
        let est = cost::estimate(
            self.pass_context_len(action, force),
            max_tokens,
            model,
            false,
        );
        let route = router::route(&router::Inputs {
            policy: self.ai.routing,
            configured: self.ai.backend,
            offline: self.ai.offline,
            local_available: cfg!(feature = "local"),
            local_fits: self.local_fits(est.tokens),
            est_tokens: est.tokens,
            est,
            threshold: self.ai.cost_threshold,
        });
        // An offline pass too big for the on-device window is refused (#849) —
        // *unless* it can be folded: a persistent-bible pass runs incrementally,
        // window-sized chunk by chunk, so it still completes on-device. The fold is
        // driven by `run_bible_tracker`, which re-detects the oversized-offline
        // condition; here we only decline to refuse a foldable pass.
        if let Some(message) = route.refuse {
            return if Self::foldable_offline(action) {
                AiStart::Run(BackendKind::Local)
            } else {
                AiStart::Refuse(message)
            };
        }
        match route.confirm {
            None => AiStart::Run(route.backend),
            Some(c) if c.offer_local => AiStart::Confirm {
                label: format!(
                    "{} — y Claude · l local · N cancel",
                    cost::compare_label(&est, model)
                ),
                offer_local: true,
            },
            Some(_) => AiStart::Confirm {
                label: format!("{} — run? (y/N)", cost::label(&est, model)),
                offer_local: false,
            },
        }
    }

    /// The model and output bound a network pass will send — the inputs to its
    /// cost estimate, read from the same constants the request builders use so
    /// the estimate can't drift from the real request. Which profile each action
    /// carries is the descriptor's [`AiCost`] (#563); this resolves it against
    /// live AI settings. `None` for the local / interactive / image actions
    /// (Ask, Triage, Brainstorm, the drafts, the portraits/scene, the thesaurus),
    /// which issue no costed whole-manuscript pass.
    fn cost_profile(&self, action: AiAction, force: bool) -> Option<(&str, u32)> {
        match action.cost() {
            AiCost::None => None,
            AiCost::Naming => Some((naming::MODEL, naming::MAX_TOKENS)),
            // Per-action model routing (#930): a heavy reasoning pass runs on the
            // configured top model; a light, small one can drop to a cheaper tier.
            // The estimate below then reflects whichever model is chosen, and the
            // same model is armed as the one-shot cloud model the request uses.
            AiCost::Manuscript => Some((
                self.cloud_model_for(action, self.pass_context_len(action, force)),
                ai::MAX_OUTPUT_TOKENS,
            )),
        }
    }

    /// The cloud (Claude) model this action's manuscript pass will run on (#930):
    /// the pure per-action picker applied to the pass's weight, its token estimate
    /// (`context_chars / 4`, the same figure the cost estimate uses), and the
    /// author's configured `[ai] model` — a heavy pass keeps the top model, a
    /// light, small one drops to the cheaper tier, and an author-set model always
    /// wins. Borrows from `self` (either `self.ai.model` or a `'static` tier id).
    fn cloud_model_for(&self, action: AiAction, context_len: usize) -> &str {
        router::pick_cloud_model(action.weight(), (context_len / 4) as u32, &self.ai.model)
    }

    /// Dispatch an AI action to its handler. Shared by the palette and the cost
    /// confirm.
    pub(crate) fn run_ai_action(&mut self, action: AiAction, force: bool) {
        match action {
            // The actions that take a free-text argument (#390) prompt for it.
            AiAction::Ask
            | AiAction::Brainstorm
            | AiAction::DraftCharacters
            | AiAction::DraftSetting
            | AiAction::Accuracy
            | AiAction::Relationship => self.prompt_ai_arg(action),
            AiAction::Characters => self.analyze_characters(force),
            AiAction::Continuity => self.analyze_continuity(force),
            AiAction::Setting => self.analyze_setting(force),
            AiAction::Plot => self.analyze_plot(force),
            AiAction::Theme => self.analyze_theme(force),
            AiAction::Scaffold => self.analyze_scaffold(),
            AiAction::Naming => self.analyze_naming(),
            AiAction::Style => self.analyze_style(),
            AiAction::Voice => self.analyze_voice(),
            AiAction::Submission => self.analyze_submission(),
            AiAction::Marketing => self.analyze_marketing(),
            AiAction::Social => self.analyze_social(),
            AiAction::AdCreative => self.analyze_ad_creative(),
            AiAction::Portrait => self.start_portrait(),
            AiAction::Scene => self.start_scene(),
            AiAction::SemanticFind => self.start_semantic_find(),
            AiAction::Echo => self.start_echo_detection(),
            AiAction::Triage => self.open_triage(),
            AiAction::Dashboard => self.open_health_dashboard(),
            AiAction::CoOccurrence => self.open_cooccurrence(),
            AiAction::AskHistory => self.open_qa_history(),
            AiAction::Thesaurus => self.start_thesaurus(),
            AiAction::NameGen => self.start_namegen(),
            AiAction::Editorial => self.analyze_editorial(),
            AiAction::Foreshadow => self.analyze_foreshadow(),
            // Author-invoked research-notebook fact-check (#1049): no argument — it
            // checks against the author's own `bible/research.md`, so it runs directly.
            AiAction::FactCheck => self.analyze_factcheck(),
            // Author-invoked pacing & tension reading (#1048): no argument — it reads
            // the whole manuscript's rhythm and reports located observations, so it
            // runs directly.
            AiAction::Pacing => self.analyze_pacing(),
        }
    }

    /// Ask the author for an AI action's free-text argument (#390) — or, on the
    /// cost-gated re-entry of an action whose argument was already captured (the
    /// confirm runs [`run_ai_action`](Self::run_ai_action) again), run it with that
    /// argument instead of re-prompting.
    fn prompt_ai_arg(&mut self, action: AiAction) {
        if let Some(text) = self.take_ai_arg(action) {
            self.run_ai_arg(action, &text);
        } else if let Some(arg) = action.arg() {
            self.open_prompt(Pending::AiArg(action), arg.label, "", None);
        }
    }

    /// The author submitted `action`'s argument prompt with `text` (trimmed). A
    /// blank entry cancels where the action can't run without one. A
    /// manuscript-costed pass (accuracy #973, relationship #1046) captures the
    /// argument and goes through the normal cost gate, whose confirmed path
    /// re-enters `run_ai_action` with the argument set; the rest run now.
    pub(crate) fn submit_ai_arg(&mut self, action: AiAction, text: &str) {
        let Some(arg) = action.arg() else {
            return;
        };
        match arg.blank {
            Some(blank) if text.is_empty() => self.message = Some(blank.into()),
            _ if action.cost() == AiCost::Manuscript => {
                self.pending_ai_arg = Some((action, text.to_string()));
                self.start_ai_action(action, false);
            }
            _ => self.run_ai_arg(action, text),
        }
    }

    /// Run a text-argument AI action with its argument, ungated — the one dispatch
    /// the prompt (after its cost gate) and the assistant (#390, which supplies the
    /// argument itself) share. Every one of these returns notes, ideas, bible
    /// entries, or findings — never manuscript prose.
    pub(crate) fn run_ai_arg(&mut self, action: AiAction, text: &str) {
        match action {
            AiAction::Ask => self.ask_ai(text),
            AiAction::Brainstorm => self.brainstorm(text),
            AiAction::DraftCharacters => self.draft_characters(text),
            AiAction::DraftSetting => self.draft_setting(text),
            AiAction::Accuracy => self.analyze_accuracy(text),
            AiAction::Relationship => self.analyze_relationship(text),
            _ => {}
        }
    }

    /// Take the argument captured for `action`'s cost-gated run, if any — only
    /// `action`'s own, so another action's leftover never leaks into this one.
    fn take_ai_arg(&mut self, action: AiAction) -> Option<String> {
        match self.pending_ai_arg.take() {
            Some((captured, text)) if captured == action => Some(text),
            other => {
                self.pending_ai_arg = other;
                None
            }
        }
    }
}
