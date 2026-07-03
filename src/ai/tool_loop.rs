//! The assistant's tool-use protocol (#306) — the building blocks the assistant
//! pane drives, shared across the Claude and local backends.
//!
//! - **Tools** are derived from the action registry ([`crate::ai::tools`]): one
//!   tool per runnable [`Action`](crate::mode::Action), with a typed input schema
//!   for the actions that take an argument (#390).
//! - **Each turn** sends the running message list plus the tools via
//!   [`build_body`]. The manuscript rides as the first user content block with
//!   `cache_control`, so the cached prefix is reused rather than re-billing it.
//! - **A model turn** is a [`Turn`]: tool calls to run, or a final text answer.
//!   A tool call is replayed as an `assistant` `tool_use` turn ([`tool_use_message`])
//!   and answered with `tool_result` blocks ([`tool_result_block`] /
//!   [`tool_result_message`]), matched by `tool_use_id`.
//! - **[`MAX_TURNS`]** is a hard cap so a confused model can't loop forever.
//! - **Transport**: [`Converse`] is the per-turn round-trip — the Claude backend
//!   implements it natively; the local backend ([`crate::ai::local`]) builds on the
//!   constrained-choice adapter below.
//!
//! The loop itself lives where the editor state does — [`crate::app::assistant`]
//! drives turns on the UI thread (network off-thread, action execution on it),
//! rather than a single off-thread function, so the orchestration isn't here.

#[cfg(any(feature = "local", test))]
use std::fmt::Write as _;

use serde_json::{json, Value};

use super::AiError;

/// The hard cap on tool-use turns — each is a billed request, so a model that
/// keeps calling tools without concluding is stopped here.
pub const MAX_TURNS: usize = 8;

/// One tool the model asked to run: the API's call id (echoed in the result), the
/// action name, and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// What one model turn produced: tool calls to run, or a final text answer.
pub enum Turn {
    ToolCalls(Vec<ToolCall>),
    Final(String),
}

/// One round-trip with the model: send a request body, get back a parsed turn.
/// The transport seam — the Claude and local backends implement it; tests mock it.
pub trait Converse {
    fn respond(&self, body: &Value) -> Result<Turn, AiError>;
}

/// The `assistant` turn replaying the model's tool calls (the API requires the
/// `tool_use` blocks be echoed back before their results). Shared by [`run`] and
/// the assistant pane (#307) so both speak the protocol the same way.
pub fn tool_use_message(calls: &[ToolCall]) -> Value {
    let blocks: Vec<Value> = calls
        .iter()
        .map(|c| json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": c.input }))
        .collect();
    json!({ "role": "assistant", "content": blocks })
}

/// One tool's result block, matched to its call by `tool_use_id`.
pub fn tool_result_block(id: &str, content: &str) -> Value {
    json!({ "type": "tool_result", "tool_use_id": id, "content": content })
}

/// The `user` turn carrying the tool results back to the model.
// takes ownership: `results` is moved into the constructed JSON message
#[allow(clippy::needless_pass_by_value)]
pub fn tool_result_message(results: Vec<Value>) -> Value {
    json!({ "role": "user", "content": results })
}

// --- Constrained-choice adapter (#310) -------------------------------------
//
// A backend without native tool-calling (the local Qwen) still drives the loop by
// making ONE *constrained* choice per turn — a JSON object `{action, message}`
// where `action` is a tool name or `"respond"`. Constrained decoding
// ([`crate::ai::constrain`]) guarantees a valid, on-list choice, so a small model
// can't emit an unparseable or off-allowlist call (tools take no arguments yet,
// #390, so a choice is just a name). The Claude backend keeps its native tool
// path; this is the shim the local [`Converse`] impl builds on. Compiled for the
// `local` backend (its only caller) and for tests (pure, so exercised in every
// `cargo test`) — `#[cfg(any(feature = "local", test))]` on each item.

/// The token a constrained choice uses to answer the author instead of calling a
/// tool.
#[cfg(any(feature = "local", test))]
pub const RESPOND: &str = "respond";

/// The tool names offered in a loop body.
#[cfg(any(feature = "local", test))]
pub fn tool_names(body: &Value) -> Vec<String> {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|ts| {
            ts.iter()
                .filter_map(|t| t["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The JSON schema for one constrained choice: an `action` (a tool name or
/// [`RESPOND`]), an `argument` (the text input for an action that takes one, #390),
/// and a `message` (the answer when responding).
#[cfg(any(feature = "local", test))]
pub fn choice_schema(tool_names: &[String]) -> Value {
    let mut actions: Vec<Value> = tool_names.iter().map(|n| json!(n)).collect();
    actions.push(json!(RESPOND));
    json!({
        "type": "object",
        "properties": {
            "action": { "enum": actions },
            "argument": { "type": "string" },
            "message": { "type": "string" },
        },
        "required": ["action", "message"],
    })
}

/// Flatten a loop body's `messages` into a plain transcript for a backend that
/// takes a single prompt (no role/block structure): the manuscript and requests,
/// the tools already run, and their results, in order.
#[cfg(any(feature = "local", test))]
pub fn flatten_conversation(messages: &[Value]) -> String {
    let mut out = String::new();
    for m in messages {
        let assistant = m["role"].as_str() == Some("assistant");
        match m["content"].as_array() {
            Some(blocks) => {
                for b in blocks {
                    match b["type"].as_str() {
                        Some("text") => {
                            if assistant {
                                out.push_str("Assistant: ");
                            }
                            out.push_str(b["text"].as_str().unwrap_or(""));
                            out.push('\n');
                        }
                        Some("tool_use") => {
                            writeln!(out, "(ran the tool: {})", b["name"].as_str().unwrap_or(""))
                                .unwrap();
                        }
                        Some("tool_result") => {
                            writeln!(
                                out,
                                "(tool result: {})",
                                b["content"].as_str().unwrap_or("")
                            )
                            .unwrap();
                        }
                        _ => {}
                    }
                }
            }
            None => {
                if let Some(s) = m["content"].as_str() {
                    out.push_str(s);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Parse a constrained `{action, argument, message}` choice into a [`Turn`]:
/// [`RESPOND`] is a final answer; any other action is the named tool call, carrying
/// `argument` as a generic `{"argument": …}` input when present (the executor maps
/// it to the action's named parameter, #390).
#[cfg(any(feature = "local", test))]
pub fn parse_choice_turn(text: &str) -> Result<Turn, AiError> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| AiError::ApiMalformed(format!("tool-use choice: {e}")))?;
    let message = v["message"].as_str().unwrap_or("").to_string();
    match v["action"].as_str() {
        Some(a) if a != RESPOND && !a.is_empty() => {
            let input = match v["argument"].as_str() {
                Some(arg) if !arg.is_empty() => json!({ "argument": arg }),
                _ => json!({}),
            };
            Ok(Turn::ToolCalls(vec![ToolCall {
                id: format!("local-{a}"),
                name: a.to_string(),
                input,
            }]))
        }
        _ => Ok(Turn::Final(message)),
    }
}

/// The Messages API body for one turn: model, the running messages, and the tools.
/// Used by the assistant pane's UI-thread turn machine (#307) and the local
/// backend's `respond` (#310), so both build the request the same way.
pub fn build_body(
    model: &str,
    system: Option<&str>,
    messages: &[Value],
    tools: &[Value],
    max_tokens: u32,
) -> Value {
    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": messages,
        "tools": tools,
    });
    if let Some(s) = system {
        body["system"] = json!(s);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Constrained-choice adapter (#310) ---------------------------------

    #[test]
    fn choice_schema_offers_every_tool_plus_respond() {
        let names = vec!["save".to_string(), "ai:continuity".to_string()];
        let schema = choice_schema(&names);
        let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
        let got: Vec<&str> = actions.iter().map(|a| a.as_str().unwrap()).collect();
        assert_eq!(got, vec!["save", "ai:continuity", RESPOND]);
        assert_eq!(schema["required"], json!(["action", "message"]));
        assert_eq!(
            schema["properties"]["argument"]["type"], "string",
            "an action's text input (#390)"
        );
    }

    #[test]
    fn a_tool_choice_carries_its_argument() {
        let turn = parse_choice_turn(r#"{"action":"ai:ask","argument":"Who dies?","message":""}"#)
            .unwrap();
        match turn {
            Turn::ToolCalls(calls) => {
                assert_eq!(calls[0].name, "ai:ask");
                assert_eq!(
                    calls[0].input,
                    json!({ "argument": "Who dies?" }),
                    "the argument rides as a generic key"
                );
            }
            Turn::Final(_) => panic!("a tool action should be a call"),
        }
    }

    #[test]
    fn tool_names_reads_the_body_catalog() {
        let body = json!({ "tools": [{ "name": "save" }, { "name": "report:progress" }] });
        assert_eq!(
            tool_names(&body),
            vec!["save".to_string(), "report:progress".to_string()]
        );
        assert!(tool_names(&json!({})).is_empty());
    }

    #[test]
    fn a_respond_choice_is_a_final_answer() {
        let turn = parse_choice_turn(r#"{"action":"respond","message":"All done."}"#).unwrap();
        match turn {
            Turn::Final(t) => assert_eq!(t, "All done."),
            Turn::ToolCalls(_) => panic!("respond should be a final answer"),
        }
    }

    #[test]
    fn a_tool_choice_is_a_named_call_with_no_args() {
        let turn = parse_choice_turn(r#"{"action":"save","message":"saving"}"#).unwrap();
        match turn {
            Turn::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "save");
                assert_eq!(calls[0].input, json!({}));
            }
            Turn::Final(_) => panic!("a tool action should be a call"),
        }
    }

    #[test]
    fn flatten_conversation_renders_the_turns_in_order() {
        let messages = vec![
            json!({ "role": "user", "content": [
                { "type": "text", "text": "MANUSCRIPT" },
                { "type": "text", "text": "save the chapter" },
            ] }),
            json!({ "role": "assistant", "content": [ { "type": "tool_use", "name": "save", "input": {} } ] }),
            json!({ "role": "user", "content": [ { "type": "tool_result", "content": "Saved." } ] }),
        ];
        let flat = flatten_conversation(&messages);
        assert!(flat.contains("MANUSCRIPT"));
        assert!(flat.contains("save the chapter"));
        assert!(flat.contains("ran the tool: save"));
        assert!(flat.contains("tool result: Saved."));
    }
}
