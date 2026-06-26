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
                let tool_use: Vec<Value> = calls
                    .iter()
                    .map(|c| json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": c.input }))
                    .collect();
                messages.push(json!({ "role": "assistant", "content": tool_use }));

                let mut results = Vec::with_capacity(calls.len());
                for c in &calls {
                    let result = exec.run(&c.name, &c.input);
                    actions.push(c.name.clone());
                    results.push(json!({ "type": "tool_result", "tool_use_id": c.id, "content": result }));
                }
                messages.push(json!({ "role": "user", "content": results }));
            }
        }
    }

    // Hit the cap without a conclusion — stop, but report what ran.
    Ok(LoopOutcome { summary: format!("Stopped after {max_turns} tool-use turns."), actions })
}

/// The Messages API body for one turn: model, the running messages, and the tools.
fn build_body(model: &str, system: Option<&str>, messages: &[Value], tools: &[Value], max_tokens: u32) -> Value {
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
}
