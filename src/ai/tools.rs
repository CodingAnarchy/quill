//! The assistant's tool catalog (#306): Quill's action registry (#225) exposed as
//! Claude tool definitions, so the model can call an action by name. Derived from
//! [`Action::ALL`] — the single source — so a new runnable action becomes a tool
//! with no second list to maintain.
//!
//! Each action becomes one tool: its stable [`name`](crate::mode::Action::name)
//! (e.g. `report:progress`) and its human [`label`](crate::mode::Action::label) as
//! the description, with an input schema derived from its
//! [`text_arg`](crate::mode::Action::text_arg) (#390).

use serde_json::{json, Value};

use crate::mode::Action;

/// The tools offered to the authorial assistant pane (#307): the registry
/// actions it can run autonomously to completion (see [`Action::assistant_runnable`]).
/// Still registry-derived — a new runnable action becomes a tool with no second
/// list to maintain.
pub fn assistant_tools() -> Vec<Value> {
    Action::ALL
        .iter()
        .filter(|a| a.assistant_runnable())
        .map(|a| tool_for(*a))
        .collect()
}

/// One action's tool definition (Messages API shape). The input schema is derived
/// from the action's [`text_arg`](crate::mode::Action::text_arg) (#390): a single
/// required string for the actions that take one (a question for Ask, a premise
/// for Brainstorm, …), the empty object for argument-free actions.
fn tool_for(action: Action) -> Value {
    let input_schema = action.text_arg().map_or_else(
        || json!({ "type": "object", "properties": {} }),
        |arg| {
            json!({
                "type": "object",
                "properties": { arg: { "type": "string", "description": format!("the {arg} to run this action with") } },
                "required": [arg],
            })
        },
    );
    json!({
        "name": action.name(),
        "description": action.label(),
        "input_schema": input_schema,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_a_registry_action_with_a_schema() {
        let tools = assistant_tools();
        assert!(!tools.is_empty());
        for t in &tools {
            let name = t["name"].as_str().unwrap();
            let action = Action::from_name(name).expect("tool maps back to an action");
            assert!(
                action.assistant_runnable(),
                "only runnable actions are offered: {name}"
            );
            assert!(!t["description"].as_str().unwrap().is_empty());
            assert_eq!(t["input_schema"]["type"], "object");
        }
    }
}
