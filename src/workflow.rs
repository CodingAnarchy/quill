//! Author workflows (#226): named, ordered sequences of registry actions defined
//! in `config.toml`, run as a single step from the run-workflow menu. This module
//! owns the `[workflows]` config shape and the pure resolution of a workflow's
//! steps — flattening any nested workflow references, rejecting unknown or
//! not-yet-supported steps, and refusing cycles. The App layer (`app::workflows`)
//! executes the resolved action list.

use std::collections::HashMap;

use serde::Deserialize;

use crate::config::config_path;
use crate::mode::Action;

/// One workflow as written in config: a name and its ordered step tokens. Each
/// token is either a registry action name (e.g. `export:docx`) or the name of
/// another workflow, expanded in place when the workflow runs.
#[derive(Clone)]
pub struct Workflow {
    pub name: String,
    pub steps: Vec<String>,
}

/// Load the `[workflows]` section from the home config file, sorted by name for a
/// stable menu order. Best-effort: a missing, unreadable, or invalid file yields
/// no workflows (the feature is simply absent), like the other config sections.
pub fn load() -> Vec<Workflow> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct File {
        workflows: HashMap<String, Vec<String>>,
    }
    let map = config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| toml::from_str::<File>(&t).ok())
        .map(|f| f.workflows)
        .unwrap_or_default();
    let mut list: Vec<Workflow> = map
        .into_iter()
        .map(|(name, steps)| Workflow { name, steps })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

/// Resolve a workflow to the flat, ordered list of actions it runs, expanding any
/// nested workflow steps. Returns an author-facing reason if a step names neither
/// an action nor a workflow, names an action a workflow can't run yet (the async
/// AI passes — #227), or forms a cycle (a workflow invoking itself, directly or
/// transitively).
pub fn resolve(workflows: &[Workflow], name: &str) -> Result<Vec<Action>, String> {
    let mut out = Vec::new();
    let mut stack: Vec<&str> = Vec::new();
    expand(workflows, name, &mut stack, &mut out)?;
    Ok(out)
}

fn expand<'a>(
    workflows: &'a [Workflow],
    name: &'a str,
    stack: &mut Vec<&'a str>,
    out: &mut Vec<Action>,
) -> Result<(), String> {
    if stack.contains(&name) {
        return Err(format!("workflow '{name}' can't run itself (cycle)"));
    }
    let Some(wf) = workflows.iter().find(|w| w.name == name) else {
        return Err(format!("unknown workflow '{name}'"));
    };
    stack.push(name);
    for step in &wf.steps {
        // A step naming another workflow expands it (under the cycle guard);
        // otherwise it must be a registry action this kind of workflow can run.
        if workflows.iter().any(|w| w.name == *step) {
            expand(workflows, step, stack, out)?;
        } else if let Some(action) = Action::from_name(step) {
            if !action.is_sync() {
                return Err(format!("step '{step}' isn't supported in a workflow yet (#227)"));
            }
            out.push(action);
        } else {
            return Err(format!("unknown action '{step}'"));
        }
    }
    stack.pop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf(name: &str, steps: &[&str]) -> Workflow {
        Workflow { name: name.into(), steps: steps.iter().map(|s| (*s).to_string()).collect() }
    }

    #[test]
    fn resolves_a_sequence_of_actions_in_order() {
        let wfs = vec![wf("finish", &["save", "report:progress", "export:docx"])];
        let actions = resolve(&wfs, "finish").unwrap();
        let names: Vec<_> = actions.iter().map(|a| a.name()).collect();
        assert_eq!(names, ["save", "report:progress", "export:docx"]);
    }

    #[test]
    fn an_unknown_step_is_reported() {
        let wfs = vec![wf("oops", &["save", "frobnicate"])];
        let err = resolve(&wfs, "oops").unwrap_err();
        assert!(err.contains("frobnicate"), "{err}");
    }

    #[test]
    fn an_async_ai_step_is_rejected_until_227() {
        let wfs = vec![wf("deep", &["save", "ai:characters"])];
        let err = resolve(&wfs, "deep").unwrap_err();
        assert!(err.contains("ai:characters") && err.contains("#227"), "{err}");
    }

    #[test]
    fn local_triage_step_is_allowed() {
        let wfs = vec![wf("check", &["save", "triage"])];
        let actions = resolve(&wfs, "check").unwrap();
        assert_eq!(actions.last().unwrap().name(), "triage");
    }

    #[test]
    fn a_nested_workflow_is_expanded_in_place() {
        let wfs = vec![
            wf("a", &["save", "b"]),
            wf("b", &["report:analysis", "export:markdown"]),
        ];
        let names: Vec<_> = resolve(&wfs, "a").unwrap().iter().map(|a| a.name()).collect();
        assert_eq!(names, ["save", "report:analysis", "export:markdown"]);
    }

    #[test]
    fn a_direct_cycle_is_refused() {
        let wfs = vec![wf("loop", &["save", "loop"])];
        let err = resolve(&wfs, "loop").unwrap_err();
        assert!(err.contains("cycle"), "{err}");
    }

    #[test]
    fn a_transitive_cycle_is_refused() {
        let wfs = vec![wf("a", &["b"]), wf("b", &["c"]), wf("c", &["a"])];
        let err = resolve(&wfs, "a").unwrap_err();
        assert!(err.contains("cycle"), "{err}");
    }
}
