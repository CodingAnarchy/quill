//! The author-workflow runner (#226, #227): the run-workflow menu and the step
//! state machine. Workflow definitions and step resolution are pure and live in
//! `crate::workflow`; this drives the resolved actions through the action registry
//! (`action_status` / `dispatch_action`), running synchronous steps inline and
//! async AI steps off the UI thread (the existing worker-thread + `Outcome` path),
//! one at a time.

use std::time::{Duration, Instant};

use super::assist::AiStart;
use super::task::Task;
use super::*;

use crate::mode::{Action, AiAction};
use crate::workflow;

/// How long a single async step may run before the no-hang guardrail aborts it
/// and stops the workflow (#227). Generous — a real AI pass can take many
/// seconds; this only catches a wedged request, not a slow one.
const STEP_TIMEOUT: Duration = Duration::from_secs(120);

/// A workflow run in progress (#227): the resolved steps and the cursor through
/// them, plus the bookkeeping the state machine needs while a run is suspended on
/// an async step. Lives in `App::workflow_run` as `Some` for the run's lifetime.
pub(crate) struct WorkflowRun {
    /// The workflow's name, for progress and summary messages.
    name: String,
    /// The fully-resolved steps, in order.
    actions: Vec<Action>,
    /// The step currently running (or about to run); `actions.len()` when done.
    index: usize,
    /// Steps that completed (for the summary). Excludes skipped steps.
    ran: usize,
    /// AI steps skipped because AI is off (reported, not failed).
    skipped: usize,
    /// Set when the author asks to cancel: the run stops cleanly after the step
    /// in flight finishes, rather than tearing it down mid-request.
    cancelled: bool,
    /// When the in-flight async step was spawned, for the per-step timeout.
    started_at: Instant,
}

impl WorkflowRun {
    fn new(name: String, actions: Vec<Action>) -> Self {
        Self { name, actions, index: 0, ran: 0, skipped: 0, cancelled: false, started_at: Instant::now() }
    }
}

/// "step" / "steps" for a count.
fn steps(n: usize) -> &'static str {
    if n == 1 {
        "step"
    } else {
        "steps"
    }
}

impl App {
    /// Open the run-workflow menu (#226): the author's named action sequences,
    /// each annotated with what it runs (or why it can't). With none defined,
    /// explain how to add one rather than opening an empty menu.
    pub(crate) fn open_workflow_palette(&mut self) {
        if self.workflows.is_empty() {
            self.message =
                Some("No workflows defined — add a [workflows] section to config.toml".into());
            return;
        }
        let items = self
            .workflows
            .iter()
            .enumerate()
            .map(|(i, wf)| {
                let desc = match workflow::resolve(&self.workflows, &wf.name) {
                    Ok(actions) => format!("{} steps: {}", actions.len(), wf.steps.join(" → ")),
                    Err(e) => format!("⚠ {e}"),
                };
                PaletteItem { label: wf.name.clone(), desc, command: Command::RunWorkflow(i) }
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette { title: "Run workflow", items, selected: 0 }));
    }

    /// Start the workflow at `index`: resolve its steps (reporting an unknown
    /// step, the interactive ask step, or a cycle), then drive them. Synchronous
    /// steps run inline; the run suspends on each async AI step and resumes from
    /// [`poll_ai`](App::poll_ai) when its result lands.
    pub(crate) fn run_workflow(&mut self, index: usize) {
        let Some(name) = self.workflows.get(index).map(|w| w.name.clone()) else {
            return;
        };
        self.run_workflow_named(&name);
    }

    /// Start the named workflow — the core entry the menu (by index), the on-save
    /// trigger, and the key triggers (#228) all route through. Refuses to start a
    /// second run, or one while a manual AI request is in flight (no overlap).
    pub(crate) fn run_workflow_named(&mut self, name: &str) {
        if self.workflow_run.is_some() {
            self.message = Some("A workflow is already running".into());
            return;
        }
        if self.ai_task.is_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        let actions = match workflow::resolve(&self.workflows, name) {
            Ok(a) => a,
            Err(e) => {
                self.message = Some(format!("Workflow '{name}': {e}"));
                return;
            }
        };
        self.workflow_run = Some(WorkflowRun::new(name.to_string(), actions));
        self.advance_workflow();
    }

    /// Run the on-save workflow trigger (#228), if one is configured. A no-op when
    /// a workflow is already running — so a workflow's own `save` step can't
    /// re-trigger the on-save workflow (no re-entrancy).
    pub(crate) fn trigger_on_save(&mut self) {
        if self.workflow_run.is_some() {
            return;
        }
        if let Some(name) = self.triggers.on_save.clone() {
            self.run_workflow_named(&name);
        }
    }

    /// Drive the run forward from its current step: run synchronous steps inline
    /// until the run finishes or reaches an async step, which it spawns (or
    /// confirms first) and then suspends on. An async step with AI off is skipped
    /// and noted; any other unavailable step stops the run. Re-entrant: called to
    /// start the run, and again from `poll_ai` after each async step completes.
    pub(crate) fn advance_workflow(&mut self) {
        loop {
            let Some(run) = self.workflow_run.as_ref() else {
                return;
            };
            if run.index >= run.actions.len() {
                let (name, ran, skipped) = (run.name.clone(), run.ran, run.skipped);
                self.workflow_run = None;
                let tail = if skipped > 0 { format!(" ({skipped} skipped)") } else { String::new() };
                self.message = Some(format!("Ran workflow '{name}' — {ran} {}{tail}", steps(ran)));
                return;
            }
            let action = run.actions[run.index];

            // An async AI step with AI off is skipped (reported, not a failure).
            if !action.is_sync() && self.ai.check_ready().is_err() {
                if let Some(run) = self.workflow_run.as_mut() {
                    run.skipped += 1;
                    run.index += 1;
                }
                continue;
            }
            // Any other up-front unavailability stops the run.
            if let Some(reason) = self.action_status(action) {
                self.stop_run(&reason);
                return;
            }
            // Synchronous step: run it and move straight on.
            if action.is_sync() {
                self.dispatch_action(action, false);
                if let Some(run) = self.workflow_run.as_mut() {
                    run.ran += 1;
                    run.index += 1;
                }
                continue;
            }
            // Async AI step: confirm the cost or spawn it, then suspend. (Resolution
            // guarantees an async step is an AI pass — never the interactive ask; if
            // that ever breaks, stop rather than wedge on a step that never spawns.)
            let Action::Ai(ai_action) = action else {
                self.stop_run("internal: a non-AI step was treated as async");
                return;
            };
            match self.ai_action_start(ai_action, false) {
                AiStart::Confirm(label) => {
                    // Suspend at the cost confirm; the y/N handler resumes or cancels.
                    self.open_prompt(Pending::ConfirmAi(ai_action, false), label, "", None);
                    return;
                }
                AiStart::Run => {
                    self.start_async_step(ai_action);
                    return;
                }
            }
        }
    }

    /// Spawn the current step's async AI pass off the UI thread and show its
    /// progress. If the pass can't even start (backend build failure), stop the
    /// run with that reason rather than hanging on a result that won't arrive.
    /// Also the resume path for a step the author confirmed past the cost gate.
    pub(crate) fn start_async_step(&mut self, action: AiAction) {
        let Some((name, step, total)) = self.workflow_run.as_mut().map(|run| {
            run.started_at = Instant::now();
            (run.name.clone(), run.index + 1, run.actions.len())
        }) else {
            return;
        };
        self.run_ai_action(action, false);
        if !self.ai_task.is_busy() {
            // The spawn failed (e.g. the backend couldn't be built); the action's
            // own message says why. Frame it as the step failure and stop.
            let why = self.message.clone().unwrap_or_else(|| "couldn't start".into());
            self.stop_run(&why);
            return;
        }
        self.message = Some(format!(
            "Workflow '{name}' — step {step}/{total}: {}… (Esc to cancel)",
            Action::Ai(action).label(),
        ));
    }

    /// Resume a run after its in-flight async step finished (called from
    /// `poll_ai`). A no-op when no workflow is running. A step error stops the run;
    /// a pending cancel stops it cleanly here, the just-finished step counted;
    /// otherwise it advances to the next step.
    pub(crate) fn advance_after_ai(&mut self, step_error: Option<String>) {
        let Some(run) = self.workflow_run.as_mut() else {
            return;
        };
        if let Some(e) = step_error {
            self.stop_run(&e);
            return;
        }
        run.ran += 1;
        run.index += 1;
        if run.cancelled {
            let (name, ran) = (run.name.clone(), run.ran);
            self.finish_cancelled(&name, ran);
            return;
        }
        self.advance_workflow();
    }

    /// Ask the running workflow to stop after the current step finishes (#227) —
    /// the clean cancel bound to Esc. The in-flight async step is left to complete;
    /// [`advance_after_ai`](App::advance_after_ai) sees the flag and stops there.
    pub(crate) fn cancel_workflow(&mut self) {
        if let Some(run) = self.workflow_run.as_mut() {
            run.cancelled = true;
            self.message = Some(format!("Workflow '{}': cancelling after this step…", run.name));
        }
    }

    /// Cancel a run immediately, with nothing in flight — the path when the author
    /// declines an async step's cost confirm. Reports what ran. No-op if idle.
    pub(crate) fn decline_workflow(&mut self) {
        if let Some(run) = self.workflow_run.as_ref() {
            let (name, ran) = (run.name.clone(), run.ran);
            self.finish_cancelled(&name, ran);
        }
    }

    /// The no-hang guardrail (#227): if the in-flight async step has run past the
    /// per-step timeout, drop it (its orphaned worker's result is ignored) and
    /// stop the run. Called each poll; a no-op unless a step is actually stuck.
    pub(crate) fn tick_workflow_timeout(&mut self) {
        let Some(run) = self.workflow_run.as_ref() else {
            return;
        };
        if !self.ai_task.is_busy() || run.started_at.elapsed() < STEP_TIMEOUT {
            return;
        }
        self.ai_task = Task::idle();
        self.stop_run("timed out");
    }

    /// Stop the run at its current step with a reason — the one place the
    /// "stopped at step N (action): why" message is built, and the run cleared.
    fn stop_run(&mut self, reason: &str) {
        if let Some(run) = self.workflow_run.take() {
            let action = run.actions[run.index];
            self.message = Some(format!(
                "Workflow '{}' stopped at step {} ({}): {reason}",
                run.name,
                run.index + 1,
                action.name(),
            ));
        }
    }

    /// Clear the run and report it cancelled with what ran — shared by the
    /// after-step cancel and the declined-confirm cancel.
    fn finish_cancelled(&mut self, name: &str, ran: usize) {
        self.workflow_run = None;
        self.message = Some(format!("Workflow '{name}' cancelled — {ran} {} ran", steps(ran)));
    }

    /// Test seam: put a run in progress at its first step, so the async-step state
    /// transitions (`advance_after_ai`, cancel, timeout) can be exercised without
    /// a live backend.
    #[cfg(test)]
    pub(crate) fn begin_workflow_for_test(&mut self, name: &str, actions: Vec<Action>) {
        self.workflow_run = Some(WorkflowRun::new(name.into(), actions));
    }

    /// Test seam: backdate the in-flight step so the per-step timeout has elapsed.
    #[cfg(test)]
    pub(crate) fn expire_workflow_step(&mut self) {
        if let Some(run) = self.workflow_run.as_mut() {
            run.started_at = Instant::now().checked_sub(STEP_TIMEOUT * 2).unwrap_or_else(Instant::now);
        }
    }
}
