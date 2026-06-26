//! The authorial assistant pane (#307): a docked conversation with the tool-use
//! assistant (#306). The author types a request in plain language; the model
//! answers and runs editor actions on their behalf, with the conversation kept
//! across turns while the pane is open.
//!
//! # Threading — a UI-thread turn machine
//!
//! The tool-use loop has two seams that pull in opposite directions: the network
//! turn must run *off* the UI thread (so typing never freezes), but executing an
//! action mutates the editor and must run *on* it. Rather than run a whole loop
//! off-thread and ferry every action back over a channel, the pane drives the loop
//! one turn at a time from the UI thread, reusing the [`tool_loop`] building blocks:
//!
//! 1. A request spawns a single network turn on a worker via [`Task`].
//! 2. [`App::poll_assistant`] (each event-loop tick) collects the result. A final
//!    answer ends the request; tool calls are run on the UI thread via
//!    [`App::dispatch_action`], their results fed back, and the next turn spawned.
//!
//! # Async passes (#392)
//!
//! Most tools finish during the call (save, export, reports). The **AI passes**
//! (continuity, characters, style, …) don't: each spawns its own background
//! request whose findings arrive later via [`App::poll_ai`]. So a turn's tool
//! calls run as a *batch* that can **park**: when a call is an async pass, the
//! batch starts it and stops; [`App::poll_ai`] resumes the batch with the pass's
//! result once it lands. Costly passes confirm first, in the conversation itself
//! (a `y`/skip reply) rather than a modal that would fight the pane for keys.
//!
//! So the building blocks of #306 are reused — the registry-derived tools, the
//! cached-manuscript body ([`tool_loop::build_body`]), the [`Converse`] transport,
//! the shared message helpers — but the orchestration lives where the editor
//! state does. Esc cancels (dropping the turn and any pending pass); the
//! conversation persists for the next request. Claude-only for now (#389).

use std::collections::VecDeque;

use serde_json::{json, Value};

use crate::ai::tool_loop::{self, ToolCall, Turn};
use crate::ai::{cost, tools, AiError, BackendKind, MAX_OUTPUT_TOKENS};
use crate::mode::{Action, AiAction};

use super::palette::AiStart;
use super::task::{Poll, Task};
use super::{App, OtherPane, Split};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The system prompt: the assistant's brief, including the manuscript-prose
/// boundary (CLAUDE.md) so the model never drifts into co-writing.
const SYSTEM: &str = "You are the authorial assistant inside Quill, a terminal novel editor. \
Help the author by running editor actions on their behalf and answering questions about their \
manuscript, which is provided as context. Use the available tools (saving, exporting, opening \
reports, navigating spelling flags, toggling view modes, and the analysis passes — continuity, \
characters, setting, style, naming, scaffold, voice, editorial) to carry out a request, then \
briefly say what you did; an analysis pass's findings open in the editor for the author to act on. \
You are an assistant, never a co-writer. You cannot write or change the author's prose, and you \
have no tool that does. If a request would draft, rewrite, tighten, expand, reword, or otherwise \
produce or edit manuscript prose, refuse and redirect: say plainly that you can't write or change \
the prose, then offer what you can do instead — flag continuity, style, or voice issues for them \
to fix, run an analysis pass, or open a report. The author always writes and places the words \
themselves. Keep replies short and plain.";

/// One off-thread network turn's result.
type TurnResult = Result<Turn, AiError>;

/// What running one tool call produced: a result text now, an async pass started
/// (the batch parks until [`App::poll_ai`] delivers it), or a cost confirmation
/// the author must answer in the conversation first.
enum Step {
    Sync(String),
    Started,
    AwaitConfirm(String),
}

/// Who spoke a transcript line, so the renderer can style it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Speaker {
    /// The author's request.
    You,
    /// The model's prose reply.
    Assistant,
    /// An action the assistant ran, and its result.
    Tool,
    /// A status note from Quill (ready, canceled, errors, cost confirms).
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
    /// The current turn's tool calls not yet run (front first), and the results
    /// collected so far — the batch that parks on an async pass (#392).
    batch: VecDeque<ToolCall>,
    batch_results: Vec<Value>,
    /// The call whose AI pass is in flight, awaiting [`App::poll_ai`] (#392).
    awaiting: Option<ToolCall>,
    /// A call whose costly pass needs the author's `y`/skip in the conversation
    /// before it runs (#392 / #82): the pane waits for the next input as the answer.
    pending_confirm: Option<ToolCall>,
    /// The current request's estimated spend so far (#309): a running ceiling over
    /// the assistant's own network turns (the manuscript cached, not re-billed).
    spent: f64,
    /// Whether the author approved continuing this run past the cost threshold
    /// (#309) — once given, the run finishes without asking again.
    over_budget_ok: bool,
    /// True while the pane waits for the author's `y`/stop answer to a budget
    /// confirm (#309): the next input is that answer, not a new request.
    pending_continue: bool,
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
            batch: VecDeque::new(),
            batch_results: Vec::new(),
            awaiting: None,
            pending_confirm: None,
            spent: 0.0,
            over_budget_ok: false,
            pending_continue: false,
            ran: Vec::new(),
            turns: 0,
        };
        a.entries.push(Entry::sys(
            "Type a request and press Enter. Esc cancels a run; Ctrl-w c closes the pane.",
        ));
        a
    }

    /// Whether the model (a network turn) or one of its passes is in flight — the
    /// "thinking" state, in which the request line is locked.
    pub(crate) fn is_running(&self) -> bool {
        self.task.is_busy() || self.awaiting.is_some()
    }

    /// Whether anything is in progress, including a pending cost or budget
    /// confirmation — what Esc cancels.
    fn is_active(&self) -> bool {
        self.is_running() || self.pending_confirm.is_some() || self.pending_continue
    }

    /// Abandon the current request: drop the turn, any parked batch / pass /
    /// confirm, and reset the per-request counters. A pass already in flight still
    /// completes and surfaces its findings; we just stop waiting on it.
    fn cancel(&mut self) {
        self.task = Task::idle();
        self.batch.clear();
        self.batch_results.clear();
        self.awaiting = None;
        self.pending_confirm = None;
        self.pending_continue = false;
        self.ran.clear();
        self.turns = 0;
        self.entries.push(Entry::sys("Canceled."));
        self.follow();
    }

    /// Reset the per-request cost tracking (#309) — a new request starts fresh.
    fn reset_spend(&mut self) {
        self.spent = 0.0;
        self.over_budget_ok = false;
        self.pending_continue = false;
    }

    /// The estimated spend on the current request so far (#309), for the renderer.
    pub(crate) fn spent(&self) -> f64 {
        self.spent
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
    pub(super) fn assistant(&self) -> Option<&Assistant> {
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

    /// Hand keys back to the editor (so a pass's findings UI is interactive while
    /// the assistant keeps working in the background).
    fn blur_assistant_pane(&mut self) {
        if let Some(Split { other: OtherPane::Assistant { focused, .. }, .. }) = self.split.as_mut() {
            *focused = false;
        }
    }

    /// Handle a key while the assistant pane has focus (#307): edit the request
    /// line, submit it, scroll the transcript, or cancel / unfocus on Esc.
    pub(crate) fn on_key_assistant(&mut self, key: KeyEvent) {
        let active = self.assistant().is_some_and(Assistant::is_active);
        // While a turn or pass runs the request line is locked; a pending cost
        // confirm leaves it open so the author can type the `y`/skip answer.
        let locked = self.assistant().is_some_and(Assistant::is_running);
        let max = self.assistant_scroll_max.get();
        match key.code {
            KeyCode::Enter => self.submit_assistant(), // submit handles all gating
            KeyCode::Esc if active => {
                if let Some(c) = self.assistant_mut() {
                    c.cancel();
                }
            }
            // Esc (idle) hands keys back to the editor; the pane stays docked.
            KeyCode::Esc => self.blur_assistant_pane(),
            _ => {
                let Some(c) = self.assistant_mut() else { return };
                match key.code {
                    KeyCode::Backspace if !locked => {
                        c.input.pop();
                    }
                    KeyCode::Char(ch) if !locked && !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        c.input.push(ch);
                    }
                    KeyCode::Up => c.scroll_up = (c.scroll_up + 1).min(max),
                    KeyCode::Down => c.scroll_up = c.scroll_up.saturating_sub(1),
                    KeyCode::PageUp => c.scroll_up = (c.scroll_up + 5).min(max),
                    KeyCode::PageDown => c.scroll_up = c.scroll_up.saturating_sub(5),
                    _ => {}
                }
            }
        }
    }

    /// Submit the typed line: the answer to a pending cost confirm, or — when
    /// idle — a fresh request that appends to the conversation and spawns the first
    /// network turn. A no-op while a run is in progress or the line is empty.
    fn submit_assistant(&mut self) {
        // A pending confirmation consumes this line as the answer, not a new request.
        if self.assistant().is_some_and(|c| c.pending_confirm.is_some()) {
            self.answer_assistant_confirm();
            return;
        }
        if self.assistant().is_some_and(|c| c.pending_continue) {
            self.answer_assistant_continue();
            return;
        }

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
            a.reset_spend(); // a new request starts a fresh budget (#309)
            a.entries.push(Entry::you(&request));
            a.follow();
            (request, a.messages.is_empty())
        };

        // AI off / not configured: explain in the pane rather than failing obscurely.
        // Checks the session backend (#310), so an offline/local setup is "ready".
        if let Err(e) = self.ai.check_ready() {
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

    /// Spawn one tool-use turn on a worker thread, on the session backend — Claude
    /// or the local model (#310). The backend is resolved on the UI thread and
    /// built (loading the local model if needed) on the worker; the result is
    /// collected by [`poll_assistant`](App::poll_assistant).
    fn spawn_assistant_turn(&mut self) {
        // Budget gate (#309): the first turn always runs (the request can't proceed
        // otherwise); once a run's projected spend crosses the threshold, confirm in
        // the conversation before each further turn — until the author approves.
        let est = self.assistant_turn_estimate();
        let threshold = self.ai.cost_threshold;
        let over_budget = self
            .assistant()
            .is_some_and(|c| c.spent > 0.0 && c.spent + est > threshold && !c.over_budget_ok);
        if over_budget {
            if let Some(a) = self.assistant_mut() {
                a.note(&format!(
                    "This run has cost about ${:.2}; continuing may pass your ${threshold:.2} threshold. \
                     Reply 'y' to continue, anything else to stop.",
                    a.spent,
                ));
                a.pending_continue = true;
            }
            return;
        }

        let spec = match self.assistant_backend() {
            Ok(spec) => spec,
            Err(e) => {
                if let Some(a) = self.assistant_mut() {
                    a.note(&format!("The assistant is unavailable: {e}"));
                }
                return;
            }
        };
        let model = self.ai.model.clone();
        let tools = tools::assistant_tools();
        let Some(a) = self.assistant_mut() else { return };
        a.spent += est; // count this turn against the run's budget (#309)
        let body = tool_loop::build_body(&model, Some(SYSTEM), &a.messages, &tools, MAX_OUTPUT_TOKENS);
        a.task.spawn(move || match spec.build() {
            Ok(backend) => match backend.as_converse() {
                Some(conv) => conv.respond(&body),
                None => Err(AiError::NotConfigured("this backend has no tool-use support".into())),
            },
            Err(e) => Err(e),
        });
    }

    /// A ceiling estimate (#309) for the turn about to be sent: the manuscript as a
    /// cached prefix plus the fresh conversation, on the session backend. Zero on
    /// the local backend — on-device runs are free, so there's nothing to gate.
    pub(super) fn assistant_turn_estimate(&self) -> f64 {
        if self.ai.backend() != BackendKind::Claude {
            return 0.0;
        }
        let Some(a) = self.assistant() else { return 0.0 };
        // The manuscript is the first user block (the cached prefix); everything
        // else — the request, tool calls, results, replies — is fresh this turn.
        let (mut cached, mut fresh) = (0usize, 0usize);
        for (mi, m) in a.messages.iter().enumerate() {
            if let Some(blocks) = m["content"].as_array() {
                for (bi, b) in blocks.iter().enumerate() {
                    let len = b["text"].as_str().or_else(|| b["content"].as_str()).map_or(0, str::len);
                    if mi == 0 && bi == 0 {
                        cached += len;
                    } else {
                        fresh += len;
                    }
                }
            }
        }
        cost::estimate_turn(cached, fresh, MAX_OUTPUT_TOKENS, &self.ai.model).dollars
    }

    /// The author's `y`/stop answer to a budget confirm (#309): approve the rest of
    /// the run (no more asking) and continue, or stop it within budget.
    fn answer_assistant_continue(&mut self) {
        let yes = {
            let Some(a) = self.assistant_mut() else { return };
            let yes = matches!(a.input.trim().chars().next(), Some('y' | 'Y'));
            a.input.clear();
            a.pending_continue = false;
            yes
        };
        if yes {
            if let Some(a) = self.assistant_mut() {
                a.over_budget_ok = true;
            }
            self.spawn_assistant_turn();
        } else if let Some(a) = self.assistant_mut() {
            a.entries.push(Entry::sys("Stopped to stay within budget."));
            a.ran.clear();
            a.turns = 0;
            a.follow();
        }
    }

    /// Collect a finished network turn (called each event-loop tick). A final
    /// answer ends the request; tool calls become a batch the turn machine drives.
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

    /// Apply one finished turn: a final answer ends the request; tool calls are
    /// queued as a batch and driven. Split from the poll so the turn machine can be
    /// tested without a live backend.
    pub(super) fn apply_assistant_turn(&mut self, outcome: TurnResult) {
        // Take the split out so action execution can borrow `&mut self` freely —
        // `conv` then borrows the local split, not a field of `self`.
        let Some(mut split) = self.split.take() else { return };
        let OtherPane::Assistant { conv, .. } = &mut split.other else {
            self.split = Some(split);
            return;
        };

        let mut drive = false;
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
                // Replay the model's tool_use turn; the results follow once the
                // batch finishes (a tool_result block per call, matched by id).
                conv.messages.push(tool_loop::tool_use_message(&calls));
                conv.batch = calls.into();
                conv.batch_results.clear();
                conv.follow();
                drive = true;
            }
            Err(e) => {
                conv.entries.push(Entry::sys(&format!("Assistant error: {e}")));
                conv.ran.clear();
                conv.turns = 0;
                conv.follow();
            }
        }

        self.split = Some(split);
        if drive {
            self.drive_assistant_batch();
        }
    }

    /// Run the queued tool calls in order, on the UI thread, until the batch empties
    /// (then send the results back and ask the model to continue) or a call parks
    /// the batch — an async pass started, or a cost confirm posted (#392).
    fn drive_assistant_batch(&mut self) {
        let Some(mut split) = self.split.take() else { return };
        let OtherPane::Assistant { conv, focused } = &mut split.other else {
            self.split = Some(split);
            return;
        };

        while let Some(call) = conv.batch.pop_front() {
            match self.assistant_run_action(&call.name, &call.input) {
                Step::Sync(result) => {
                    conv.ran.push(call.name.clone());
                    conv.entries.push(Entry::tool(&format!("{} — {result}", call.name)));
                    conv.batch_results.push(tool_loop::tool_result_block(&call.id, &result));
                }
                Step::Started => {
                    conv.ran.push(call.name.clone());
                    conv.entries.push(Entry::tool(&format!("{} — running…", call.name)));
                    conv.awaiting = Some(call);
                    conv.follow();
                    // Yield focus so the pass's findings UI is interactive.
                    *focused = false;
                    self.split = Some(split);
                    return;
                }
                Step::AwaitConfirm(note) => {
                    conv.entries.push(Entry::sys(&note));
                    conv.pending_confirm = Some(call);
                    conv.follow();
                    self.split = Some(split);
                    return;
                }
            }
        }

        // All calls done — send the results back and ask the model to continue.
        let results = std::mem::take(&mut conv.batch_results);
        conv.messages.push(tool_loop::tool_result_message(results));
        conv.follow();
        self.split = Some(split);
        self.spawn_assistant_turn();
    }

    /// Resume a parked batch once an assistant-initiated pass finishes (#392),
    /// called from [`poll_ai`](App::poll_ai) with the pass's result summary.
    pub(super) fn resume_assistant_pass(&mut self, summary: String) {
        let call = self.assistant_mut().and_then(|c| c.awaiting.take());
        let Some(call) = call else { return };
        self.record_assistant_result(&call, &summary);
        self.drive_assistant_batch();
    }

    /// The result text for an assistant-initiated pass / action that just finished,
    /// or `None` when the pane isn't waiting on one (so [`poll_ai`](App::poll_ai)
    /// resumes only its own work). Read before the outcome is consumed. A Q&A answer
    /// (#390) comes back in full — it *is* the result the author asked for — while
    /// the analysis passes report that their findings opened in the editor.
    pub(super) fn assistant_pass_summary(&self, outcome: &super::Outcome) -> Option<String> {
        let call = self.assistant()?.awaiting.as_ref()?;
        let label = Action::from_name(&call.name).map_or("the action", Action::label);
        if let Some(e) = outcome.error() {
            return Some(format!("error: {e}"));
        }
        if let super::Outcome::Answer(Ok(resp)) = outcome {
            return Some(resp.text.clone());
        }
        Some(format!("Done — {label}. The result is in the editor."))
    }

    /// The author's `y`/skip answer to a pending cost confirm (#392): run the pass
    /// on Claude, or skip it, then drive the rest of the batch.
    fn answer_assistant_confirm(&mut self) {
        let (yes, call) = {
            let Some(c) = self.assistant_mut() else { return };
            let yes = matches!(c.input.trim().chars().next(), Some('y' | 'Y'));
            c.input.clear();
            (yes, c.pending_confirm.take())
        };
        let Some(call) = call else { return };

        if yes {
            if let Some(Action::Ai(a)) = Action::from_name(&call.name) {
                if self.fire_pass(a, BackendKind::Claude) {
                    if let Some(c) = self.assistant_mut() {
                        c.ran.push(call.name.clone());
                        c.entries.push(Entry::tool(&format!("{} — running…", call.name)));
                        c.awaiting = Some(call);
                        c.follow();
                    }
                    self.blur_assistant_pane();
                    return; // parked for poll_ai
                }
            }
            let msg = self.message.take().unwrap_or_else(|| "couldn't start the pass".into());
            self.record_assistant_result(&call, &msg);
        } else {
            self.record_assistant_result(&call, "skipped by the author");
        }
        self.drive_assistant_batch();
    }

    /// Record one call's result into the transcript and the batch, for the model.
    fn record_assistant_result(&mut self, call: &ToolCall, text: &str) {
        if let Some(c) = self.assistant_mut() {
            c.ran.push(call.name.clone());
            c.entries.push(Entry::tool(&format!("{} — {text}", call.name)));
            c.batch_results.push(tool_loop::tool_result_block(&call.id, text));
            c.follow();
        }
    }

    /// Run one tool the assistant asked for, returning how it resolved: a
    /// synchronous result, an async pass started, or a cost confirm to post.
    /// Reuses the registry's availability check and dispatch.
    ///
    /// This is the prose-boundary guardrail's execution layer (#308): only an
    /// action on the registry-derived allowlist ([`Action::assistant_runnable`])
    /// ever runs — anything else is blocked here, in code, no matter what the
    /// model emits. The allowlist holds no prose-writing action, so the assistant
    /// has no capability to draft or edit chapter prose; the prompt's
    /// refuse-and-redirect is the first layer, this is the guarantee.
    fn assistant_run_action(&mut self, name: &str, input: &Value) -> Step {
        let Some(action) = Action::from_name(name) else {
            return Step::Sync(format!(
                "Blocked: there is no action '{name}'. You can't write or edit prose — \
                 offer the author a safe action (an analysis pass, a report) instead."
            ));
        };
        if !action.assistant_runnable() {
            return Step::Sync(format!(
                "Blocked: '{name}' is not a permitted assistant action. You can't write or edit \
                 prose — offer to flag continuity, style, or voice issues, or another safe action."
            ));
        }
        if let Some(reason) = self.action_status(action) {
            return Step::Sync(format!("unavailable: {reason}"));
        }
        // An action that takes a free-text argument (#390): run it with the value
        // the model supplied — under its named parameter (Claude's native tools) or
        // the generic `argument` key (the local constrained-choice path) — instead
        // of the interactive prompt the assistant can't answer.
        if let Some(arg_key) = action.text_arg() {
            let text = input
                .get(arg_key)
                .or_else(|| input.get("argument"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if text.is_empty() {
                return Step::Sync(format!("{} needs a '{arg_key}' argument.", action.label()));
            }
            return self.fire_text_action(action, &text);
        }
        if let Action::Ai(a) = action {
            return self.start_assistant_pass(a, action.label());
        }
        self.message = None;
        self.dispatch_action(action, false);
        Step::Sync(self.message.take().unwrap_or_else(|| format!("Ran {}.", action.label())))
    }

    /// Fire a text-argument AI action (#390) with the model's argument, on the
    /// session backend. These run as a background request (like the analysis
    /// passes) so the batch parks until [`poll_ai`](App::poll_ai) resumes it; the
    /// Q&A answer comes back into the conversation (see `assistant_pass_summary`).
    fn fire_text_action(&mut self, action: Action, text: &str) -> Step {
        self.message = None;
        match action {
            Action::Ai(AiAction::Ask) => self.ask_ai(text),
            Action::Ai(AiAction::Brainstorm) => self.brainstorm(text),
            Action::Ai(AiAction::DraftCharacters) => self.draft_characters(text),
            Action::Ai(AiAction::DraftSetting) => self.draft_setting(text),
            _ => return Step::Sync(format!("{} can't take an argument here.", action.label())),
        }
        if self.ai_task.is_busy() {
            Step::Started
        } else {
            Step::Sync(self.message.take().unwrap_or_else(|| "couldn't start the action".into()))
        }
    }

    /// Begin an AI analysis pass for the assistant: run it now when the cost is
    /// under the threshold (or the router resolves a backend), or post a cost
    /// confirm to the conversation when it isn't (#392 / #82). The in-pane confirm
    /// is a plain y/skip on the cloud backend (the router's `offer_local` choice
    /// isn't surfaced here).
    fn start_assistant_pass(&mut self, action: AiAction, label: &str) -> Step {
        match self.ai_action_start(action, false) {
            AiStart::Run(backend) => {
                if self.fire_pass(action, backend) {
                    Step::Started
                } else {
                    Step::Sync(self.message.take().unwrap_or_else(|| "couldn't start the pass".into()))
                }
            }
            AiStart::Confirm { label: cost, .. } => {
                // The cost label is built for the modal confirm ("… — run? (y/N)" /
                // "… — y Claude · l local · N cancel"); keep just the estimate (the
                // part before the first " — ") and ask for a y/skip here instead.
                let estimate = cost.split(" — ").next().unwrap_or(&cost);
                Step::AwaitConfirm(format!("{label} would cost {estimate}. Reply 'y' to run it, anything else to skip."))
            }
        }
    }

    /// Fire an AI pass on `backend`, returning whether it actually started (a
    /// request already in flight, say, leaves the status message instead).
    fn fire_pass(&mut self, action: AiAction, backend: BackendKind) -> bool {
        self.message = None;
        self.run_confirmed_ai(action, false, backend);
        self.ai_task.is_busy()
    }

    /// Park a pass and queue the rest of a batch, so [`resume_assistant_pass`] can
    /// be tested without a live backend (#392).
    #[cfg(test)]
    pub(super) fn test_install_pass(&mut self, awaiting: ToolCall, rest: Vec<ToolCall>) {
        if let Some(c) = self.assistant_mut() {
            c.awaiting = Some(awaiting);
            c.batch = rest.into();
            c.batch_results.clear();
        }
    }

    /// Pretend the current run has already spent `dollars`, so the budget gate
    /// (#309) can be tested without driving real turns.
    #[cfg(test)]
    pub(super) fn test_set_spent(&mut self, dollars: f64) {
        if let Some(c) = self.assistant_mut() {
            c.spent = dollars;
        }
    }

    /// Drive one turn spawn, exercising the budget gate (#309) without a backend.
    #[cfg(test)]
    pub(super) fn test_spawn_turn(&mut self) {
        self.spawn_assistant_turn();
    }
}
