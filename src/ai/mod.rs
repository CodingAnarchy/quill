//! The `quill-ai` backend: the seam between Quill's AI features and a provider.
//! Claude (HTTP) is the only backend today; a local model lands behind the same
//! trait later (#83).
//!
//! The trait is **deliberately synchronous** — the TUI loop is synchronous, so a
//! request runs on a worker thread behind a channel either way, and a sync trait
//! also fits a CPU/GPU-bound local model without `spawn_blocking`. See
//! `docs/ai-features.md`. AI is opt-in and off by default: the manuscript only
//! leaves the machine on an explicit request, with a key supplied by the author.

pub mod bible;
pub mod character;
pub mod continuity;
pub mod naming;
pub mod scaffold;
pub mod setting;
pub mod style;
mod claude;

pub use character::Character;
pub use claude::ClaudeBackend;
pub use continuity::Continuity;
pub use naming::Naming;
pub use scaffold::Scaffold;
pub use setting::Setting;
// `style::Style` is referred to qualified (avoids clashing with ratatui's Style).

use std::fmt;
use std::path::PathBuf;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;

/// A provider that turns a [`Request`] into a [`Response`]. Implemented by the
/// Claude HTTP client now, a local model later.
pub trait Backend: Send {
    fn complete(&self, req: &Request) -> Result<Response, AiError>;
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
}

/// Token usage reported by the backend (zeros when unknown).
#[derive(Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
}

/// Why a request couldn't be completed.
#[derive(Debug)]
pub enum AiError {
    /// AI is disabled, offline, or no key is configured — nothing was sent.
    NotConfigured(String),
    /// The transport/HTTP call failed.
    Network(String),
    /// The API returned an error status or an unparseable body.
    Api(String),
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AiError::NotConfigured(m) => write!(f, "{m}"),
            AiError::Network(m) => write!(f, "network error: {m}"),
            AiError::Api(m) => write!(f, "API error: {m}"),
        }
    }
}

/// The default Claude model (1M context — a whole novel fits in one request).
pub fn default_model() -> String {
    "claude-opus-4-8".to_string()
}

/// AI configuration, loaded from the user's home config — never the project dir,
/// so an unpublished draft and the key stay separate. Off by default.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Master opt-in. When false, no AI feature is reachable.
    pub enabled: bool,
    /// Disable all network calls (the Claude backend) regardless of `enabled`.
    pub offline: bool,
    /// Model id sent to the backend.
    pub model: String,
    /// API key, if set in the config file. The `ANTHROPIC_API_KEY` env var
    /// overrides this (see [`Settings::load`]).
    pub api_key: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            offline: false,
            model: default_model(),
            api_key: None,
        }
    }
}

#[derive(Deserialize, Default)]
struct ConfigFile {
    #[serde(default)]
    ai: Settings,
}

impl Settings {
    /// Load AI settings from `$XDG_CONFIG_HOME/quill/config.toml` (else
    /// `$HOME/.config/quill/config.toml`). Best-effort: a missing or unreadable
    /// file yields the defaults (AI off). The `ANTHROPIC_API_KEY` env var, if
    /// set, takes precedence over a key in the file.
    pub fn load() -> Self {
        let mut settings = config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| toml::from_str::<ConfigFile>(&t).ok())
            .map(|c| c.ai)
            .unwrap_or_default();
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            if !key.trim().is_empty() {
                settings.api_key = Some(key);
            }
        }
        settings
    }

    /// The configured API key, if any (non-empty).
    pub fn resolved_key(&self) -> Option<&str> {
        self.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty())
    }

    /// The API key to send with, or why a request can't be made right now
    /// (disabled, offline, or no key). The single readiness chokepoint.
    pub fn check_ready(&self) -> Result<&str, AiError> {
        if !self.enabled {
            return Err(AiError::NotConfigured(
                "AI is off — set `enabled = true` under [ai] in ~/.config/quill/config.toml".into(),
            ));
        }
        if self.offline {
            return Err(AiError::NotConfigured("AI is in offline mode — no network calls".into()));
        }
        self.resolved_key().ok_or_else(|| {
            AiError::NotConfigured(
                "no API key — set ANTHROPIC_API_KEY or `api_key` under [ai] in the config".into(),
            )
        })
    }
}

/// `$XDG_CONFIG_HOME/quill/config.toml`, else `$HOME/.config/quill/config.toml`.
fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("quill").join("config.toml"))
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
        Style::default().fg(Color::DarkGray),
    )));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut s = Settings { enabled: true, offline: false, model: default_model(), api_key: Some("k".into()) };
        s.check_ready().unwrap();
        s.offline = true;
        assert!(matches!(s.check_ready(), Err(AiError::NotConfigured(_))));
        s.offline = false;
        s.api_key = None;
        assert!(s.check_ready().is_err()); // no key
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
