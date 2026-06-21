//! The Claude HTTP backend: calls the Anthropic Messages API over HTTPS via
//! `ureq` (synchronous; runs on a worker thread). The manuscript is sent as a
//! cacheable content block so repeat questions over an unchanged draft are
//! cheap. See `docs/ai-features.md`.

use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use super::{AiError, Backend, Request, Response, Usage};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const BATCH_URL: &str = "https://api.anthropic.com/v1/messages/batches";
const API_VERSION: &str = "2023-06-01";

/// The `custom_id` of the single request in a one-shot rebuild batch (#176).
const BATCH_ID: &str = "rebuild";
/// Poll the batch's status this often, up to this many times (~10 min ceiling)
/// before giving up — a one-request batch usually ends in seconds to minutes.
const BATCH_POLL: Duration = Duration::from_secs(3);
const BATCH_MAX_POLLS: u32 = 200;

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
        let (status, value) = self.post(API_URL, &build_body(req))?;
        if !is_ok(status) {
            return Err(api_error(status, &value));
        }
        parse_response(&value)
    }

    /// Run the request through the Message Batches API (#176): submit a batch of
    /// one, poll until it ends, then fetch and parse the single result. Half the
    /// per-token price at the cost of latency — used for full-manuscript rebuilds.
    fn complete_batched(&self, req: &Request) -> Result<Response, AiError> {
        let (status, value) = self.post(BATCH_URL, &batch_submit_body(req))?;
        if !is_ok(status) {
            return Err(api_error(status, &value));
        }
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| AiError::Api("batch response had no id".into()))?;
        let status_url = format!("{BATCH_URL}/{id}");

        for _ in 0..BATCH_MAX_POLLS {
            let (code, batch) = self.get_json(&status_url)?;
            if !is_ok(code) {
                return Err(api_error(code, &batch));
            }
            if batch.get("processing_status").and_then(Value::as_str) == Some("ended") {
                let results_url = batch
                    .get("results_url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AiError::Api("batch ended without a results url".into()))?;
                return parse_batch_result(&self.get_text(results_url)?, BATCH_ID);
            }
            thread::sleep(BATCH_POLL);
        }
        Err(AiError::Api("batch still processing after the time limit — try again".into()))
    }
}

impl ClaudeBackend {
    /// POST `body` to `url` with the auth headers; returns the HTTP status and the
    /// parsed JSON body (the API returns JSON on errors too, so we read it on any
    /// status to surface its `error.message`).
    fn post(&self, url: &str, body: &Value) -> Result<(u16, Value), AiError> {
        let mut res = ureq::post(url)
            .config()
            .http_status_as_error(false)
            .build()
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .send_json(body)
            .map_err(|e| AiError::Network(e.to_string()))?;
        let status = res.status().as_u16();
        let value = res.body_mut().read_json().map_err(|e| AiError::Api(e.to_string()))?;
        Ok((status, value))
    }

    /// GET `url` as JSON with the auth headers.
    fn get_json(&self, url: &str) -> Result<(u16, Value), AiError> {
        let mut res = ureq::get(url)
            .config()
            .http_status_as_error(false)
            .build()
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .call()
            .map_err(|e| AiError::Network(e.to_string()))?;
        let status = res.status().as_u16();
        let value = res.body_mut().read_json().map_err(|e| AiError::Api(e.to_string()))?;
        Ok((status, value))
    }

    /// GET `url` as raw text (the batch results are JSONL, not a JSON document).
    fn get_text(&self, url: &str) -> Result<String, AiError> {
        ureq::get(url)
            .config()
            .http_status_as_error(false)
            .build()
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .call()
            .map_err(|e| AiError::Network(e.to_string()))?
            .body_mut()
            .read_to_string()
            .map_err(|e| AiError::Api(e.to_string()))
    }
}

/// Whether an HTTP status is 2xx.
fn is_ok(status: u16) -> bool {
    (200..300).contains(&status)
}

/// An `AiError::Api` from an error response body (Anthropic returns a JSON
/// `error.message`).
fn api_error(status: u16, value: &Value) -> AiError {
    let msg = value.pointer("/error/message").and_then(Value::as_str).unwrap_or("unknown error");
    AiError::Api(format!("{status}: {msg}"))
}

/// The Message Batches submit body: a batch of one request, the standard message
/// body as its `params`.
fn batch_submit_body(req: &Request) -> Value {
    json!({ "requests": [ { "custom_id": BATCH_ID, "params": build_body(req) } ] })
}

/// Parse the batch results JSONL: find the line for `custom_id`, require it
/// succeeded, and pull the answer out of its message.
fn parse_batch_result(jsonl: &str, custom_id: &str) -> Result<Response, AiError> {
    for line in jsonl.lines().filter(|l| !l.trim().is_empty()) {
        let value: Value = serde_json::from_str(line).map_err(|e| AiError::Api(format!("bad batch result: {e}")))?;
        if value.get("custom_id").and_then(Value::as_str) != Some(custom_id) {
            continue;
        }
        let result = value.get("result");
        return match result.and_then(|r| r.get("type")).and_then(Value::as_str) {
            Some("succeeded") => {
                let message = result.and_then(|r| r.get("message")).ok_or_else(|| {
                    AiError::Api("batch result missing its message".into())
                })?;
                parse_response(message)
            }
            Some(other) => Err(AiError::Api(format!("batch request {other}"))),
            None => Err(AiError::Api("batch result had no outcome".into())),
        };
    }
    Err(AiError::Api(format!("no batch result for '{custom_id}'")))
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

    fn req() -> Request {
        Request {
            system: None,
            cached_context: "m".into(),
            user: "u".into(),
            model: "claude-opus-4-8".into(),
            max_tokens: 1024,
            json_schema: None,
        }
    }

    #[test]
    fn batch_submit_wraps_the_message_body() {
        let body = batch_submit_body(&req());
        assert_eq!(body["requests"][0]["custom_id"], BATCH_ID);
        // The standard message body rides along as the request's params.
        assert_eq!(body["requests"][0]["params"]["model"], "claude-opus-4-8");
        assert_eq!(body["requests"][0]["params"]["messages"][0]["content"][0]["text"], "m");
    }

    #[test]
    fn parse_batch_result_extracts_a_succeeded_message() {
        let jsonl = r#"{"custom_id":"rebuild","result":{"type":"succeeded","message":{"content":[{"type":"text","text":"ok"}],"usage":{"output_tokens":2}}}}"#;
        let resp = parse_batch_result(jsonl, "rebuild").unwrap();
        assert_eq!(resp.text, "ok");
        assert_eq!(resp.usage.output_tokens, 2);
    }

    #[test]
    fn parse_batch_result_reports_a_failed_request() {
        let jsonl = r#"{"custom_id":"rebuild","result":{"type":"errored","error":{}}}"#;
        assert!(matches!(parse_batch_result(jsonl, "rebuild"), Err(AiError::Api(_))));
    }

    #[test]
    fn parse_batch_result_errors_when_its_id_is_absent() {
        let jsonl = r#"{"custom_id":"other","result":{"type":"succeeded"}}"#;
        assert!(matches!(parse_batch_result(jsonl, "rebuild"), Err(AiError::Api(_))));
        assert!(matches!(parse_batch_result("", "rebuild"), Err(AiError::Api(_))));
    }
}
