//! Workflow generation from a plain-language description (#232): the AI proposes
//! a sequence of registered actions, validated against the registry before the
//! author is asked to confirm and save it.

use super::super::*;

use crate::ai::workflow_gen::{self, GeneratedWorkflow};

impl App {
    /// Begin building a workflow from a description (#232): only when AI is ready,
    /// then open the description prompt. The estimate is trivial (no manuscript is
    /// sent), so there's no cost confirm.
    pub(crate) fn start_describe_workflow(&mut self) {
        match self.ai.check_ready() {
            Ok(_) => self.open_prompt(Pending::DescribeWorkflow, "Describe a workflow: ", "", None),
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    /// Generate a workflow from a plain-language description (#232): send the action
    /// vocabulary plus the description to the model (no manuscript — cheap) and
    /// propose the result via [`poll_ai`](App::poll_ai). The schema constrains the
    /// steps to valid tokens, so the model can't invent an action.
    pub(crate) fn generate_workflow(&mut self, description: &str) {
        let description = description.trim().to_string();
        if description.is_empty() {
            return;
        }
        let vocab = self.workflow_vocab();
        let model = self.ai.model.clone();
        self.spawn_outcome("⟳ Building a workflow…", move |backend| {
            let req = workflow_gen::build_request(&description, &vocab, &model);
            Outcome::Workflow(
                backend
                    .complete(&req)
                    .and_then(|r| workflow_gen::parse(&r.text)),
            )
        });
    }

    /// Handle a generated-workflow reply (#232): report a request nothing mapped
    /// to, validate the proposal against the registry, then stash the steps and
    /// open the name-confirm prompt (prefilled with the proposed name) so the
    /// author confirms, renames, or cancels before anything is written.
    pub(crate) fn apply_generated_workflow(
        &mut self,
        result: Result<GeneratedWorkflow, ai::AiError>,
    ) {
        let Some(gen) = self.ok_or_report(result) else {
            return;
        };
        if gen.steps.is_empty() {
            self.message = Some(if gen.unmapped.is_empty() {
                "Couldn't build a workflow from that".into()
            } else {
                format!("Couldn't map: {}", gen.unmapped.join(", "))
            });
            return;
        }
        // Defense beyond the schema enum: reject an unknown/interactive step or a
        // cycle before offering to save (resolve treats the proposal as a new
        // workflow alongside the existing ones).
        let mut candidate = self.workflows.clone();
        candidate.push(crate::workflow::Workflow {
            name: gen.name.clone(),
            steps: gen.steps.clone(),
        });
        if let Err(e) = crate::workflow::resolve(&candidate, &gen.name) {
            self.message = Some(format!("Proposed workflow is invalid: {e}"));
            return;
        }
        let note = if gen.unmapped.is_empty() {
            String::new()
        } else {
            format!("(couldn't map: {}) ", gen.unmapped.join(", "))
        };
        let label = format!("{note}Save workflow [{}] as: ", gen.steps.join(" → "));
        self.pending_workflow = Some(gen.steps);
        self.open_prompt(Pending::NameNewWorkflow, label, &gen.name, None);
    }
}
