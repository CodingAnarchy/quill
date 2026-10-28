//! The typed config *data* for the `[ai]` and `[image]` sections, brought under
//! `config/` alongside the other sections (#809) so `config` is the single place
//! that describes the whole config file's shape. These are the plain,
//! serde-`Deserialize` fields plus the config-value enums they need
//! ([`BackendKind`], [`Policy`], [`ImageBackendKind`]).
//!
//! Only the *data* lives here. The AI/image *behavior* — API-key resolution
//! (env → OS keychain → plaintext, #530), the `keychain_cache` memo, `check_ready`,
//! and backend/router selection — stays in the `ai` module, hung on these types as
//! inherent `impl` blocks (an inherent impl may live in any module of the crate
//! that defines the type). That keeps the dependency direction `ai → config`:
//! `config` gains no dependency on `ai`, and the keychain/router logic never
//! migrates into this leaf module.

use std::cell::OnceCell;

use serde::Deserialize;

use super::load_section;

/// The default Claude model (1M context — a whole novel fits in one request). The
/// default lives with the config data (#809) — it seeds [`AiConfig::model`] — so
/// `config` stays a leaf with no dependency on `ai`.
fn default_ai_model() -> String {
    "claude-opus-4-8".to_string()
}

/// Echo / near-duplicate detection (#920) is available by default; an author who
/// leans on deliberate repetition can turn it off with `[ai] echo-detection =
/// false`. A field default (not a plain `bool`) so an existing config that never
/// mentions the key still gets the pass, not a silent off.
const fn default_echo_detection() -> bool {
    true
}

/// The background batch bible update (#950) is opt-in and off by default: it sends
/// a full-manuscript rebuild off-machine to the asynchronous Message Batches API
/// (half price, but a window of up to ~24h), so an author enables it deliberately
/// with `[ai] background-batch-update = true`. A field default keeps an existing
/// config (which never mentions the key) at the safe off state.
const fn default_background_batch_update() -> bool {
    false
}

/// The "verify on idle" auto-recheck (#1053) is opt-in and off by default: when
/// on, findings whose chapters changed since they were generated are re-checked
/// on the idle tick and cleared if an edit resolved them, so triage stays honest
/// during a revision pass without the author remembering to re-run. It re-runs
/// only the existing analysis passes scoped to the changed chapters (content-digest
/// keyed, like the bible cache), never the whole book, and never edits prose. A
/// field default keeps an existing config (which never mentions the key) at the
/// safe off state.
const fn default_verify_on_idle() -> bool {
    false
}

/// The fal.ai FLUX model used when no `[image] model` is configured — fast and
/// inexpensive. It seeds [`ImageConfig::model`] (#809).
fn default_image_model() -> String {
    "fal-ai/flux/schnell".to_string()
}

/// Which provider the AI features run against (#87).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    /// The Claude HTTP API (needs an API key; sends the manuscript off-machine).
    #[default]
    Claude,
    /// The local candle model (runs offline; nothing leaves the machine).
    Local,
}

/// The author's routing preference, from `[ai] routing`. The default keeps the
/// historical behavior — always the configured/session backend — so routing is
/// strictly opt-in and an unset config behaves exactly as before. The routing
/// *logic* (the pure `route` function and its inputs) lives in [`ai::router`],
/// which re-exports this enum; only the config value itself lives here (#809).
///
/// [`ai::router`]: crate::ai::router
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

/// Which image provider to generate with (#240).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ImageBackendKind {
    /// The remote fal.ai API (needs a key; sends the prompt off-machine).
    #[default]
    Remote,
    /// A local Stable-Diffusion model (runs offline; nothing leaves the machine,
    /// no per-image cost). Behind the `local` cargo feature.
    Local,
}

/// AI configuration for the `[ai]` section, loaded from the user's home config —
/// never the project dir, so an unpublished draft and the key stay separate. Off
/// by default.
///
/// This carries only the config *data*; the behavior (`resolved_key`, `backend`,
/// `check_ready`, …) is defined on it in the `ai` module (#809). It's re-exported
/// there as `ai::Settings` so existing call sites are unchanged.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    /// Master opt-in. When false, no AI feature is reachable.
    pub enabled: bool,
    /// Which backend to use (`claude` or `local`).
    pub backend: BackendKind,
    /// Hybrid routing policy (#372): how to pick local vs remote per action.
    /// Defaults to `configured` (always `backend`), so routing is opt-in.
    pub routing: Policy,
    /// The local models the author wants available (#377), by id (`["1.7B",
    /// "8B"]`); the router picks the best of these per request. Empty ⇒ the single
    /// hardware-recommended model, as before.
    pub local_models: Vec<String>,
    /// Privacy switch: when true, force the local backend so nothing ever leaves
    /// the machine, regardless of `backend`.
    pub offline: bool,
    /// Model id sent to the Claude backend (the local backend picks its own).
    pub model: String,
    /// The on-device embedding model id for grounded-Ask semantic retrieval (#831),
    /// by id (`"minilm"`). Empty ⇒ the default [`embed::EmbedModel::default_model`].
    /// Only consumed by the `local` build; when its weights aren't on disk, Ask
    /// degrades gracefully to sending the whole book (the pre-#831 path).
    ///
    /// [`embed::EmbedModel::default_model`]: crate::ai::embed::EmbedModel::default_model
    #[serde(default)]
    pub embed_model: String,
    /// API key, if set in the config file. The `ANTHROPIC_API_KEY` env var
    /// overrides this (see `AiConfig::resolved_key` in the `ai` module).
    pub api_key: Option<String>,
    /// Confirm before any Claude pass whose estimated cost exceeds this many
    /// dollars. Cheaper passes (and the free local backend) run without a prompt.
    pub cost_threshold: f64,
    /// Rebuild a bible from the whole manuscript (instead of an incremental
    /// revise) once this many chapters have changed since its last pass (#90).
    /// 0 disables incremental updates — every pass is a full rebuild.
    pub refresh_after_changes: usize,
    /// The off-switch for echo / near-duplicate detection (#920). When false the
    /// echo pass refuses to run and surfaces no findings, so an author who leans on
    /// deliberate repetition (a recurring refrain) isn't nagged. Default true — the
    /// pass is available; individual pairs the author decides are intentional are
    /// dismissed one-by-one through triage ("mark intentional reprise"), which the
    /// durable store already keeps from re-surfacing.
    #[serde(default = "default_echo_detection")]
    pub echo_detection: bool,
    /// Opt-in for the background batch bible update (#950). When true, the
    /// "Queue a background bible update" run-action becomes available: it submits a
    /// full-manuscript bible rebuild through the asynchronous Message Batches API
    /// (half the per-token price, but a processing window of up to ~24h), persists
    /// the batch id, and reconciles the result on a later launch or idle tick —
    /// decoupled from the interactive refresh path (#655/#955), which is unchanged.
    /// Off by default: the manuscript only ever goes to the batch endpoint on this
    /// explicit opt-in *and* an explicit run-action. A field default keeps an
    /// existing config that never mentions the key at the safe off state.
    #[serde(default = "default_background_batch_update")]
    pub background_batch_update: bool,
    /// Opt-in for "verify on idle" auto-recheck (#1053). When true, findings whose
    /// chapters changed since they were generated are re-checked on the idle tick
    /// and cleared if the edit resolved them — the same reconcile a manual
    /// re-check-all (#830) does, but automatic and scoped to only the changed
    /// chapters (content-digest keyed, like the incremental bible cache #90). It
    /// re-runs the existing analysis passes, writes no prose, never auto-dismisses a
    /// finding the author is still working, and does no whole-manuscript
    /// re-analysis. Off by default: an author enables it deliberately with `[ai]
    /// verify-on-idle = true`. A field default keeps an existing config that never
    /// mentions the key at the safe off state.
    #[serde(default = "default_verify_on_idle")]
    pub verify_on_idle: bool,
    /// The turn cap for the offline bible-grounded Q&A retrieval loop (#856) — how
    /// many read/search tool turns the on-device model may take before it must
    /// answer with what it gathered. Deliberately larger than the assistant pane's
    /// fixed cap of 8: each on-device turn is free and slow-but-correct is the
    /// accepted trade for an answer that otherwise can't exist offline. Defaults to
    /// [`retrieval::RETRIEVAL_MAX_TURNS`](crate::ai::retrieval::RETRIEVAL_MAX_TURNS);
    /// the cloud assistant loop's cap is unaffected.
    #[serde(default = "default_retrieval_max_turns")]
    pub retrieval_max_turns: usize,
    /// Memoized OS-keychain read for the Claude key, so the store is consulted at
    /// most once per session rather than on every palette-gating check (and, on a
    /// changed dev binary, prompts at most once). Not config — skipped on load and
    /// reset whenever settings are reloaded (a fresh `AiConfig`). Only populated
    /// when the env var is absent, so an env key never triggers the read.
    /// `pub(crate)` only so tests can build an `AiConfig { .. }` literal; treat it
    /// as private (write nothing but `OnceCell::new()`).
    #[serde(skip)]
    pub keychain_cache: OnceCell<Option<String>>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: BackendKind::Claude,
            routing: Policy::Configured,
            local_models: Vec::new(),
            offline: false,
            model: default_ai_model(),
            embed_model: String::new(),
            api_key: None,
            cost_threshold: 0.20,
            refresh_after_changes: 5,
            echo_detection: true,
            background_batch_update: false,
            verify_on_idle: false,
            retrieval_max_turns: default_retrieval_max_turns(),
            keychain_cache: OnceCell::new(),
        }
    }
}

/// The default offline Q&A retrieval turn cap (#856) — the module constant, so the
/// config default can't drift from the loop's own default.
const fn default_retrieval_max_turns() -> usize {
    crate::ai::retrieval::RETRIEVAL_MAX_TURNS
}

impl AiConfig {
    /// Load the `[ai]` section from the home config file. Best-effort: a missing or
    /// unreadable file yields the defaults (AI off). The key itself is resolved
    /// lazily in `resolved_key` (defined in the `ai` module) — env var → OS keychain
    /// → the plaintext `api_key` here (#530) — so this just parses the file.
    pub fn load() -> Self {
        load_section("ai")
    }
}

/// Image-generation settings from the `[image]` config section. Off by default;
/// its own opt-in and key, separate from the text `[ai]` settings.
///
/// Data only — the behavior (`resolved_key`, `backend`, `check_ready`) lives on it
/// in the `ai::image` module (#809), which re-exports it as `ImageSettings`.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ImageConfig {
    /// Opt-in for image generation.
    pub enabled: bool,
    /// Which provider to use (`remote` or `local`).
    pub backend: ImageBackendKind,
    /// Privacy switch: when true, force the local backend so nothing ever leaves
    /// the machine, regardless of `backend`.
    pub offline: bool,
    /// The remote provider API key (env `FAL_KEY` overrides this, see
    /// `ImageConfig::resolved_key` in the `ai::image` module). Unused by the local
    /// backend.
    pub api_key: Option<String>,
    /// The remote provider model id (a fal.ai model, e.g. `fal-ai/flux/schnell`).
    pub model: String,
    /// Memoized OS-keychain read for the fal.ai key (mirrors [`AiConfig`]'s cache):
    /// consulted at most once per session, only when the env var is absent. Not
    /// config — skipped on load, reset on reload. `pub(crate)` only so tests can
    /// build the struct literal; treat as private.
    #[serde(skip)]
    pub keychain_cache: OnceCell<Option<String>>,
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: ImageBackendKind::Remote,
            offline: false,
            api_key: None,
            model: default_image_model(),
            keychain_cache: OnceCell::new(),
        }
    }
}

impl ImageConfig {
    /// Load the `[image]` section from the home config file. Best-effort (a
    /// missing/unreadable file yields the defaults, off). The key is resolved
    /// lazily in `resolved_key` (defined in the `ai::image` module) — env → keychain
    /// → plaintext (#530) — so this just parses the file.
    pub fn load() -> Self {
        load_section("image")
    }
}
