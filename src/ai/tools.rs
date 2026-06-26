//! The assistant's tool catalog (#306): Quill's action registry (#225) exposed as
//! Claude tool definitions, so the model can call an action by name. Derived from
//! [`Action::ALL`] — the single source — so a new action becomes a tool with no
//! second list to maintain.
//!
//! Each action becomes one tool: its stable [`name`](crate::mode::Action::name)
//! (e.g. `report:progress`) and its human [`label`](crate::mode::Action::label) as
//! the description. The actions take no free-text arguments today, so the input
//! schema is the empty object; per-action inputs (a question for Ask, a note for
//! Annotate) are a documented follow-up (#390).
//!
//! Foundation for the assistant epic (#305): [`action_tools`] derives the full
//! catalog (consumed off-thread by the tool-use loop's tests); the pane (#307)
//! offers the author-facing [`assistant_tools`] — the actions it can actually run
//! to completion.

use serde_json::{json, Value};

use crate::mode::Action;

/// The Claude `tools` array for one tool per registry action — the full catalog,
/// including actions only some surfaces can run. No non-test caller today.
#[allow(dead_code)]
pub fn action_tools() -> Vec<Value> {
    Action::ALL.iter().map(|a| tool_for(*a)).collect()
}

/// The tools offered to the authorial assistant pane (#307): the registry
/// actions it can run autonomously to completion (see [`Action::assistant_runnable`]).
/// Still registry-derived — a new runnable action becomes a tool with no second
/// list to maintain.
pub fn assistant_tools() -> Vec<Value> {
    Action::ALL.iter().filter(|a| a.assistant_runnable()).map(|a| tool_for(*a)).collect()
}

/// One action's tool definition (Messages API shape).
fn tool_for(action: Action) -> Value {
    json!({
        "name": action.name(),
        "description": action.label(),
        // No free-text arguments yet — the loop runs the named action as-is (#390).
        "input_schema": { "type": "object", "properties": {} }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_becomes_one_tool_sourced_from_the_registry() {
        let tools = action_tools();
        assert_eq!(tools.len(), Action::ALL.len(), "one tool per registry action, no second list");
        // Each tool's name is a real, resolvable action name.
        for t in &tools {
            let name = t["name"].as_str().unwrap();
            assert!(Action::from_name(name).is_some(), "tool '{name}' maps back to an action");
            assert!(!t["description"].as_str().unwrap().is_empty());
            assert_eq!(t["input_schema"]["type"], "object");
        }
    }

    #[test]
    fn a_known_action_is_present() {
        let tools = action_tools();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"report:progress"));
        assert!(names.contains(&"save"));
    }
}
