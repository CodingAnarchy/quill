//! The Claude HTTP backend: calls the Anthropic Messages API over HTTPS via
//! `ureq` (synchronous; runs on a worker thread). The manuscript is sent as a
//! cacheable content block so repeat questions over an unchanged draft are
//! cheap. See `docs/ai-features.md`.

use serde_json::{json, Value};

use super::{AiError, Backend, Request, Response, Usage};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

/// The Claude backend. Holds only the API key; the model travels on each
/// [`Request`].
pub struct ClaudeBackend {
    key: String,
}

impl ClaudeBackend {
    pub fn new(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }
}

impl Backend for ClaudeBackend {
    fn complete(&self, req: &Request) -> Result<Response, AiError> {
        let body = build_body(req);
        // Read the body on any status so we can surface the API's error message
        // (Anthropic returns a JSON `error.message` on 4xx/5xx).
        let mut res = ureq::post(API_URL)
            .config()
            .http_status_as_error(false)
            .build()
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .send_json(&body)
            .map_err(|e| AiError::Network(e.to_string()))?;

        let status = res.status();
        let value: Value = res
            .body_mut()
            .read_json()
            .map_err(|e| AiError::Api(e.to_string()))?;

        if !status.is_success() {
            let msg = value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(AiError::Api(format!("{}: {msg}", status.as_u16())));
        }
        parse_response(&value)
    }
}

/// The Messages API request body. The manuscript is the first user content block
/// and carries `cache_control`, so the cached prefix (system + manuscript) is
/// reused across questions; the question block follows it, uncached.
fn build_body(req: &Request) -> Value {
    let content = json!([
        {
            "type": "text",
            "text": req.cached_context,
            "cache_control": { "type": "ephemeral" }
        },
        { "type": "text", "text": req.user }
    ]);

    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": [ { "role": "user", "content": content } ],
    });
    if let Some(system) = &req.system {
        body["system"] = json!(system);
    }
    // Structured output: constrain the reply to the schema. The API then
    // guarantees the first text block is schema-valid JSON.
    if let Some(schema) = &req.json_schema {
        body["output_config"] = json!({
            "format": { "type": "json_schema", "schema": schema }
        });
    }
    body
}

/// Pull the answer text and usage out of a Messages API response.
fn parse_response(value: &Value) -> Result<Response, AiError> {
    let text = value
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err(AiError::Api("empty response".into()));
    }

    let usage = value
        .get("usage")
        .map(|u| Usage {
            input_tokens: u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,
            output_tokens: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,
            cache_read_tokens: u
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
        })
        .unwrap_or_default();

    Ok(Response { text, usage })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_body_caches_the_manuscript_block() {
        let req = Request {
            system: Some("sys".into()),
            cached_context: "the manuscript".into(),
            user: "the question".into(),
            model: "claude-opus-4-8".into(),
            max_tokens: 1024,
            json_schema: None,
        };
        let body = build_body(&req);
        assert_eq!(body["model"], "claude-opus-4-8");
        assert_eq!(body["system"], "sys");
        let content = &body["messages"][0]["content"];
        assert_eq!(content[0]["text"], "the manuscript");
        assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(content[1]["text"], "the question");
        // Only the manuscript block is cached, not the question.
        assert!(content[1].get("cache_control").is_none());
        // No schema → no output_config.
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn build_body_emits_output_config_for_a_schema() {
        let req = Request {
            system: None,
            cached_context: "m".into(),
            user: "u".into(),
            model: "claude-opus-4-8".into(),
            max_tokens: 1024,
            json_schema: Some(json!({ "type": "object" })),
        };
        let body = build_body(&req);
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(body["output_config"]["format"]["schema"]["type"], "object");
    }

    #[test]
    fn parse_response_extracts_text_and_usage() {
        let value = json!({
            "content": [ { "type": "text", "text": "The locket is in chapter one." } ],
            "usage": { "input_tokens": 1200, "output_tokens": 30, "cache_read_input_tokens": 1000 }
        });
        let resp = parse_response(&value).unwrap();
        assert_eq!(resp.text, "The locket is in chapter one.");
        assert_eq!(resp.usage.input_tokens, 1200);
        assert_eq!(resp.usage.cache_read_tokens, 1000);
    }

    #[test]
    fn parse_response_rejects_empty_content() {
        let value = json!({ "content": [], "usage": {} });
        assert!(matches!(parse_response(&value), Err(AiError::Api(_))));
    }
}
