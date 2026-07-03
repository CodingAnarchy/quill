//! The Claude HTTP backend: calls the Anthropic Messages API over HTTPS via
//! `ureq` (synchronous; runs on a worker thread). The manuscript is sent as a
//! cacheable content block so repeat questions over an unchanged draft are
//! cheap. See `docs/ai-features.md`.

use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use super::http::{self, Header};
use super::tool_loop::{Converse, ToolCall, Turn};
use super::{is_ok, AiError, Backend, Request, Response, Usage};

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
        let body = build_body(req);
        // Single idempotent request: ride out a transient blip (429/529, dropped
        // connection) with bounded backoff rather than forfeiting a whole,
        // expensive pass (#697). The batch path below is deliberately not retried.
        let (status, value) = http::retry(http::transient_status, || self.post(API_URL, &body))?;
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
            .ok_or_else(|| AiError::ApiMalformed("batch response had no id".into()))?;
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
                    .ok_or_else(|| {
                        AiError::ApiMalformed("batch ended without a results url".into())
                    })?;
                return parse_batch_result(&self.get_text(results_url)?, BATCH_ID);
            }
            thread::sleep(BATCH_POLL);
        }
        // Don't invite a blind retry: the submitted batch keeps running (and
        // billing) server-side, so re-running would pay for a second one.
        Err(AiError::Network {
            status: None,
            message: "batch still processing after the time limit — it should finish on its own shortly (re-running starts a new, separately-billed batch)".into(),
        })
    }

    fn as_converse(&self) -> Option<&dyn Converse> {
        Some(self)
    }
}

impl Converse for ClaudeBackend {
    /// One turn of the assistant tool-use loop (#306): POST the loop-built body
    /// (messages + tools + the cached manuscript prefix) and parse the reply into
    /// tool calls or a final answer.
    fn respond(&self, body: &Value) -> Result<Turn, AiError> {
        // One idempotent turn of the tool-use loop: same bounded-retry policy as
        // the single-shot completion (#697).
        let (status, value) = http::retry(http::transient_status, || self.post(API_URL, body))?;
        if !is_ok(status) {
            return Err(api_error(status, &value));
        }
        parse_turn(&value)
    }
}

/// Parse a Messages API reply into a [`Turn`] (#306): any `tool_use` blocks become
/// tool calls; otherwise the joined text is the final answer.
fn parse_turn(value: &Value) -> Result<Turn, AiError> {
    let blocks = value
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| AiError::ApiMalformed("response had no content".into()))?;

    let calls: Vec<ToolCall> = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
        .map(|b| ToolCall {
            id: b
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: b
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input: b.get("input").cloned().unwrap_or_else(|| json!({})),
        })
        .collect();
    if !calls.is_empty() {
        return Ok(Turn::ToolCalls(calls));
    }

    let text = blocks
        .iter()
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    Ok(Turn::Final(text))
}

impl ClaudeBackend {
    /// The auth headers every Claude request carries.
    const fn auth(&self) -> [Header<'_>; 2] {
        [
            ("x-api-key", self.key.as_str()),
            ("anthropic-version", API_VERSION),
        ]
    }

    /// POST `body` to `url` with the auth headers; returns the HTTP status and the
    /// parsed JSON body (the API returns JSON on errors too, so we read it on any
    /// status to surface its `error.message`).
    fn post(&self, url: &str, body: &Value) -> Result<(u16, Value), AiError> {
        let [key, version] = self.auth();
        http::post_json(
            url,
            &[key, version, ("content-type", "application/json")],
            body,
        )
    }

    /// GET `url` as JSON with the auth headers.
    fn get_json(&self, url: &str) -> Result<(u16, Value), AiError> {
        http::get_json(url, &self.auth())
    }

    /// GET `url` as raw text (the batch results are JSONL, not a JSON document).
    fn get_text(&self, url: &str) -> Result<String, AiError> {
        http::get_text(url, &self.auth())
    }
}

/// The `Network` error for an error response body (Anthropic returns a JSON
/// `error.message`); `Display` prepends the status.
fn api_error(status: u16, value: &Value) -> AiError {
    let msg = value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    AiError::Network {
        status: Some(status),
        message: msg.to_string(),
    }
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
        let value: Value = serde_json::from_str(line)
            .map_err(|e| AiError::ApiMalformed(format!("bad batch result: {e}")))?;
        if value.get("custom_id").and_then(Value::as_str) != Some(custom_id) {
            continue;
        }
        let result = value.get("result");
        return match result.and_then(|r| r.get("type")).and_then(Value::as_str) {
            Some("succeeded") => {
                let message = result.and_then(|r| r.get("message")).ok_or_else(|| {
                    AiError::ApiMalformed("batch result missing its message".into())
                })?;
                parse_response(message)
            }
            Some(other) => Err(AiError::Network {
                status: None,
                message: format!("batch request {other}"),
            }),
            None => Err(AiError::ApiMalformed("batch result had no outcome".into())),
        };
    }
    Err(AiError::ApiMalformed(format!(
        "no batch result for '{custom_id}'"
    )))
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
        return Err(AiError::ApiMalformed("empty response".into()));
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
        assert!(matches!(
            parse_response(&value),
            Err(AiError::ApiMalformed(_))
        ));
    }

    #[test]
    fn parse_turn_reads_tool_use_blocks() {
        let value = json!({
            "content": [
                { "type": "tool_use", "id": "t1", "name": "report:progress", "input": {} },
                { "type": "tool_use", "id": "t2", "name": "save", "input": {} }
            ]
        });
        match parse_turn(&value).unwrap() {
            Turn::ToolCalls(calls) => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].name, "report:progress");
                assert_eq!(calls[0].id, "t1");
                assert_eq!(calls[1].name, "save");
            }
            Turn::Final(_) => panic!("expected tool calls"),
        }
    }

    #[test]
    fn a_surprising_response_shape_errors_rather_than_panics() {
        // Shapes the API shouldn't send, but might if it changes (#407): each must
        // resolve to a Result we can inspect — never an indexing/unwrap panic.
        for v in [
            json!({}),
            json!({ "content": "not an array" }),
            json!({ "content": [ { "type": "image" } ] }),
            json!("a bare string"),
            json!(42),
            json!({ "content": [ { "no": "type" } ], "usage": "wrong" }),
        ] {
            let _ = parse_response(&v); // must not panic
            let _ = parse_turn(&v); // must not panic
        }
        // The empty / wrong-typed content cases are specifically surfaced errors.
        assert!(matches!(
            parse_response(&json!({})),
            Err(AiError::ApiMalformed(_))
        ));
        assert!(matches!(
            parse_response(&json!({ "content": "x" })),
            Err(AiError::ApiMalformed(_))
        ));
        assert!(matches!(
            parse_turn(&json!({})),
            Err(AiError::ApiMalformed(_))
        ));
    }

    #[test]
    fn parse_turn_reads_a_final_text_answer() {
        let value =
            json!({ "content": [ { "type": "text", "text": "Done — saved the chapter." } ] });
        match parse_turn(&value).unwrap() {
            Turn::Final(text) => assert_eq!(text, "Done — saved the chapter."),
            Turn::ToolCalls(_) => panic!("expected a final answer"),
        }
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
        assert_eq!(
            body["requests"][0]["params"]["messages"][0]["content"][0]["text"],
            "m"
        );
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
        assert!(matches!(
            parse_batch_result(jsonl, "rebuild"),
            Err(AiError::Network { .. })
        ));
    }

    #[test]
    fn parse_batch_result_errors_when_its_id_is_absent() {
        let jsonl = r#"{"custom_id":"other","result":{"type":"succeeded"}}"#;
        assert!(matches!(
            parse_batch_result(jsonl, "rebuild"),
            Err(AiError::ApiMalformed(_))
        ));
        assert!(matches!(
            parse_batch_result("", "rebuild"),
            Err(AiError::ApiMalformed(_))
        ));
    }
}
