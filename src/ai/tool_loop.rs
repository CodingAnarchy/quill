//! The assistant's tool-use loop (#306) — the settled contract every other child
//! of the assistant epic (#305) builds on.
//!
//! # The decision
//!
//! - **Tools** are derived from the action registry ([`crate::ai::tools`]): one
//!   tool per [`Action`](crate::mode::Action), `name` = the action's stable token,
//!   `description` = its label, input schema = the empty object (no free-text args
//!   yet — that's #390). New actions appear automatically.
//! - **Each turn** sends the running message list plus the tools. The manuscript
//!   rides as the first user content block with `cache_control`, so the cached
//!   prefix is reused every turn rather than re-billing the novel.
//! - **A tool call** ([`Turn::ToolCalls`]) is replayed back as an `assistant`
//!   `tool_use` turn, then each call is run via [`ToolExec`] and its outcome
//!   returned as a `tool_result` block in the following `user` turn — matched by
//!   `tool_use_id`. The loop then asks the model to continue.
//! - **Stop condition**: the model replies with text and no tool call
//!   ([`Turn::Final`]) — that text is the answer. Or the turn cap is hit.
//! - **Max turns**: [`MAX_TURNS`], a hard bound so a confused model can't loop
//!   forever (each turn is a billed request).
//! - **Threading**: [`run`] is synchronous and pure of editor state — it talks to
//!   a [`Converse`] (the transport) and a [`ToolExec`] (the action runner), so it
//!   runs on a worker thread via [`crate::app::task`] and is cancelable by
//!   dropping the task. Executing actions that *mutate the editor* must hand back
//!   to the UI thread; that routing (and the assistant pane) is the next child —
//!   this spike settles the protocol, not the UI.
//!
//! Deferred (follow-ups): the local backend's tool-use (#389), per-action tool
//! inputs (#390), and UI-thread action execution + the pane (#305's next child).
//!
//! Backend-first foundation: the contract and the Claude transport land here,
//! tested with a mock backend and off-thread via the task runner; the user-facing
//! command and real action execution are the next child's wiring, so these items
//! have no non-test caller yet.
#![allow(dead_code)]

use serde_json::{json, Value};

use super::AiError;

/// The hard cap on tool-use turns — each is a billed request, so a model that
/// keeps calling tools without concluding is stopped here.
pub const MAX_TURNS: usize = 8;

/// One tool the model asked to run: the API's call id (echoed in the result), the
/// action name, and its arguments (the empty object today).
#[derive(Debug, Clone, PartialEq)]
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
/// The transport seam — the Claude backend implements it; tests mock it.
pub trait Converse {
    fn respond(&self, body: &Value) -> Result<Turn, AiError>;
}

/// Runs a named action and returns the result text fed back to the model. The
/// execution seam — kept off the loop so the loop stays transport-only and the
/// runner (a UI-thread router, a snapshot reader, or a test recorder) is the
/// caller's choice.
pub trait ToolExec {
    fn run(&mut self, name: &str, input: &Value) -> String;
}

/// The result of a loop: the model's final summary and the actions it ran, in
/// order — so the caller can report "what it ran" even if the model's prose is thin.
pub struct LoopOutcome {
    pub summary: String,
    pub actions: Vec<String>,
}

/// Drive the tool-use loop to a final answer (or the turn cap). Synchronous; run
/// it on a worker thread. `manuscript` is the cached prefix, `request` the
/// author's ask, `tools` the registry-derived catalog.
#[allow(clippy::too_many_arguments)]
pub fn run(
    conv: &dyn Converse,
    exec: &mut dyn ToolExec,
    model: &str,
    system: Option<&str>,
    manuscript: &str,
    request: &str,
    tools: &[Value],
    max_turns: usize,
    max_tokens: u32,
) -> Result<LoopOutcome, AiError> {
    let mut messages = vec![json!({
        "role": "user",
        "content": [
            { "type": "text", "text": manuscript, "cache_control": { "type": "ephemeral" } },
            { "type": "text", "text": request }
        ]
    })];
    let mut actions = Vec::new();

    for _ in 0..max_turns {
        let body = build_body(model, system, &messages, tools, max_tokens);
        match conv.respond(&body)? {
            Turn::Final(text) => return Ok(LoopOutcome { summary: text, actions }),
            Turn::ToolCalls(calls) => {
                // Replay the model's tool_use turn, then answer with tool_results.
                messages.push(tool_use_message(&calls));
                let mut results = Vec::with_capacity(calls.len());
                for c in &calls {
                    let result = exec.run(&c.name, &c.input);
                    actions.push(c.name.clone());
                    results.push(tool_result_block(&c.id, &result));
                }
                messages.push(tool_result_message(results));
            }
        }
    }

    // Hit the cap without a conclusion — stop, but report what ran.
    Ok(LoopOutcome { summary: format!("Stopped after {max_turns} tool-use turns."), actions })
}

/// The `assistant` turn replaying the model's tool calls (the API requires the
/// `tool_use` blocks be echoed back before their results). Shared by [`run`] and
/// the assistant pane (#307) so both speak the protocol the same way.
pub(crate) fn tool_use_message(calls: &[ToolCall]) -> Value {
    let blocks: Vec<Value> = calls
        .iter()
        .map(|c| json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": c.input }))
        .collect();
    json!({ "role": "assistant", "content": blocks })
}

/// One tool's result block, matched to its call by `tool_use_id`.
pub(crate) fn tool_result_block(id: &str, content: &str) -> Value {
    json!({ "type": "tool_result", "tool_use_id": id, "content": content })
}

/// The `user` turn carrying the tool results back to the model.
pub(crate) fn tool_result_message(results: Vec<Value>) -> Value {
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
// path; this is the shim the local [`Converse`] impl builds on. The pure pieces
// live here (not in the `local`-gated module) so they're tested in every build.

/// The token a constrained choice uses to answer the author instead of calling a
/// tool.
pub(crate) const RESPOND: &str = "respond";

/// The tool names offered in a loop body.
pub(crate) fn tool_names(body: &Value) -> Vec<String> {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|ts| ts.iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// The JSON schema for one constrained choice: an `action` (a tool name or
/// [`RESPOND`]), an `argument` (the text input for an action that takes one, #390),
/// and a `message` (the answer when responding).
pub(crate) fn choice_schema(tool_names: &[String]) -> Value {
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
pub(crate) fn flatten_conversation(messages: &[Value]) -> String {
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
                            out.push_str(&format!("(ran the tool: {})\n", b["name"].as_str().unwrap_or("")));
                        }
                        Some("tool_result") => {
                            out.push_str(&format!("(tool result: {})\n", b["content"].as_str().unwrap_or("")));
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
pub(crate) fn parse_choice_turn(text: &str) -> Result<Turn, AiError> {
    let v: Value =
        serde_json::from_str(text).map_err(|e| AiError::ApiMalformed(format!("tool-use choice: {e}")))?;
    let message = v["message"].as_str().unwrap_or("").to_string();
    match v["action"].as_str() {
        Some(a) if a != RESPOND && !a.is_empty() => {
            let input = match v["argument"].as_str() {
                Some(arg) if !arg.is_empty() => json!({ "argument": arg }),
                _ => json!({}),
            };
            Ok(Turn::ToolCalls(vec![ToolCall { id: format!("local-{a}"), name: a.to_string(), input }]))
        }
        _ => Ok(Turn::Final(message)),
    }
}

/// The Messages API body for one turn: model, the running messages, and the tools.
/// Shared by [`run`] and the assistant pane's UI-thread turn machine (#307), so
/// both build the request the same way.
pub(crate) fn build_body(model: &str, system: Option<&str>, messages: &[Value], tools: &[Value], max_tokens: u32) -> Value {
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
    use std::sync::Mutex;

    /// A scripted backend: hands back the next queued turn per call.
    struct ScriptedConverse {
        turns: Mutex<std::collections::VecDeque<Turn>>,
        bodies: Mutex<Vec<Value>>,
    }
    impl ScriptedConverse {
        fn new(turns: Vec<Turn>) -> Self {
            Self { turns: Mutex::new(turns.into()), bodies: Mutex::new(Vec::new()) }
        }
    }
    impl Converse for ScriptedConverse {
        fn respond(&self, body: &Value) -> Result<Turn, AiError> {
            self.bodies.lock().unwrap().push(body.clone());
            Ok(self.turns.lock().unwrap().pop_front().expect("a scripted turn"))
        }
    }

    /// Records the tools it was asked to run and echoes a stub result.
    #[derive(Default)]
    struct Recorder {
        ran: Vec<String>,
    }
    impl ToolExec for Recorder {
        fn run(&mut self, name: &str, _input: &Value) -> String {
            self.ran.push(name.to_string());
            format!("ran {name}")
        }
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall { id: id.into(), name: name.into(), input: json!({}) }
    }

    #[test]
    fn invokes_tools_in_order_then_returns_the_final_answer() {
        let conv = ScriptedConverse::new(vec![
            Turn::ToolCalls(vec![call("a", "save")]),
            Turn::ToolCalls(vec![call("b", "report:progress")]),
            Turn::Final("Saved and opened the progress report.".into()),
        ]);
        let mut exec = Recorder::default();
        let out = run(&conv, &mut exec, "m", None, "MS", "do it", &[], MAX_TURNS, 1024).unwrap();

        assert_eq!(exec.ran, vec!["save", "report:progress"]);
        assert_eq!(out.actions, vec!["save", "report:progress"]);
        assert_eq!(out.summary, "Saved and opened the progress report.");

        // The manuscript is the cached prefix every turn, and tool_results feed back.
        let bodies = conv.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 3);
        assert_eq!(bodies[0]["messages"][0]["content"][0]["cache_control"]["type"], "ephemeral");
        // By the third turn the conversation carries the tool_use + tool_result turns.
        assert_eq!(bodies[2]["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(bodies[2]["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(bodies[2]["messages"][2]["content"][0]["tool_use_id"], "a");
    }

    #[test]
    fn several_tool_calls_in_one_turn_all_run() {
        let conv = ScriptedConverse::new(vec![
            Turn::ToolCalls(vec![call("a", "save"), call("b", "export:markdown")]),
            Turn::Final("done".into()),
        ]);
        let mut exec = Recorder::default();
        let out = run(&conv, &mut exec, "m", None, "", "", &[], MAX_TURNS, 64).unwrap();
        assert_eq!(out.actions, vec!["save", "export:markdown"]);
    }

    #[test]
    fn stops_at_the_turn_cap_reporting_what_ran() {
        // Always asks for another tool — never concludes.
        let conv = ScriptedConverse::new(vec![
            Turn::ToolCalls(vec![call("a", "save")]),
            Turn::ToolCalls(vec![call("b", "save")]),
            Turn::ToolCalls(vec![call("c", "save")]),
        ]);
        let mut exec = Recorder::default();
        let out = run(&conv, &mut exec, "m", None, "", "", &[], 2, 64).unwrap();
        assert!(out.summary.contains("Stopped after 2"), "{}", out.summary);
        assert_eq!(out.actions.len(), 2, "ran exactly the capped number of turns");
    }

    #[test]
    fn runs_off_thread_via_the_task_runner() {
        use crate::app::task::{Poll, Task};
        let mut task: Task<LoopOutcome> = Task::idle();
        task.spawn(|| {
            let conv = ScriptedConverse::new(vec![
                Turn::ToolCalls(vec![call("a", "save")]),
                Turn::Final("done".into()),
            ]);
            let mut exec = Recorder::default();
            run(&conv, &mut exec, "m", None, "", "", &[], MAX_TURNS, 64).unwrap()
        });
        // Spin briefly for the worker (the runner is non-blocking).
        let mut out = None;
        for _ in 0..1000 {
            if let Poll::Ready(o) = task.poll() {
                out = Some(o);
                break;
            }
            std::thread::yield_now();
        }
        let out = out.expect("the off-thread loop produced a result");
        assert_eq!(out.actions, vec!["save"]);
        assert_eq!(out.summary, "done");
    }

    // --- Constrained-choice adapter (#310) ---------------------------------

    #[test]
    fn choice_schema_offers_every_tool_plus_respond() {
        let names = vec!["save".to_string(), "ai:continuity".to_string()];
        let schema = choice_schema(&names);
        let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
        let got: Vec<&str> = actions.iter().map(|a| a.as_str().unwrap()).collect();
        assert_eq!(got, vec!["save", "ai:continuity", RESPOND]);
        assert_eq!(schema["required"], json!(["action", "message"]));
        assert_eq!(schema["properties"]["argument"]["type"], "string", "an action's text input (#390)");
    }

    #[test]
    fn a_tool_choice_carries_its_argument() {
        let turn = parse_choice_turn(r#"{"action":"ai:ask","argument":"Who dies?","message":""}"#).unwrap();
        match turn {
            Turn::ToolCalls(calls) => {
                assert_eq!(calls[0].name, "ai:ask");
                assert_eq!(calls[0].input, json!({ "argument": "Who dies?" }), "the argument rides as a generic key");
            }
            Turn::Final(_) => panic!("a tool action should be a call"),
        }
    }

    #[test]
    fn tool_names_reads_the_body_catalog() {
        let body = json!({ "tools": [{ "name": "save" }, { "name": "report:progress" }] });
        assert_eq!(tool_names(&body), vec!["save".to_string(), "report:progress".to_string()]);
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
