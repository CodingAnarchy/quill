//! Author workflows (#226, #228): named, ordered sequences of registry actions
//! defined in `config.toml`, run as a single step. This module owns the
//! `[workflows]` / `[triggers]` config shape and the pure resolution of a
//! workflow's steps — flattening any nested workflow references, rejecting unknown
//! or interactive steps, and refusing cycles. The App layer (`app::workflows`)
//! executes the resolved action list and wires the triggers.

use std::collections::HashMap;

use serde::Deserialize;

use crate::config::config_path;
use crate::mode::{Action, AiAction};

/// One workflow as written in config: a name and its ordered step tokens. Each
/// token is either a registry action name (e.g. `export:docx`) or the name of
/// another workflow, expanded in place when the workflow runs.
#[derive(Clone)]
pub struct Workflow {
    pub name: String,
    pub steps: Vec<String>,
}

/// How a workflow fires beyond the run-workflow menu (#228): automatically after
/// a save, and/or from a bound Vim Normal-mode key. Validated against the defined
/// workflows at load — entries naming an unknown workflow (or a bad key) are
/// dropped and reported, never silently kept.
#[derive(Default)]
pub struct Triggers {
    /// The workflow run after each manual save, if any.
    pub on_save: Option<String>,
    /// Vim Normal-mode key → workflow name.
    pub keys: HashMap<char, String>,
}

/// The result of loading the workflow config: the workflows, the validated
/// triggers, and any config errors to surface to the author (#208-style).
pub struct Loaded {
    pub workflows: Vec<Workflow>,
    pub triggers: Triggers,
    pub errors: Vec<String>,
}

/// Load and validate the `[workflows]` and `[triggers]` sections from the home
/// config file. Workflows are sorted by name for a stable menu order; triggers
/// are checked against them. Best-effort: a missing, unreadable, or invalid file
/// yields nothing (the feature is simply absent), like the other config sections.
pub fn load() -> Loaded {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct TriggersRaw {
        on_save: Option<String>,
        keys: HashMap<String, String>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct File {
        workflows: HashMap<String, Vec<String>>,
        triggers: TriggersRaw,
    }
    let file = config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| toml::from_str::<File>(&t).ok())
        .unwrap_or_default();

    let mut workflows: Vec<Workflow> = file
        .workflows
        .into_iter()
        .map(|(name, steps)| Workflow { name, steps })
        .collect();
    workflows.sort_by(|a, b| a.name.cmp(&b.name));

    let (triggers, errors) = validate_triggers(&workflows, file.triggers.on_save, file.triggers.keys);
    Loaded { workflows, triggers, errors }
}

/// Drop trigger entries that name an unknown workflow or aren't a single key,
/// returning the kept triggers plus a reason for each dropped one. Separate from
/// parsing so the rule is testable without a config file on disk.
fn validate_triggers(
    workflows: &[Workflow],
    on_save: Option<String>,
    keys: HashMap<String, String>,
) -> (Triggers, Vec<String>) {
    let known = |name: &str| workflows.iter().any(|w| w.name == name);
    let mut errors = Vec::new();
    let mut triggers = Triggers::default();

    if let Some(name) = on_save {
        if known(&name) {
            triggers.on_save = Some(name);
        } else {
            errors.push(format!("on_save names unknown workflow '{name}'"));
        }
    }
    for (key, name) in keys {
        let mut chars = key.chars();
        match (chars.next(), chars.next()) {
            (Some(k), None) if known(&name) => {
                triggers.keys.insert(k, name);
            }
            (Some(_), None) => errors.push(format!("key '{key}' names unknown workflow '{name}'")),
            _ => errors.push(format!("trigger key '{key}' must be a single key")),
        }
    }
    (triggers, errors)
}

/// Resolve a workflow to the flat, ordered list of actions it runs, expanding any
/// nested workflow steps. Returns an author-facing reason if a step names neither
/// an action nor a workflow, names the interactive "ask" action (which needs a
/// typed question), or forms a cycle (a workflow invoking itself, directly or
/// transitively). The async AI passes are allowed — the runner runs them
/// off-thread (#227).
pub fn resolve(workflows: &[Workflow], name: &str) -> Result<Vec<Action>, String> {
    let Some(wf) = workflows.iter().find(|w| w.name == name) else {
        return Err(format!("unknown workflow '{name}'"));
    };
    let mut out = Vec::new();
    let mut stack: Vec<&str> = Vec::new();
    expand(workflows, wf, &mut stack, &mut out)?;
    Ok(out)
}

fn expand<'a>(
    workflows: &'a [Workflow],
    wf: &'a Workflow,
    stack: &mut Vec<&'a str>,
    out: &mut Vec<Action>,
) -> Result<(), String> {
    if stack.contains(&wf.name.as_str()) {
        return Err(format!("workflow '{}' can't run itself (cycle)", wf.name));
    }
    stack.push(&wf.name);
    for step in &wf.steps {
        // A step naming another workflow expands it (under the cycle guard);
        // otherwise it must be a registry action this kind of workflow can run.
        if let Some(sub) = workflows.iter().find(|w| w.name == *step) {
            expand(workflows, sub, stack, out)?;
        } else if let Some(action) = Action::from_name(step) {
            // Interactive actions can't run unattended in a workflow: "ask" needs a
            // typed question, and the portrait pass (#238) confirms before it spends
            // on image generation. Every other registry action can (the async AI
            // passes run off-thread — #227).
            match action {
                Action::Ai(AiAction::Ask) => {
                    return Err(format!("step '{step}' needs a typed question — not usable in a workflow"))
                }
                Action::Ai(AiAction::Brainstorm | AiAction::DraftCharacters) => {
                    return Err(format!("step '{step}' needs a typed premise — not usable in a workflow"))
                }
                Action::Ai(AiAction::Portrait | AiAction::Scene) => {
                    return Err(format!("step '{step}' is interactive — not usable in a workflow"))
                }
                _ => {}
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

    fn keymap(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    #[test]
    fn valid_triggers_are_kept() {
        let wfs = vec![wf("save-all", &["save"])];
        let (t, errs) =
            validate_triggers(&wfs, Some("save-all".into()), keymap(&[("R", "save-all")]));
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(t.on_save.as_deref(), Some("save-all"));
        assert_eq!(t.keys.get(&'R').map(String::as_str), Some("save-all"));
    }

    #[test]
    fn on_save_naming_an_unknown_workflow_is_dropped_and_reported() {
        let wfs = vec![wf("save-all", &["save"])];
        let (t, errs) = validate_triggers(&wfs, Some("nope".into()), HashMap::new());
        assert!(t.on_save.is_none(), "the bad on_save is dropped");
        assert!(errs.iter().any(|e| e.contains("nope")), "{errs:?}");
    }

    #[test]
    fn a_multi_key_trigger_is_rejected() {
        let wfs = vec![wf("save-all", &["save"])];
        let (t, errs) = validate_triggers(&wfs, None, keymap(&[("gg", "save-all")]));
        assert!(t.keys.is_empty());
        assert!(errs.iter().any(|e| e.contains("single key")), "{errs:?}");
    }

    #[test]
    fn a_key_naming_an_unknown_workflow_is_rejected() {
        let wfs = vec![wf("save-all", &["save"])];
        let (t, errs) = validate_triggers(&wfs, None, keymap(&[("R", "ghost")]));
        assert!(t.keys.is_empty());
        assert!(errs.iter().any(|e| e.contains("ghost")), "{errs:?}");
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
    fn an_async_ai_step_resolves() {
        let wfs = vec![wf("deep", &["save", "ai:characters"])];
        let names: Vec<_> = resolve(&wfs, "deep").unwrap().iter().map(|a| a.name()).collect();
        assert_eq!(names, ["save", "ai:characters"]);
    }

    #[test]
    fn the_interactive_ask_step_is_rejected() {
        let wfs = vec![wf("q", &["ai:ask"])];
        let err = resolve(&wfs, "q").unwrap_err();
        assert!(err.contains("ai:ask") && err.contains("question"), "{err}");
    }

    #[test]
    fn the_interactive_portrait_step_is_rejected() {
        let wfs = vec![wf("art", &["ai:portrait"])];
        let err = resolve(&wfs, "art").unwrap_err();
        assert!(err.contains("ai:portrait") && err.contains("interactive"), "{err}");
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
