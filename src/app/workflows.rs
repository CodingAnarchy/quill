//! The author-workflow runner (#226): the run-workflow menu and synchronous step
//! execution. Workflow definitions and step resolution are pure and live in
//! `crate::workflow`; this drives the resolved actions through the action registry
//! (`action_status` / `dispatch_action`).

use super::*;

use crate::workflow;

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

    /// Run the workflow at `index`: resolve its steps to actions (reporting an
    /// unknown step, an unsupported async step, or a cycle), then run each in
    /// order, stopping at the first step whose action can't run — the steps before
    /// it have already taken effect.
    ///
    /// "Can't run" here is the up-front availability check ([`action_status`]); a
    /// step that starts but then fails at runtime (e.g. an export I/O error)
    /// surfaces its own message but doesn't halt the run. Threading real step
    /// results through the runner comes with the async-step work (#227).
    pub(crate) fn run_workflow(&mut self, index: usize) {
        let Some(name) = self.workflows.get(index).map(|w| w.name.clone()) else {
            return;
        };
        let actions = match workflow::resolve(&self.workflows, &name) {
            Ok(a) => a,
            Err(e) => {
                self.message = Some(format!("Workflow '{name}': {e}"));
                return;
            }
        };
        for (i, &action) in actions.iter().enumerate() {
            if let Some(reason) = self.action_status(action) {
                self.message = Some(format!(
                    "Workflow '{name}' stopped at step {} ({}): {reason}",
                    i + 1,
                    action.name()
                ));
                return;
            }
            self.dispatch_action(action, false);
        }
        let n = actions.len();
        let plural = if n == 1 { "" } else { "s" };
        self.message = Some(format!("Ran workflow '{name}' — {n} step{plural}"));
    }
}
