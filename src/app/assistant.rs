//! The authorial assistant pane (#307): a docked conversation with the tool-use
//! assistant (#306). The author types a request in plain language; the model
//! answers and runs editor actions on their behalf, with the conversation kept
//! across turns while the pane is open.
//!
//! # Threading — a UI-thread turn machine
//!
//! The tool-use loop has two seams that pull in opposite directions: the network
//! turn must run *off* the UI thread (so typing never freezes), but executing an
//! action mutates the editor and must run *on* it. Rather than run the all-in-one
//! [`tool_loop::run`] off-thread and ferry every action back over a channel, the
//! pane drives the loop one turn at a time from the UI thread:
//!
//! 1. A request spawns a single network turn on a worker via [`Task`].
//! 2. [`App::poll_assistant`] (each event-loop tick) collects the result. A final
//!    answer ends the request; tool calls are run right here on the UI thread via
//!    [`App::dispatch_action`], their results fed back, and the next turn spawned.
//!
//! So the building blocks of #306 are reused — the registry-derived tools, the
//! cached-manuscript body ([`tool_loop::build_body`]), the [`Converse`] transport
//! — but the orchestration lives where the editor state does. Esc cancels by
//! dropping the in-flight task; the conversation persists for the next request.
//!
//! Claude-only for now (local tool-use is #389), and the model is offered only
//! the actions it can run to completion ([`tools::assistant_tools`]).

use serde_json::{json, Value};

use crate::ai::tool_loop::{self, Converse, Turn};
use crate::ai::{tools, AiError, BackendKind, ClaudeBackend, MAX_OUTPUT_TOKENS};
use crate::mode::Action;

use super::task::{Poll, Task};
use super::{App, OtherPane, Split};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The system prompt: the assistant's brief, including the manuscript-prose
/// boundary (CLAUDE.md) so the model never drifts into co-writing.
const SYSTEM: &str = "You are the authorial assistant inside Quill, a terminal novel editor. \
Help the author by running editor actions on their behalf and answering questions about their \
manuscript, which is provided as context. Use the available tools (saving, exporting, opening \
reports, navigating spelling flags, toggling view modes) to carry out a request, then briefly \
say what you did. You are an assistant, never a co-writer: never draft, rewrite, expand, tighten, \
or suggest manuscript prose, and never propose edits to the author's text — if a request would \
require writing prose for them, decline and say why. Keep replies short and plain.";

/// One off-thread network turn's result.
type TurnResult = Result<Turn, AiError>;

/// Who spoke a transcript line, so the renderer can style it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Speaker {
    /// The author's request.
    You,
    /// The model's prose reply.
    Assistant,
    /// An action the assistant ran, and its result.
    Tool,
    /// A status note from Quill (ready, canceled, errors).
    System,
}

/// One rendered line of the conversation transcript.
pub(crate) struct Entry {
    pub(crate) who: Speaker,
    pub(crate) text: String,
}

impl Entry {
    fn new(who: Speaker, text: impl Into<String>) -> Self {
        Self { who, text: text.into() }
    }
    fn you(text: &str) -> Self {
        Self::new(Speaker::You, text)
    }
    fn assistant(text: &str) -> Self {
        Self::new(Speaker::Assistant, text)
    }
    fn tool(text: &str) -> Self {
        Self::new(Speaker::Tool, text)
    }
    fn sys(text: &str) -> Self {
        Self::new(Speaker::System, text)
    }
}

/// The assistant pane's conversation state (#307), scoped to the pane's lifetime.
pub(crate) struct Assistant {
    /// The Messages API conversation: every turn appended and re-sent so context
    /// persists across requests. The manuscript rides the first user block with
    /// `cache_control`, so the cached prefix is reused rather than re-billed.
    messages: Vec<Value>,
    /// The transcript shown in the pane.
    entries: Vec<Entry>,
    /// The request line being typed.
    input: String,
    /// How many rows the transcript is scrolled up from the tail (0 = follow it).
    scroll_up: u16,
    /// The in-flight network turn (off-thread); idle when not awaiting the model.
    task: Task<TurnResult>,
    /// Tool calls run during the current request, for the "ran …" record.
    ran: Vec<String>,
    /// Tool-use turns spent on the current request, capped at [`tool_loop::MAX_TURNS`].
    turns: usize,
}

impl Assistant {
    /// A fresh conversation with an opening hint.
    pub(super) fn new() -> Self {
        let mut a = Self {
            messages: Vec::new(),
            entries: Vec::new(),
            input: String::new(),
            scroll_up: 0,
            task: Task::idle(),
            ran: Vec::new(),
            turns: 0,
        };
        a.entries.push(Entry::sys(
            "Type a request and press Enter. Esc cancels a run; Ctrl-w c closes the pane.",
        ));
        a
    }

    /// Whether a network turn is in flight (the model is "thinking").
    pub(crate) fn is_running(&self) -> bool {
        self.task.is_busy()
    }

    /// The transcript lines, for the renderer.
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The request line being typed, for the renderer.
    pub(crate) fn input(&self) -> &str {
        &self.input
    }

    /// How far the transcript is scrolled up from the tail, for the renderer.
    pub(crate) fn scroll_up(&self) -> u16 {
        self.scroll_up
    }

    /// Push a Quill status note into the transcript (used when opening the pane).
    pub(super) fn note(&mut self, text: &str) {
        self.entries.push(Entry::sys(text));
        self.follow();
    }

    /// Jump the transcript back to the tail (after new content).
    fn follow(&mut self) {
        self.scroll_up = 0;
    }
}

impl App {
    /// The docked assistant pane's conversation, if one is open.
    fn assistant(&self) -> Option<&Assistant> {
        match &self.split.as_ref()?.other {
            OtherPane::Assistant { conv, .. } => Some(conv),
            _ => None,
        }
    }

    /// The docked assistant pane's conversation, mutably, if one is open.
    fn assistant_mut(&mut self) -> Option<&mut Assistant> {
        match &mut self.split.as_mut()?.other {
            OtherPane::Assistant { conv, .. } => Some(conv),
            _ => None,
        }
    }

    /// Handle a key while the assistant pane has focus (#307): edit the request
    /// line, submit it, scroll the transcript, or cancel / unfocus on Esc.
    pub(crate) fn on_key_assistant(&mut self, key: KeyEvent) {
        let running = self.assistant().is_some_and(Assistant::is_running);
        let max = self.assistant_scroll_max.get();
        match key.code {
            // Enter submits; both touch `&mut self`, so they run outside the borrow.
            KeyCode::Enter if !running => self.submit_assistant(),
            // Cancel the in-flight turn by dropping the task; the conversation stays
            // so the next request still has context.
            KeyCode::Esc if running => {
                if let Some(a) = self.assistant_mut() {
                    a.task = Task::idle();
                    a.ran.clear();
                    a.turns = 0;
                    a.entries.push(Entry::sys("Canceled."));
                    a.follow();
                }
            }
            // Esc (idle) hands keys back to the editor; the pane stays docked.
            KeyCode::Esc => {
                if let Some(Split { other: OtherPane::Assistant { focused, .. }, .. }) = self.split.as_mut() {
                    *focused = false;
                }
            }
            _ => {
                let Some(a) = self.assistant_mut() else { return };
                match key.code {
                    KeyCode::Backspace if !running => {
                        a.input.pop();
                    }
                    KeyCode::Char(c) if !running && !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        a.input.push(c);
                    }
                    KeyCode::Up => a.scroll_up = (a.scroll_up + 1).min(max),
                    KeyCode::Down => a.scroll_up = a.scroll_up.saturating_sub(1),
                    KeyCode::PageUp => a.scroll_up = (a.scroll_up + 5).min(max),
                    KeyCode::PageDown => a.scroll_up = a.scroll_up.saturating_sub(5),
                    _ => {}
                }
            }
        }
    }

    /// Submit the typed request: append it to the conversation and spawn the first
    /// network turn. A no-op while a run is in progress or the line is empty.
    fn submit_assistant(&mut self) {
        let (request, first) = {
            let Some(a) = self.assistant_mut() else { return };
            if a.is_running() {
                return;
            }
            let request = a.input.trim().to_string();
            if request.is_empty() {
                return;
            }
            a.input.clear();
            a.ran.clear();
            a.turns = 0;
            a.entries.push(Entry::you(&request));
            a.follow();
            (request, a.messages.is_empty())
        };

        // AI off / not configured: explain in the pane rather than failing obscurely.
        if let Err(e) = self.ai.ready_for(BackendKind::Claude) {
            if let Some(a) = self.assistant_mut() {
                a.note(&format!("The assistant is unavailable: {e}"));
            }
            return;
        }

        // The manuscript is the cached prefix on the first message; later requests
        // are just the new text, so context persists without re-billing the novel.
        let content = if first {
            let manuscript = self.manuscript_text();
            vec![
                json!({ "type": "text", "text": manuscript, "cache_control": { "type": "ephemeral" } }),
                json!({ "type": "text", "text": request }),
            ]
        } else {
            vec![json!({ "type": "text", "text": request })]
        };
        if let Some(a) = self.assistant_mut() {
            a.messages.push(json!({ "role": "user", "content": content }));
        }
        self.spawn_assistant_turn();
    }

    /// Spawn one network turn on a worker thread (Claude-only for now, #389). The
    /// result is collected by [`poll_assistant`](App::poll_assistant).
    fn spawn_assistant_turn(&mut self) {
        let key = match self.ai.resolved_key() {
            Some(k) => k.to_string(),
            None => {
                if let Some(a) = self.assistant_mut() {
                    a.note("The assistant needs the Claude backend (no API key).");
                }
                return;
            }
        };
        let model = self.ai.model.clone();
        let tools = tools::assistant_tools();
        let Some(a) = self.assistant_mut() else { return };
        let body = tool_loop::build_body(&model, Some(SYSTEM), &a.messages, &tools, MAX_OUTPUT_TOKENS);
        a.task.spawn(move || ClaudeBackend::new(key).respond(&body));
    }

    /// Collect a finished network turn (called each event-loop tick). A final
    /// answer ends the request; tool calls are run on the UI thread, fed back, and
    /// the next turn spawned.
    pub(crate) fn poll_assistant(&mut self) {
        let outcome = match self.assistant_mut() {
            Some(a) => match a.task.poll() {
                Poll::Pending => return,
                Poll::Ready(r) => r,
                Poll::Died => Err(AiError::Network { status: None, message: "the assistant request failed".into() }),
            },
            None => return,
        };
        self.apply_assistant_turn(outcome);
    }

    /// Apply one finished turn: a final answer ends the request; tool calls are run
    /// on the UI thread, fed back, and the next turn spawned. Split from the poll so
    /// the turn machine can be tested without a live backend.
    pub(super) fn apply_assistant_turn(&mut self, outcome: TurnResult) {
        // Take the split out so action execution can borrow `&mut self` freely —
        // `conv` then borrows the local split, not a field of `self`.
        let Some(mut split) = self.split.take() else { return };
        let OtherPane::Assistant { conv, .. } = &mut split.other else {
            self.split = Some(split);
            return;
        };

        let mut respawn = false;
        match outcome {
            Ok(Turn::Final(text)) => {
                let text = text.trim();
                if !text.is_empty() {
                    conv.entries.push(Entry::assistant(text));
                }
                conv.ran.clear();
                conv.turns = 0;
                conv.follow();
            }
            Ok(Turn::ToolCalls(_)) if conv.turns >= tool_loop::MAX_TURNS => {
                conv.entries.push(Entry::sys(&format!("Stopped after {} tool-use turns.", tool_loop::MAX_TURNS)));
                conv.ran.clear();
                conv.turns = 0;
                conv.follow();
            }
            Ok(Turn::ToolCalls(calls)) => {
                conv.turns += 1;
                // Replay the model's tool_use turn, then run each action here on the
                // UI thread and answer with a tool_result block (matched by id).
                conv.messages.push(tool_loop::tool_use_message(&calls));
                let mut results = Vec::with_capacity(calls.len());
                for c in &calls {
                    let result = self.assistant_run_action(&c.name);
                    conv.ran.push(c.name.clone());
                    conv.entries.push(Entry::tool(&format!("{} — {result}", c.name)));
                    results.push(tool_loop::tool_result_block(&c.id, &result));
                }
                conv.messages.push(tool_loop::tool_result_message(results));
                conv.follow();
                respawn = true;
            }
            Err(e) => {
                conv.entries.push(Entry::sys(&format!("Assistant error: {e}")));
                conv.ran.clear();
                conv.turns = 0;
                conv.follow();
            }
        }

        self.split = Some(split);
        if respawn {
            self.spawn_assistant_turn();
        }
    }

    /// Run one action the assistant asked for and return the result text fed back
    /// to the model — reusing the registry's availability check and dispatch, so
    /// the action's own status message becomes the model's feedback. Guarded to
    /// the runnable set even though the tool catalog is already filtered.
    fn assistant_run_action(&mut self, name: &str) -> String {
        let Some(action) = Action::from_name(name) else {
            return format!("no such action: {name}");
        };
        if !action.assistant_runnable() {
            return format!("{} can't be run from the assistant.", action.label());
        }
        if let Some(reason) = self.action_status(action) {
            return format!("unavailable: {reason}");
        }
        self.message = None;
        self.dispatch_action(action, false);
        self.message.take().unwrap_or_else(|| format!("Ran {}.", action.label()))
    }
}
