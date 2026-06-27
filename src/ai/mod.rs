//! The `quill-ai` backend: the seam between Quill's AI features and a provider.
//! Claude (HTTP) is the only backend today; a local model lands behind the same
//! trait later (#83).
//!
//! The trait is **deliberately synchronous** — the TUI loop is synchronous, so a
//! request runs on a worker thread behind a channel either way, and a sync trait
//! also fits a CPU/GPU-bound local model without `spawn_blocking`. See
//! `docs/ai-features.md`. AI is opt-in and off by default: the manuscript only
//! leaves the machine on an explicit request, with a key supplied by the author.

pub mod ad_creative;
pub mod bible;
pub mod brainstorm;
pub mod character;
#[cfg(feature = "local")] // constrained decoding only drives the local backend (#406)
pub mod constrain;
pub mod continuity;
pub mod cost;
pub mod editorial;
pub mod fal;
mod http;
pub mod image;
pub mod import;
pub mod incremental;
#[cfg(feature = "local")]
pub mod local_image;
pub mod marketing;
#[cfg(feature = "local")]
pub mod local;
pub mod model_select;
pub mod naming;
pub mod router;
pub mod scaffold;
pub mod scene_brief;
pub mod setting;
pub mod social;
pub mod style;
pub mod submission;
pub mod config_gen;
pub mod thesaurus;
pub mod tool_loop;
pub mod tools;
pub mod voice;
pub mod workflow_gen;
mod claude;

pub use character::Character;
pub use claude::ClaudeBackend;
pub use continuity::Continuity;
pub use naming::Naming;
pub use scaffold::Scaffold;
pub use setting::Setting;
pub use submission::Submission;
// `style::Style` is referred to qualified (avoids clashing with ratatui's Style).

use std::fmt;
use crate::theme;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde::Deserialize;

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
    /// `None` for free-form replies (Q&A).
    pub json_schema: Option<serde_json::Value>,
}

/// A completion result: the answer text and token usage.
pub struct Response {
    pub text: String,
    pub usage: Usage,
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
    /// Scaffolding: beat outline + structural gaps (#76).
    Scaffold(Result<Scaffold, AiError>),
    /// Naming & term consistency (#78).
    Naming(Result<Naming, AiError>),
    /// Prose & style analysis (#79).
    Style(Result<style::Style, AiError>),
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
    /// Character voice analysis — dialogue/narration consistency (#183).
    Voice(Result<voice::Voice, AiError>),
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
    /// Editorial read — fix-free flags (structural / continuity / line) (#293).
    Editorial(Result<Vec<editorial::Note>, AiError>),
    /// A failure that isn't tied to a specific feature — e.g. the backend
    /// couldn't be built (local model failed to load). Surfaced as a message.
    Error(AiError),
}

impl Outcome {
    /// The error this outcome carries, if it represents a failure — a per-feature
    /// request/parse error or a backend-build [`Outcome::Error`]. Lets the
    /// workflow runner (#227) treat a failed AI step as a step error that stops
    /// the run, without inspecting each variant at the call site.
    pub(crate) fn error(&self) -> Option<&AiError> {
        match self {
            Outcome::Answer(r) => r.as_ref().err(),
            Outcome::Characters(r) => r.as_ref().err(),
            Outcome::Continuity(r) => r.as_ref().err(),
            Outcome::Setting(r) => r.as_ref().err(),
            Outcome::Scaffold(r) => r.as_ref().err(),
            Outcome::Naming(r) => r.as_ref().err(),
            Outcome::Style(r) => r.as_ref().err(),
            Outcome::Submission(r) => r.as_ref().err(),
            Outcome::Brainstorm(r) => r.as_ref().err(),
            Outcome::AdCreative(r) => r.as_ref().err(),
            Outcome::Marketing(r) => r.as_ref().err(),
            Outcome::Social(r) => r.as_ref().err(),
            Outcome::Voice(r) => r.as_ref().err(),
            Outcome::Image(r) => r.as_ref().err(),
            Outcome::Workflow(r) => r.as_ref().err(),
            Outcome::Config(r) => r.as_ref().err(),
            Outcome::Import(r) => r.as_ref().err(),
            Outcome::Thesaurus(r) => r.as_ref().err(),
            Outcome::Editorial(r) => r.as_ref().err(),
            Outcome::Error(e) => Some(e),
        }
    }
}

/// Whether an HTTP status is 2xx — shared by the HTTP backends.
pub(crate) fn is_ok(status: u16) -> bool {
    (200..300).contains(&status)
}

/// A configured key from an optional config string: trimmed, `None` when blank.
/// Shared by the text [`Settings`] and image [`image::ImageSettings`].
pub(crate) fn resolved_key(key: &Option<String>) -> Option<&str> {
    key.as_deref().map(str::trim).filter(|k| !k.is_empty())
}

/// Render a list of items as numbered Markdown sections (`## {label} N`), each
/// trimmed, with a blank line around it — the shared shape for off-manuscript copy
/// artifacts (marketing variants #236, social posts #237). Empty input → empty.
pub(crate) fn numbered_sections(label: &str, items: &[String]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(i, it)| format!("## {label} {}\n\n{}\n\n", i + 1, it.trim()))
        .collect()
}

/// Render a `(token, description)` vocabulary as cached context for a constrained
/// generation request — `heading` then one `- token: desc` line each. Shared by
/// describe-a-workflow (#232) and describe-a-config (#303).
pub(crate) fn vocab_context(heading: &str, vocab: &[(String, String)]) -> String {
    let mut s = format!("{heading}\n");
    for (token, desc) in vocab {
        s.push_str(&format!("- {token}: {desc}\n"));
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
    Network { status: Option<u16>, message: String },
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

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AiError::NotConfigured(m) => write!(f, "{m}"),
            AiError::Network { status: Some(code), message } => {
                write!(f, "network error ({code}): {message}")
            }
            AiError::Network { status: None, message } => write!(f, "network error: {message}"),
            AiError::ApiMalformed(m) => write!(f, "malformed API response: {m}"),
            AiError::ModelNonCompliance(m) => write!(f, "model did not comply: {m}"),
            AiError::Internal(m) => write!(f, "internal error: {m}"),
        }
    }
}

/// The default Claude model (1M context — a whole novel fits in one request).
pub fn default_model() -> String {
    "claude-opus-4-8".to_string()
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
pub(crate) fn manuscript_request(
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
pub(crate) fn parse_json<T: serde::de::DeserializeOwned>(text: &str, kind: &str) -> Result<T, AiError> {
    serde_json::from_str(text)
        .map_err(|e| AiError::ModelNonCompliance(format!("bad {kind} JSON: {e}")))
}

/// The JSON-Schema fragment for an array of strings — shared by the feature
/// schemas that previously each re-declared it (#313).
pub(crate) fn string_array() -> serde_json::Value {
    serde_json::json!({ "type": "array", "items": { "type": "string" } })
}

/// `{ "type": "string" }` — the most common schema leaf (#429).
pub(crate) fn string_field() -> serde_json::Value {
    serde_json::json!({ "type": "string" })
}

/// `{ "type": "array", "items": <item> }` — an array of `item` (#429).
pub(crate) fn array_of(item: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "type": "array", "items": item })
}

/// The `{ "type": "object", "properties": …, "required": …, "additionalProperties":
/// false }` envelope every feature schema shares (#429). `required` is omitted
/// when empty, so an all-optional object matches a hand-written one that left the
/// key out (constrained decoding is unchanged).
pub(crate) fn object_schema(props: Vec<(&str, serde_json::Value)>, required: &[&str]) -> serde_json::Value {
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
pub(crate) fn chapters_finding(field_a: &str, field_b: &str) -> serde_json::Value {
    object_schema(
        vec![(field_a, string_field()), (field_b, string_field()), ("chapters", string_array())],
        &[field_a, field_b, "chapters"],
    )
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

/// AI configuration, loaded from the user's home config — never the project dir,
/// so an unpublished draft and the key stay separate. Off by default.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Master opt-in. When false, no AI feature is reachable.
    pub enabled: bool,
    /// Which backend to use (`claude` or `local`).
    pub backend: BackendKind,
    /// Hybrid routing policy (#372): how to pick local vs remote per action.
    /// Defaults to `configured` (always `backend`), so routing is opt-in.
    pub routing: router::Policy,
    /// The local models the author wants available (#377), by id (`["1.5B",
    /// "7B"]`); the router picks the best of these per request. Empty ⇒ the single
    /// hardware-recommended model, as before.
    pub local_models: Vec<String>,
    /// Privacy switch: when true, force the local backend so nothing ever leaves
    /// the machine, regardless of `backend`.
    pub offline: bool,
    /// Model id sent to the Claude backend (the local backend picks its own).
    pub model: String,
    /// API key, if set in the config file. The `ANTHROPIC_API_KEY` env var
    /// overrides this (see [`Settings::load`]).
    pub api_key: Option<String>,
    /// Confirm before any Claude pass whose estimated cost exceeds this many
    /// dollars. Cheaper passes (and the free local backend) run without a prompt.
    pub cost_threshold: f64,
    /// Rebuild a bible from the whole manuscript (instead of an incremental
    /// revise) once this many chapters have changed since its last pass (#90).
    /// 0 disables incremental updates — every pass is a full rebuild.
    pub refresh_after_changes: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: BackendKind::Claude,
            routing: router::Policy::Configured,
            local_models: Vec::new(),
            offline: false,
            model: default_model(),
            api_key: None,
            cost_threshold: 0.20,
            refresh_after_changes: 5,
        }
    }
}

impl Settings {
    /// Load AI settings from `$XDG_CONFIG_HOME/quill/config.toml` (else
    /// `$HOME/.config/quill/config.toml`). Best-effort: a missing or unreadable
    /// file yields the defaults (AI off). The `ANTHROPIC_API_KEY` env var, if
    /// set, takes precedence over a key in the file.
    pub fn load() -> Self {
        let mut settings: Settings = crate::config::load_section("ai");
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            if !key.trim().is_empty() {
                settings.api_key = Some(key);
            }
        }
        settings
    }

    /// The configured API key, if any (non-empty).
    pub fn resolved_key(&self) -> Option<&str> {
        resolved_key(&self.api_key)
    }

    /// The backend that will actually run, accounting for the privacy switch:
    /// `offline` forces local; otherwise the configured `backend`.
    pub fn backend(&self) -> BackendKind {
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
                "AI is off — set `enabled = true` under [ai] in ~/.config/quill/config.toml".into(),
            ));
        }
        match backend {
            BackendKind::Local => Ok(()),
            BackendKind::Claude if self.resolved_key().is_some() => Ok(()),
            BackendKind::Claude => Err(AiError::NotConfigured(
                "no API key — set ANTHROPIC_API_KEY or `api_key` under [ai] in the config \
                 (or set `backend = \"local\"`)"
                    .into(),
            )),
        }
    }
}

/// System prompt for grounded manuscript Q&A.
const QA_SYSTEM: &str = "You are a writing assistant answering questions about a \
novel manuscript supplied by the user. Answer only from the manuscript, and cite \
the chapter titles you draw your answer from. If the manuscript does not contain \
the answer, say so plainly rather than guessing.";

/// Build a Q&A request: the manuscript as the cacheable context, `question` as
/// the prompt.
pub fn build_qa_request(manuscript: String, question: &str, model: &str) -> Request {
    Request {
        system: Some(QA_SYSTEM.to_string()),
        cached_context: manuscript,
        user: question.to_string(),
        model: model.to_string(),
        max_tokens: 1024,
        json_schema: None,
    }
}

/// Render a completion as overlay lines: the answer, then a dimmed usage line.
/// Long lines are wrapped by the renderer (the overlay enables wrapping).
pub fn answer_to_lines(resp: &Response) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = resp.text.lines().map(|l| Line::raw(l.to_string())).collect();
    if out.is_empty() {
        out.push(Line::raw("(no answer)"));
    }
    out.push(Line::raw(""));
    let u = &resp.usage;
    out.push(Line::from(Span::styled(
        format!(
            "{} tokens in ({} cached) · {} out",
            u.input_tokens, u.cache_read_tokens, u.output_tokens
        ),
        Style::default().fg(theme::current().dim),
    )));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `[ai]` section wrapper — used by the tests that parse a config string
    /// directly. (Loading from disk goes through `config::load_section`, #430.)
    #[derive(Deserialize, Default)]
    struct ConfigFile {
        #[serde(default)]
        ai: Settings,
    }

    #[test]
    fn object_schema_builds_the_envelope() {
        let s = object_schema(vec![("name", string_field()), ("tags", string_array())], &["name"]);
        assert_eq!(s["type"], "object");
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["required"], serde_json::json!(["name"]));
        assert_eq!(s["properties"]["name"], serde_json::json!({ "type": "string" }));
        assert_eq!(s["properties"]["tags"], serde_json::json!({ "type": "array", "items": { "type": "string" } }));
    }

    #[test]
    fn object_schema_omits_required_when_empty() {
        // Matches a hand-written all-optional object that left the key out, so
        // constrained decoding is unchanged (#429).
        let s = object_schema(vec![("note", string_field())], &[]);
        assert!(s.get("required").is_none(), "no empty `required` key: {s}");
    }

    #[test]
    fn chapters_finding_is_two_strings_plus_chapters() {
        let f = chapters_finding("kind", "detail");
        assert_eq!(f["required"], serde_json::json!(["kind", "detail", "chapters"]));
        assert_eq!(f["properties"]["kind"], serde_json::json!({ "type": "string" }));
        assert_eq!(f["properties"]["detail"], serde_json::json!({ "type": "string" }));
        assert_eq!(f["properties"]["chapters"], serde_json::json!({ "type": "array", "items": { "type": "string" } }));
        assert_eq!(f["additionalProperties"], false);
    }

    #[test]
    fn settings_default_to_ai_off() {
        let s = Settings::default();
        assert!(!s.enabled);
        assert_eq!(s.model, "claude-opus-4-8");
        assert!(s.check_ready().is_err());
    }

    #[test]
    fn settings_parse_from_config_toml() {
        let cfg: ConfigFile =
            toml::from_str("[ai]\nenabled = true\nmodel = \"claude-haiku-4-5\"\napi_key = \"sk-x\"\n")
                .unwrap();
        assert!(cfg.ai.enabled);
        assert_eq!(cfg.ai.model, "claude-haiku-4-5");
        assert_eq!(cfg.ai.resolved_key(), Some("sk-x"));
        cfg.ai.check_ready().unwrap(); // enabled + key + online → ready
    }

    #[test]
    fn check_ready_reports_why_not() {
        let mut s = Settings { enabled: true, ..Settings::default() };
        s.api_key = Some("k".into());
        s.check_ready().unwrap(); // Claude + key → ready
        s.api_key = None;
        assert!(s.check_ready().is_err()); // Claude, no key → not ready
    }

    #[test]
    fn local_backend_is_ready_without_a_key_and_offline_forces_it() {
        // Local needs no key.
        let s = Settings { enabled: true, backend: BackendKind::Local, ..Settings::default() };
        assert_eq!(s.backend(), BackendKind::Local);
        s.check_ready().unwrap();
        // Offline forces local even with backend = claude and no key.
        let s = Settings { enabled: true, offline: true, ..Settings::default() };
        assert_eq!(s.backend(), BackendKind::Local);
        s.check_ready().unwrap();
        // Disabled is never ready, regardless of backend.
        let s = Settings { backend: BackendKind::Local, ..Settings::default() };
        assert!(s.check_ready().is_err());
    }

    #[test]
    fn backend_parses_from_config() {
        let cfg: ConfigFile = toml::from_str("[ai]\nenabled = true\nbackend = \"local\"\n").unwrap();
        assert_eq!(cfg.ai.backend, BackendKind::Local);
    }

    #[test]
    fn build_qa_request_carries_manuscript_and_question() {
        let req = build_qa_request("# One\n\nThe locket gleamed.".into(), "where is the locket?", "claude-opus-4-8");
        assert!(req.cached_context.contains("locket gleamed"));
        assert_eq!(req.user, "where is the locket?");
        assert_eq!(req.model, "claude-opus-4-8");
        assert!(req.system.is_some());
    }

    #[test]
    fn answer_to_lines_renders_answer_and_usage() {
        let resp = Response {
            text: "Chapter One introduces the locket.".into(),
            usage: Usage { input_tokens: 1200, output_tokens: 30, cache_read_tokens: 1000 },
        };
        let lines = answer_to_lines(&resp);
        let text: String = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("introduces the locket"));
        assert!(text.contains("1,200 tokens in") || text.contains("1200 tokens in"));
    }
}
