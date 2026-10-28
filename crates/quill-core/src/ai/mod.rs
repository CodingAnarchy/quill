//! The `quill-ai` backend: the seam between Quill's AI features and a provider.
//! Claude (HTTP) is the only backend today; a local model lands behind the same
//! trait later (#83).
//!
//! The trait is **deliberately synchronous** — the TUI loop is synchronous, so a
//! request runs on a worker thread behind a channel either way, and a sync trait
//! also fits a CPU/GPU-bound local model without `spawn_blocking`. See
//! `docs/ai-features.md`. AI is opt-in and off by default: the manuscript only
//! leaves the machine on an explicit request, with a key supplied by the author.

pub mod accuracy;
mod action;
pub mod ad_creative;
pub mod batch;
pub mod bible;
pub mod brainstorm;
pub mod character;
mod claude;
pub mod config_gen;
#[cfg(feature = "local")] // constrained decoding only drives the local backend (#406)
pub mod constrain;
pub mod continuity;
pub mod cooccurrence;
pub mod cost;
pub mod editorial;
pub mod embed;
pub mod factcheck; // research-notebook fact-check (#1049)
pub mod fal;
mod finding;
pub mod foreshadow;
mod http;
pub mod image;
pub mod import;
pub mod incremental;
pub mod keychain;
#[cfg(feature = "local")]
pub mod local;
#[cfg(feature = "local")]
pub mod local_image;
pub mod marketing;
pub mod model_select;
pub mod namegen;
pub mod naming;
pub mod pacing;
pub mod plot;
pub mod relationship;
pub mod retrieval;
pub mod router;
pub mod scaffold;
pub mod scene_brief;
pub mod setting;
pub mod social;
pub mod style;
pub mod submission;
pub mod theme;
pub mod thesaurus;
pub mod tool_loop;
pub mod voice;
pub mod workflow_gen;

pub use action::{AiAction, AiCost, ModelWeight, Needs};
pub use character::Character;
pub use claude::ClaudeBackend;
pub use continuity::Continuity;
pub use finding::{Finding, FindingKind};
pub use plot::Plot;
pub use scaffold::Scaffold;
pub use setting::Setting;
pub use submission::Submission;
pub use theme::Theme;

use std::fmt;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// A provider that turns a [`Request`] into a [`Response`] — the Claude HTTP
/// client or the local candle model. `Send + Sync` so a loaded backend can be
/// cached in an `Arc` and shared with the worker thread.
pub trait Backend: Send + Sync {
    fn complete(&self, req: &Request) -> Result<Response, AiError>;

    /// Run a request through the provider's batch path (#176) — half price, higher
    /// latency — for the occasional full-manuscript rebuild. The default just runs
    /// it normally, so a backend with no batch support (the local model) and the
    /// non-batch path both keep working.
    fn complete_batched(&self, req: &Request) -> Result<Response, AiError> {
        self.complete(req)
    }

    /// Submit `req` to the provider's Message Batches API and return the batch id,
    /// *without waiting* for it to finish (#950). This is the non-blocking half of
    /// the old polling `complete_batched`: the background bible update submits here,
    /// persists the returned id, and reconciles later ([`reconcile_batch`]) across
    /// idle ticks and sessions rather than blocking a worker thread for hours.
    /// The default errors — only a backend with an async batch endpoint (Claude)
    /// implements it, so the local model and any non-batch backend refuse cleanly.
    ///
    /// [`reconcile_batch`]: Backend::reconcile_batch
    fn submit_batch(&self, _req: &Request) -> Result<String, AiError> {
        Err(AiError::NotConfigured(
            "this backend has no batch API — the background bible update needs the \
             Claude (cloud) backend"
                .into(),
        ))
    }

    /// Poll a previously-[`submit_batch`](Backend::submit_batch)ted batch *once*
    /// (no internal loop, #950) and report where it is: still processing, ended with
    /// a parsed result, or ended in a failure/expiry. This is the reconcile half of
    /// the split — driven from the UI's idle/launch cadence, so a batch that takes
    /// hours is checked without ever blocking a worker thread on a poll loop. An
    /// `AiError` here is a genuine transport/API fault fetching the status, distinct
    /// from a batch that *ended* in failure ([`BatchStatus::Failed`]). The default
    /// errors — only the batch backend implements it.
    fn reconcile_batch(&self, _batch_id: &str) -> Result<BatchStatus, AiError> {
        Err(AiError::NotConfigured(
            "this backend has no batch API".into(),
        ))
    }

    /// This backend as the assistant's tool-use transport (#310), when it supports
    /// one. Lets the assistant get a [`Converse`](tool_loop::Converse) from a
    /// backend built through the shared [`Backend`] machinery, no downcast. `None`
    /// for a backend with no tool-use path.
    fn as_converse(&self) -> Option<&dyn tool_loop::Converse> {
        None
    }
}

/// One completion request. `cached_context` is the large, slowly-changing prefix
/// (the manuscript) sent as a cacheable block; `user` is the prompt that follows.
pub struct Request {
    pub system: Option<String>,
    pub cached_context: String,
    pub user: String,
    pub model: String,
    pub max_tokens: u32,
    /// When set, the backend constrains the reply to this JSON Schema and the
    /// response text is guaranteed-valid JSON for the caller to deserialize.
    /// `None` for a free-form reply.
    pub json_schema: Option<serde_json::Value>,
}

/// A completion result: the answer text and token usage.
pub struct Response {
    pub text: String,
    pub usage: Usage,
}

/// Where a submitted batch stands when [`reconcile_batch`](Backend::reconcile_batch)
/// polls it (#950) — the three graceful outcomes the background bible update must
/// handle. Distinct from an [`AiError`], which is a fault *fetching* the status; a
/// batch that runs its course but the request inside it errored/expired is a
/// [`Failed`](BatchStatus::Failed), surfaced (never silently swallowed) so the
/// author knows and can re-queue.
pub enum BatchStatus {
    /// The batch hasn't finished — check again on a later tick / launch. Never a
    /// "timed out" (the #955 bug); the update stays queued until it truly ends.
    Processing,
    /// The batch ended and its single request succeeded — the parsed rebuild
    /// response, ready to apply to the bible.
    Ready(Response),
    /// The batch ended, but its request didn't succeed (errored, was canceled, or
    /// expired past the provider's window) — carries a human reason to surface.
    Failed(String),
}

/// The result of a backend task, delivered from the worker thread to the UI. One
/// variant per AI feature so [`crate::app::App`] can dispatch on what completed.
pub enum Outcome {
    /// "Ask the manuscript" Q&A (#73).
    Answer(Result<Response, AiError>),
    /// Character extraction (#74).
    Characters(Result<Vec<Character>, AiError>),
    /// Timeline & continuity check (#75).
    Continuity(Result<Continuity, AiError>),
    /// Setting & worldbuilding extraction + rule checks (#77).
    Setting(Result<Setting, AiError>),
    /// Plot-thread / arc tracking + dangling-thread findings (#606).
    Plot(Result<Plot, AiError>),
    /// Theme / motif tracking + dangling-motif findings (#828).
    Theme(Result<Theme, AiError>),
    /// Scaffolding: beat outline + structural gaps (#76). Writes `bible/outline.md`
    /// (a bible artifact) *and* surfaces the gaps as findings, so it keeps its own
    /// variant + apply — it isn't analysis-only like the [`Findings`](Self::Findings)
    /// passes, which write nothing.
    Scaffold(Result<Scaffold, AiError>),
    /// A completed analysis-only findings pass (#563): the UI findings the pass
    /// produced, already mapped from its typed result *and attributed to its
    /// originating action* by the worker (each [`Finding`](Finding) carries
    /// `action: Some(_)`), so one generic apply path (`show_findings`) serves every
    /// analysis-only pass — naming, style, editorial, voice, accuracy, relationship,
    /// foreshadow, factcheck, pacing. Collapses the per-pass `Outcome` variants + their
    /// `error()` arms + `apply_outcome` dispatch arms into this one. The persistent-bible
    /// passes (characters/continuity/setting/plot/theme) and scaffold keep their own
    /// variants — they write a bible file, not just findings.
    Findings(Result<Vec<Finding>, AiError>),
    /// Synopsis / blurb / query-letter generation (#81).
    Submission(Result<Submission, AiError>),
    /// Story brainstorming from a premise (#255).
    Brainstorm(Result<brainstorm::Brainstorm, AiError>),
    /// Platform-ready ad creative & copy (#251).
    AdCreative(Result<ad_creative::AdCreative, AiError>),
    /// Marketing copy — blurb variants & taglines (#236).
    Marketing(Result<marketing::Marketing, AiError>),
    /// Social posts — per-platform promo copy (#237).
    Social(Result<social::Social, AiError>),
    /// Generated images saved to disk — character portraits (#238).
    Image(Result<image::ImageReport, AiError>),
    /// A workflow generated from a plain-language description (#232).
    Workflow(Result<workflow_gen::GeneratedWorkflow, AiError>),
    /// Configuration changes proposed from a plain-language description (#303).
    Config(Result<config_gen::GeneratedConfig, AiError>),
    /// A story bible extracted from imported notes (#283).
    Import(Result<import::Imported, AiError>),
    /// In-context thesaurus / word search candidates (#290).
    Thesaurus(Result<Vec<thesaurus::Word>, AiError>),
    /// Name-generator candidates — character / place / term (#610).
    NameGen(Result<Vec<namegen::Name>, AiError>),
    /// Semantic "find similar scenes" (#924): the passages most similar to a
    /// selection / query, ranked with scores + jumpable citations. `None` when no
    /// embedding model is available (graceful degradation, not an error).
    SemanticFind(Result<Option<Vec<embed::Hit>>, AiError>),
    /// Echo / near-duplicate detection (#920): near-duplicate + echoed passage
    /// pairs, each with both citations + a score. `None` when no embedding model is
    /// available (graceful degradation, not an error).
    Echo(Result<Option<Vec<embed::EchoPair>>, AiError>),
    /// Re-check of a single triage finding (#608): its originating pass re-run
    /// scoped to the finding's chapters, parsed to the fresh UI findings that
    /// pass now produces (so the one being rechecked is cleared if it's gone, or
    /// kept if it's still flagged). Analysis-only — it writes no bible file.
    Recheck(Result<Vec<Finding>, AiError>),
    /// Re-check of every open triage finding in one pass (#830): each open
    /// finding's originating pass re-run scoped to its chapters (grouped by
    /// pass + chapters to avoid redundant runs), parsed to the combined fresh UI
    /// findings across all groups (so each rechecked finding is cleared if it's
    /// gone or kept if still flagged). Analysis-only — it writes no bible file.
    RecheckAll(Result<Vec<Finding>, AiError>),
    /// On-demand scope-to-selection pass (#1054): a continuity/setting pass the
    /// author invoked over an editor selection / chapter range, re-run scoped to
    /// just those chapters and parsed to the same kind of findings the full pass
    /// produces — surfaced fresh (not reconciled against one flagged finding like
    /// [`Recheck`](Self::Recheck)). Analysis-only; narrows input, never the output
    /// contract, and writes no bible file.
    ScopedPass(Result<Vec<Finding>, AiError>),
    /// A failure that isn't tied to a specific feature — e.g. the backend
    /// couldn't be built (local model failed to load). Surfaced as a message.
    Error(AiError),
}

impl Outcome {
    /// The error this outcome carries, if it represents a failure — a per-feature
    /// request/parse error or a backend-build [`Outcome::Error`]. Lets the
    /// workflow runner (#227) treat a failed AI step as a step error that stops
    /// the run, without inspecting each variant at the call site.
    pub fn error(&self) -> Option<&AiError> {
        match self {
            Self::Answer(r) => r.as_ref().err(),
            Self::Characters(r) => r.as_ref().err(),
            Self::Continuity(r) => r.as_ref().err(),
            Self::Setting(r) => r.as_ref().err(),
            Self::Plot(r) => r.as_ref().err(),
            Self::Theme(r) => r.as_ref().err(),
            Self::Scaffold(r) => r.as_ref().err(),
            Self::Submission(r) => r.as_ref().err(),
            Self::Brainstorm(r) => r.as_ref().err(),
            Self::AdCreative(r) => r.as_ref().err(),
            Self::Marketing(r) => r.as_ref().err(),
            Self::Social(r) => r.as_ref().err(),
            Self::Image(r) => r.as_ref().err(),
            Self::Workflow(r) => r.as_ref().err(),
            Self::Config(r) => r.as_ref().err(),
            Self::Import(r) => r.as_ref().err(),
            Self::Thesaurus(r) => r.as_ref().err(),
            Self::NameGen(r) => r.as_ref().err(),
            Self::SemanticFind(r) => r.as_ref().err(),
            Self::Echo(r) => r.as_ref().err(),
            // Every analysis-only findings pass (#563) plus the re-check / scoped-pass
            // variants all carry a `Result<Vec<Finding>, _>`, one arm.
            Self::Findings(r) | Self::Recheck(r) | Self::RecheckAll(r) | Self::ScopedPass(r) => {
                r.as_ref().err()
            }
            Self::Error(e) => Some(e),
        }
    }
}

/// Whether an HTTP status is 2xx — shared by the HTTP backends.
pub fn is_ok(status: u16) -> bool {
    (200..300).contains(&status)
}

/// Render a list of items as numbered Markdown sections (`## {label} N`), each
/// trimmed, with a blank line around it — the shared shape for off-manuscript copy
/// artifacts (marketing variants #236, social posts #237). Empty input → empty.
pub fn numbered_sections(label: &str, items: &[String]) -> String {
    let mut s = String::new();
    for (i, it) in items.iter().enumerate() {
        let _ = write!(s, "## {label} {}\n\n{}\n\n", i + 1, it.trim());
    }
    s
}

/// Render a `(token, description)` vocabulary as cached context for a constrained
/// generation request — `heading` then one `- token: desc` line each. Shared by
/// describe-a-workflow (#232) and describe-a-config (#303).
pub fn vocab_context(heading: &str, vocab: &[(String, String)]) -> String {
    let mut s = format!("{heading}\n");
    for (token, desc) in vocab {
        let _ = writeln!(s, "- {token}: {desc}");
    }
    s
}

/// Token usage reported by the backend (zeros when unknown).
#[derive(Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
}

/// Why a request couldn't be completed (#319).
#[derive(Debug)]
pub enum AiError {
    /// AI is disabled, offline, or no key is configured — nothing was sent.
    NotConfigured(String),
    /// The HTTP call failed, the server returned an error status, or a remote
    /// operation failed or didn't finish (a batch erroring out or running past
    /// the time limit). `status` is `Some(code)` when the server responded with
    /// an HTTP error, `None` when there's no status to report (a failed
    /// connection, or a remote operation that never produced one).
    Network {
        status: Option<u16>,
        message: String,
    },
    /// The server responded, but the body wasn't the shape we expected — an
    /// unreadable body, a missing field, or an empty completion.
    ApiMalformed(String),
    /// The model's reply didn't satisfy the requested JSON schema.
    ModelNonCompliance(String),
    /// A failure on our side: a poisoned lock or a local-inference error. Only
    /// the `local` backends produce it, so it's unconstructed in default builds.
    #[cfg_attr(not(feature = "local"), allow(dead_code))]
    Internal(String),
}

impl AiError {
    /// Whether this failure is worth retrying on an *idempotent single request*:
    /// a transient condition a bounded backoff can ride out. A dropped connection
    /// (`Network { status: None }`, where the request may never have reached the
    /// server) and the retryable HTTP statuses (see [`retryable_status`]) qualify.
    /// Everything else — a definite error status, a malformed body, a
    /// non-compliant model, a config problem, an internal fault — is permanent:
    /// re-sending would only re-pay for the same failure. (#697)
    pub const fn retryable(&self) -> bool {
        match self {
            Self::Network { status: None, .. } => true,
            Self::Network {
                status: Some(code), ..
            } => retryable_status(*code),
            _ => false,
        }
    }
}

/// Whether an HTTP status is a *transient* server condition worth retrying on an
/// idempotent request: 429 (rate limited) and 529 (overloaded) are the two the
/// provider explicitly marks retryable. Everything else (other 5xx included) is
/// treated as permanent here — we only retry what's known-safe. (#697)
pub const fn retryable_status(status: u16) -> bool {
    matches!(status, 429 | 529)
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured(m) => write!(f, "{m}"),
            Self::Network {
                status: Some(code),
                message,
            } => {
                write!(f, "network error ({code}): {message}")
            }
            Self::Network {
                status: None,
                message,
            } => write!(f, "network error: {message}"),
            Self::ApiMalformed(m) => write!(f, "malformed API response: {m}"),
            Self::ModelNonCompliance(m) => write!(f, "model did not comply: {m}"),
            Self::Internal(m) => write!(f, "internal error: {m}"),
        }
    }
}

/// Output bound shared by the structured analysis passes (continuity, setting,
/// scaffold, character, style, submission). The cheap naming pass overrides it
/// with its own smaller bound. One source of truth so the cost estimate and the
/// real request agree.
pub const MAX_OUTPUT_TOKENS: u32 = 8192;

/// Build the request shared by the manuscript-extraction (and concept-draft)
/// passes (#313): a system prompt, a one-line user instruction, the text as
/// cached context, the model, the default output bound, and the JSON schema.
/// Passes with extra inputs (naming, voice, ad-creative, thesaurus) keep their
/// own builders.
pub fn manuscript_request(
    system: &str,
    user: &str,
    cached_context: String,
    model: &str,
    schema: serde_json::Value,
) -> Request {
    Request {
        system: Some(system.to_string()),
        cached_context,
        user: user.to_string(),
        model: model.to_string(),
        max_tokens: MAX_OUTPUT_TOKENS,
        json_schema: Some(schema),
    }
}

/// Deserialize a structured AI reply, wrapping a parse failure in a uniform
/// `bad {kind} JSON` message (#313) so every feature reports it the same way. A
/// failure here means the model didn't honor the requested schema (#319).
pub fn parse_json<T: serde::de::DeserializeOwned>(text: &str, kind: &str) -> Result<T, AiError> {
    serde_json::from_str(text)
        .map_err(|e| AiError::ModelNonCompliance(format!("bad {kind} JSON: {e}")))
}

/// The JSON-Schema fragment for an array of strings — shared by the feature
/// schemas that previously each re-declared it (#313).
pub fn string_array() -> serde_json::Value {
    serde_json::json!({ "type": "array", "items": { "type": "string" } })
}

/// `{ "type": "string" }` — the most common schema leaf (#429).
pub fn string_field() -> serde_json::Value {
    serde_json::json!({ "type": "string" })
}

/// `{ "type": "array", "items": <item> }` — an array of `item` (#429).
// takes ownership: `item` is moved into the constructed JSON value
#[allow(clippy::needless_pass_by_value)]
pub fn array_of(item: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "type": "array", "items": item })
}

/// The `{ "type": "object", "properties": …, "required": …, "additionalProperties":
/// false }` envelope every feature schema shares (#429). `required` is omitted
/// when empty, so an all-optional object matches a hand-written one that left the
/// key out (constrained decoding is unchanged).
pub fn object_schema(
    props: Vec<(&str, serde_json::Value)>,
    required: &[&str],
) -> serde_json::Value {
    let properties: serde_json::Map<String, serde_json::Value> =
        props.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false,
    });
    if !required.is_empty() {
        schema["required"] = serde_json::json!(required);
    }
    schema
}

/// The common "finding" item — two string fields plus the involved `chapters`
/// (#429), e.g. `kind`/`detail` or `rule`/`violation`. All three keys required.
pub fn chapters_finding(field_a: &str, field_b: &str) -> serde_json::Value {
    object_schema(
        vec![
            (field_a, string_field()),
            (field_b, string_field()),
            ("chapters", string_array()),
        ],
        &[field_a, field_b, "chapters"],
    )
}

/// Which provider the AI features run against (#87). The config *value* lives in
/// [`config::BackendKind`](crate::config::BackendKind) alongside the rest of the
/// `[ai]` data (#809); re-exported here so `ai::BackendKind` is unchanged for
/// callers.
pub use crate::config::BackendKind;

/// AI configuration data, loaded from the user's home config. The struct and its
/// `[ai]` fields live in [`config::AiConfig`](crate::config::AiConfig) so `config`
/// is the single typed home for every section (#809); re-exported here as
/// `ai::Settings` so existing callers are unchanged. Its *behavior*
/// ([`resolved_key`](Settings::resolved_key), [`backend`](Settings::backend),
/// [`check_ready`](Settings::check_ready)) is defined below as inherent impls on
/// that config-owned type — an inherent impl may live in any module of the crate
/// that defines the type — so the keychain/router logic stays here in `ai` and the
/// dependency direction stays `ai → config` (config gains no dependency on `ai`).
pub use crate::config::AiConfig as Settings;

impl Settings {
    /// The resolved Claude API key, if any: `ANTHROPIC_API_KEY` → OS keychain →
    /// the plaintext `api_key` in config (#530). The env var is the headless / CI
    /// escape hatch; the keychain keeps the secret out of the synced config; the
    /// plaintext field keeps existing setups working and is the migration source.
    /// Test builds read neither the env var nor the keychain (#528/#531), so the
    /// suite can't authenticate, and a default config resolves to `None`.
    pub fn resolved_key(&self) -> Option<String> {
        // The env key is the headless escape hatch in production; test builds never
        // read it, so the suite can't reach the API by accident (#528).
        #[cfg(any(test, feature = "test-support"))]
        let env: Option<String> = None;
        #[cfg(not(any(test, feature = "test-support")))]
        let env = std::env::var("ANTHROPIC_API_KEY").ok();
        // `resolve_lazy` only calls the thunk when the env var is absent, so an env
        // key still skips the keychain; the thunk memoizes the read so repeated
        // gating checks consult (and, on a changed binary, prompt for) it at most
        // once. `api_key`/env stay live — only the keychain read is cached.
        keychain::resolve_lazy(
            env.as_deref(),
            || {
                self.keychain_cache
                    .get_or_init(|| keychain::get(keychain::Key::Anthropic))
                    .clone()
            },
            self.api_key.as_deref(),
        )
    }

    /// The backend that will actually run, accounting for the privacy switch:
    /// `offline` forces local; otherwise the configured `backend`.
    pub const fn backend(&self) -> BackendKind {
        if self.offline {
            BackendKind::Local
        } else {
            self.backend
        }
    }

    /// Whether the AI features are usable as configured, or why not — the
    /// readiness chokepoint for the *active* backend (gates the palette and each
    /// request). A thin wrapper over [`ready_for`](Self::ready_for) for the
    /// backend [`backend`](Self::backend) resolves to.
    pub fn check_ready(&self) -> Result<(), AiError> {
        self.ready_for(self.backend())
    }

    /// Whether `backend` specifically is usable, or why not. Local is always
    /// ready (it loads on first use); Claude needs an API key. The hybrid router
    /// (#372) checks the backend it actually picked, which may differ from the
    /// configured one.
    pub fn ready_for(&self, backend: BackendKind) -> Result<(), AiError> {
        if !self.enabled {
            return Err(AiError::NotConfigured(
                "AI is off — run \"Enable AI assistance\" from the run-action menu (Ctrl-K), \
                 or set `enabled = true` under [ai] in ~/.config/quill/config.toml"
                    .into(),
            ));
        }
        match backend {
            BackendKind::Local => Ok(()),
            BackendKind::Claude if self.resolved_key().is_some() => Ok(()),
            BackendKind::Claude => Err(AiError::NotConfigured(
                "no API key — run \"Set Claude API key\" from the run-action menu, set \
                 ANTHROPIC_API_KEY, or set `backend = \"local\"`"
                    .into(),
            )),
        }
    }
}

/// System prompt for grounded manuscript Q&A.
const QA_SYSTEM: &str = "You are a writing assistant answering questions about a \
novel manuscript supplied by the user. Answer only from the manuscript. Put the \
answer prose in `answer`, and list in `citations` the exact chapter titles you \
drew the answer from (so the author can jump to them) — an empty list when the \
answer names no chapter or the manuscript does not contain the answer. If the \
manuscript does not contain the answer, say so plainly in `answer` rather than \
guessing.";

/// System prompt for the offline bible-grounded retrieval loop (#856). Unlike
/// [`QA_SYSTEM`], the manuscript is NOT in context — the model gathers it by calling
/// read-only lookup tools (read a chapter, search the manuscript, read a bible
/// section). It answers only from what it read and cites the chapters it drew from.
/// It has no tool that writes prose, and must never draft, rewrite, or produce
/// manuscript text — only answer *about* the manuscript.
pub const QA_SYSTEM_RETRIEVAL: &str = "You are a writing assistant answering a question \
about a novel manuscript that is too large to read all at once. You are seeded with a \
compact story bible and the chapter list. Gather what you need with the read-only tools \
— read:chapter to read a chapter's prose, search:manuscript to find where a phrase \
occurs, read:bible to read a bible section, list:chapters to list the chapters — then \
answer ONLY from what you read. Put the answer prose in `answer` and list in \
`citations` the exact chapter titles you drew from (so the author can jump to them). If \
what you gathered does not contain the answer, say so plainly rather than guessing. You \
have no tool that writes or edits prose: never draft, rewrite, or produce manuscript \
text — only answer about the manuscript.";

/// A grounded Q&A reply (#609): the answer prose plus the chapter titles it was
/// drawn from, so the author can jump to each cited chapter (like a
/// [`Finding`](Finding)). The AI answers *about* the manuscript — it
/// never writes into it; `citations` are retrieval, not prose.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Answer {
    /// The answer prose, grounded in the manuscript.
    pub answer: String,
    /// Chapter titles the answer draws from — jumpable citations. May be empty.
    #[serde(default)]
    pub citations: Vec<String>,
    /// For an offline read-loop answer (#1056): metadata about *what the loop read*
    /// and *how completely* it covered the book — the read-set and a coverage sense,
    /// so a holistic answer stays hedged rather than presented as exhaustive. `None`
    /// for the whole-book (cloud) path, which reads the entire manuscript. This is
    /// metadata about the read; it never adds or alters the answer prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
}

/// Coverage metadata for an offline read-loop Q&A answer (#1056): a structured,
/// answer-prose-independent record of *what the loop read* and *how complete* that
/// coverage was, so the disclosure ("answered from 6 of 40 chapters; may be
/// incomplete") is testable on its own, separate from the answer text.
///
/// The loop answers from the chapters it *chose* to read, not the whole book — this
/// captures that read-set and the reasons coverage may be partial (no story bible to
/// seed from; the retrieval turn cap reached before finishing). It is metadata about
/// the read: it never adds or alters manuscript prose.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Coverage {
    /// The chapters the loop actually read or searched — the read-set, deduped and
    /// in encounter order. These are the same chapter titles as [`Answer::citations`]:
    /// the ground truth of what the answer could draw from.
    #[serde(default)]
    pub chapters_read: Vec<String>,
    /// How many chapters the manuscript has in total, so `chapters_read.len()` of
    /// `chapters_total` reads as a coverage fraction ("6 of 40").
    #[serde(default)]
    pub chapters_total: usize,
    /// No story bible was available to seed the loop, so it ran search-only and the
    /// diffuse global structure a bible carries was missing — coverage may be partial.
    #[serde(default)]
    pub no_bible: bool,
    /// The retrieval turn cap was reached before the loop could finish gathering, so
    /// the answer is from what it managed to read — coverage is incomplete.
    #[serde(default)]
    pub cap_reached: bool,
}

impl Coverage {
    /// Whether this answer should be hedged: the loop did not read the whole book, or
    /// it lacked a bible, or it hit the turn cap. True unless the read-set covered
    /// every chapter with a full, uninterrupted read — the calibration the author
    /// needs so a holistic answer isn't trusted as exhaustive (#1056).
    #[must_use]
    pub const fn is_hedged(&self) -> bool {
        self.no_bible || self.cap_reached || self.chapters_read.len() < self.chapters_total
    }
}

/// The JSON Schema for a grounded Q&A reply (#609): the answer prose plus the
/// cited chapter titles.
pub fn qa_schema() -> serde_json::Value {
    object_schema(
        vec![("answer", string_field()), ("citations", string_array())],
        &["answer", "citations"],
    )
}

/// Build a Q&A request: the manuscript as the cacheable context, `question` as
/// the prompt, constrained to the [`qa_schema`] so the reply carries jumpable
/// chapter citations (#609).
///
/// `manuscript` is either the whole active book (the default, unchanged path) or —
/// when the semantic index is available — just the retrieved passages under their
/// chapter titles ([`retrieved_context`], #831). Either way it's the same request
/// shape and the same schema, so there's one Q&A path: retrieval changes only *what
/// context* is fed, not how the answer is produced or how its citations jump. The
/// `QA_SYSTEM` prompt still asks the model to cite the exact chapter titles it drew
/// from, so a retrieved passage's chapter stays a jumpable citation.
pub fn build_qa_request(manuscript: String, question: &str, model: &str) -> Request {
    Request {
        system: Some(QA_SYSTEM.to_string()),
        cached_context: manuscript,
        user: question.to_string(),
        model: model.to_string(),
        max_tokens: 1024,
        json_schema: Some(qa_schema()),
    }
}

/// Serialize retrieved passages ([`embed::Hit`]) into the manuscript-shaped context
/// [`build_qa_request`] feeds (#831): each passage under its chapter title heading,
/// most-relevant first, exactly the `# Title\n\n<prose>` shape the whole-book render
/// uses — so the model cites the same chapter titles and every citation stays
/// jumpable. Consecutive hits from the same chapter share one heading. This is the
/// retrieval step that lets Ask scale past one book's window: instead of the whole
/// book, only the passages the query actually pulled up are sent, with their
/// citations preserved. Reads prose to retrieve + cite it; never writes it.
///
/// Its only non-test caller is the `local`-gated Ask retrieval step (retrieval
/// needs the candle embedder), so it's unreachable in a default non-test build;
/// the scoped allow silences that without deleting logic the tests + `local` build
/// use (#406).
#[cfg_attr(not(feature = "local"), allow(dead_code))]
pub fn retrieved_context(hits: &[embed::Hit]) -> String {
    let mut out = String::new();
    let mut last_title: Option<&str> = None;
    for hit in hits {
        if last_title != Some(hit.passage.title.as_str()) {
            out.push_str("# ");
            out.push_str(&hit.passage.title);
            out.push_str("\n\n");
            last_title = Some(hit.passage.title.as_str());
        }
        out.push_str(hit.passage.text.trim());
        out.push_str("\n\n");
    }
    out
}

/// Parse a Q&A reply into an [`Answer`] (#609). The schema guarantees the JSON
/// shape, but a backend that ignores it (or a mock returning free text) falls
/// back to treating the whole reply as the answer with no citations, so the Q&A
/// flow never hard-fails on a well-meaning but unstructured reply.
pub fn parse_answer(text: &str) -> Answer {
    serde_json::from_str::<Answer>(text).unwrap_or_else(|_| Answer {
        answer: text.to_string(),
        citations: Vec::new(),
        coverage: None,
    })
}

#[cfg(test)]
mod tests;
