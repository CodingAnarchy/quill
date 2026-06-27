//! The author-workflow runner (#226, #227): the run-workflow menu and the step
//! state machine. Workflow definitions and step resolution are pure and live in
//! `crate::workflow`; this drives the resolved actions through the action registry
//! (`action_status` / `dispatch_action`), running synchronous steps inline and
//! async AI steps off the UI thread (the existing worker-thread + `Outcome` path),
//! one at a time.

use super::palette::AiStart;
use super::sequenced::{plural, Sequenced, SequencedRun};
use super::*;

use crate::mode::{Action, AiAction};
use crate::workflow;

/// A workflow run in progress (#227): the resolved steps plus the shared
/// sequenced-run bookkeeping (#344). Lives in `App::workflow_run` as `Some` for
/// the run's lifetime.
pub(crate) struct WorkflowRun {
    /// The workflow's name, for progress and summary messages.
    name: String,
    /// The fully-resolved steps, in order.
    actions: Vec<Action>,
    /// Cursor, tallies, cancel flag, and the in-flight step's start time (#344).
    seq: SequencedRun,
}

impl WorkflowRun {
    fn new(name: String, actions: Vec<Action>) -> Self {
        Self {
            name,
            actions,
            seq: SequencedRun::new(),
        }
    }
}

/// The workflow runner's [`Sequenced`] behavior (#344) — the marker the generic
/// advance/timeout/cancel dispatch through.
pub(crate) struct WorkflowDriver;

impl Sequenced for WorkflowDriver {
    fn run_mut(app: &mut App) -> Option<&mut SequencedRun> {
        app.workflow_run.as_mut().map(|r| &mut r.seq)
    }
    fn run_ref(app: &App) -> Option<&SequencedRun> {
        app.workflow_run.as_ref().map(|r| &r.seq)
    }
    fn drive(app: &mut App) {
        app.advance_workflow();
    }
    fn stop(app: &mut App, reason: &str) {
        app.stop_run(reason);
    }
    fn finish(app: &mut App) {
        if let Some(run) = app.workflow_run.as_ref() {
            let (name, ran) = (run.name.clone(), run.seq.ran);
            app.finish_cancelled(&name, ran);
        }
    }
    fn cancelling_message(app: &App) -> String {
        let name = app
            .workflow_run
            .as_ref()
            .map(|r| r.name.as_str())
            .unwrap_or("");
        format!("Workflow '{name}': cancelling after this step…")
    }
}

impl App {
    /// Open the run-workflow menu (#226): the author's named action sequences,
    /// each annotated with what it runs (or why it can't). With none defined,
    /// explain how to add one rather than opening an empty menu.
    pub(crate) fn open_workflow_palette(&mut self) {
        // The menu always offers to build a new workflow from a description (#232),
        // so it's useful even before any workflow is defined.
        let mut items = vec![PaletteItem {
            label: "✎ New workflow from a description…".into(),
            desc: "describe a routine in words; the AI drafts it".into(),
            command: Command::NewWorkflowFromPrompt,
        }];
        items.extend(self.workflows.iter().enumerate().map(|(i, wf)| {
            let desc = match workflow::resolve(&self.workflows, &wf.name) {
                Ok(actions) => format!("{} steps: {}", actions.len(), wf.steps.join(" → ")),
                Err(e) => format!("⚠ {e}"),
            };
            PaletteItem {
                label: wf.name.clone(),
                desc,
                command: Command::RunWorkflow(i),
            }
        }));
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Workflows",
            items,
            selected: 0,
        }));
    }

    /// The vocabulary an AI workflow draft (#232) may choose steps from: every
    /// registry action that's valid as a workflow step (so not the interactive
    /// "ask"), each with its label, plus the existing workflow names.
    pub(crate) fn workflow_vocab(&self) -> Vec<(String, String)> {
        let mut vocab: Vec<(String, String)> = Action::ALL
            .iter()
            .filter(|a| !matches!(a, Action::Ai(AiAction::Ask)))
            .map(|a| (a.name().to_string(), a.label().to_string()))
            .collect();
        for w in &self.workflows {
            vocab.push((w.name.clone(), format!("run the '{}' workflow", w.name)));
        }
        vocab
    }

    /// Save the AI-proposed workflow (#232) under `name` after the author confirms
    /// it: writes the `[workflows]` entry to config.toml and adds it live. Refuses
    /// an empty name or one that already exists (re-prompting rather than clobber).
    pub(crate) fn save_generated_workflow(&mut self, name: &str) {
        let name = name.trim().to_string();
        let Some(steps) = self.pending_workflow.take() else {
            return;
        };
        if name.is_empty() {
            self.message = Some("Workflow name can't be empty — cancelled".into());
            return;
        }
        if self.workflows.iter().any(|w| w.name == name) {
            self.pending_workflow = Some(steps);
            self.open_prompt(
                Pending::NameNewWorkflow,
                format!("'{name}' already exists — save as: "),
                &name,
                None,
            );
            return;
        }
        match crate::config::add_workflow(&name, &steps) {
            Ok(path) => {
                self.workflows.push(workflow::Workflow {
                    name: name.clone(),
                    steps,
                });
                self.workflows.sort_by(|a, b| a.name.cmp(&b.name));
                self.message = Some(format!("Saved workflow '{name}' to {}", path.display()));
            }
            Err(e) => self.message = Some(format!("Couldn't save workflow: {e}")),
        }
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

    /// Run the on-save workflow trigger (#228), if one is configured, after an
    /// explicit save. A no-op when a workflow is already running — so a workflow's
    /// own `save` step can't re-trigger the on-save workflow (no re-entrancy), and
    /// without the "already running" message that path would otherwise show.
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
            if run.seq.index >= run.actions.len() {
                let (name, ran, skipped) = (run.name.clone(), run.seq.ran, run.seq.skipped);
                self.workflow_run = None;
                let tail = if skipped > 0 {
                    format!(" ({skipped} skipped)")
                } else {
                    String::new()
                };
                self.message = Some(format!(
                    "Ran workflow '{name}' — {ran} {}{tail}",
                    plural(ran, "step", "steps")
                ));
                return;
            }
            let action = run.actions[run.seq.index];

            // An async AI step with AI off is skipped (reported, not a failure).
            if !action.is_sync() && self.ai.check_ready().is_err() {
                if let Some(run) = self.workflow_run.as_mut() {
                    run.seq.skipped += 1;
                    run.seq.index += 1;
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
                    run.seq.ran += 1;
                    run.seq.index += 1;
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
                AiStart::Confirm { label, offer_local } => {
                    // Suspend at the cost confirm; the handler resumes or cancels.
                    self.open_prompt(
                        Pending::ConfirmAi {
                            action: ai_action,
                            force: false,
                            offer_local,
                        },
                        label,
                        "",
                        None,
                    );
                    return;
                }
                AiStart::Run(backend) => {
                    // Route this step to the backend (and local model) the router picked.
                    self.route_request(backend, ai_action, false);
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
            run.seq.touch();
            (run.name.clone(), run.seq.index + 1, run.actions.len())
        }) else {
            return;
        };
        self.run_ai_action(action, false);
        if !self.ai_task.is_busy() {
            // The spawn failed (e.g. the backend couldn't be built); the action's
            // own message says why. Frame it as the step failure and stop.
            let why = self
                .message
                .clone()
                .unwrap_or_else(|| "couldn't start".into());
            self.stop_run(&why);
            return;
        }
        self.message = Some(format!(
            "Workflow '{name}' — step {step}/{total}: {}… (Esc to cancel)",
            Action::Ai(action).label(),
        ));
    }

    /// Cancel a run immediately, with nothing in flight — the path when the author
    /// declines an async step's cost confirm. Reports what ran. No-op if idle.
    pub(crate) fn decline_workflow(&mut self) {
        if let Some(run) = self.workflow_run.as_ref() {
            let (name, ran) = (run.name.clone(), run.seq.ran);
            self.finish_cancelled(&name, ran);
        }
    }

    /// Stop the run at its current step with a reason — the one place the
    /// "stopped at step N (action): why" message is built, and the run cleared.
    /// The [`WorkflowDriver`]'s [`Sequenced::stop`] hook (#344).
    fn stop_run(&mut self, reason: &str) {
        if let Some(run) = self.workflow_run.take() {
            let action = run.actions[run.seq.index];
            self.message = Some(format!(
                "Workflow '{}' stopped at step {} ({}): {reason}",
                run.name,
                run.seq.index + 1,
                action.name(),
            ));
        }
    }

    /// Clear the run and report it cancelled with what ran — shared by the
    /// after-step cancel and the declined-confirm cancel.
    fn finish_cancelled(&mut self, name: &str, ran: usize) {
        self.workflow_run = None;
        self.message = Some(format!(
            "Workflow '{name}' cancelled — {ran} {} ran",
            plural(ran, "step", "steps")
        ));
    }

    /// Test seam: put a run in progress at its first step, so the async-step state
    /// transitions (advance, cancel, timeout) can be exercised without a backend.
    #[cfg(test)]
    pub(crate) fn begin_workflow_for_test(&mut self, name: &str, actions: Vec<Action>) {
        self.workflow_run = Some(WorkflowRun::new(name.into(), actions));
    }

    /// Test seam: backdate the in-flight step so the per-step timeout has elapsed.
    #[cfg(test)]
    pub(crate) fn expire_workflow_step(&mut self) {
        use super::sequenced::STEP_TIMEOUT;
        use std::time::Instant;
        if let Some(run) = self.workflow_run.as_mut() {
            run.seq.started_at = Instant::now()
                .checked_sub(STEP_TIMEOUT * 2)
                .unwrap_or_else(Instant::now);
        }
    }
}
