//! Tests for the offline retrieval loop (#856), split by area into submodules:
//! [`answer_loop`] (the loop end to end), [`coverage`] (the coverage metadata and
//! its disclosure), [`search`] (the lexical search and its excerpts), and
//! [`tools`] (the tool definitions and every executor arm). All are hermetic
//! (project rule #531) — a scripted mock stands in for the model, so no test
//! reaches the network or the keychain.
//!
//! The corpus fixtures and that mock live here; each submodule pulls them in with
//! `use super::*`.

use super::*;
use crate::ai::embed::Chapter;
use crate::ai::tool_loop::{Converse, ToolCall, Turn};
use crate::ai::AiError;
use serde_json::{json, Value};
use std::cell::RefCell;

mod answer_loop;
mod coverage;
mod search;
mod tools;

fn chapter(title: &str, body: &str) -> Chapter {
    Chapter {
        title: title.to_string(),
        file: format!("{title}.md"),
        body: body.to_string(),
    }
}

/// A big corpus that would blow the on-device window if sent whole — the
/// oversized-offline case the loop exists for.
fn oversized_corpus() -> Vec<Chapter> {
    (1..=40)
        .map(|i| {
            let filler = "Mara walked the quiet road. ".repeat(200);
            let body = if i == 7 {
                format!("{filler}\nMara pressed the silver locket to her chest.\n{filler}")
            } else if i == 12 {
                format!("{filler}\nShe finally spoke of her father, long gone.\n{filler}")
            } else {
                filler
            };
            chapter(&format!("Chapter {i}"), &body)
        })
        .collect()
}

fn bible_with_sections() -> BibleSeed {
    BibleSeed::new(vec![
        (
            "characters".to_string(),
            "Mara — protagonist; carries a silver locket introduced in Chapter 7.".to_string(),
        ),
        (
            "timeline".to_string(),
            "Ch.7: the locket. Ch.12: Mara speaks of her father.".to_string(),
        ),
    ])
}

/// A mock [`Converse`] that replays a canned script of turns, so a test drives the
/// loop with no live model, no network, no keychain (project rule #531).
struct MockConverse {
    script: RefCell<Vec<Turn>>,
    /// Every request body the loop sent, for assertions on seed / compaction.
    bodies: RefCell<Vec<Value>>,
}

impl MockConverse {
    fn new(script: Vec<Turn>) -> Self {
        Self {
            script: RefCell::new(script.into_iter().rev().collect()),
            bodies: RefCell::new(Vec::new()),
        }
    }
}

impl Converse for MockConverse {
    fn respond(&self, body: &Value) -> Result<Turn, AiError> {
        self.bodies.borrow_mut().push(body.clone());
        self.script
            .borrow_mut()
            .pop()
            .ok_or_else(|| AiError::ApiMalformed("mock script exhausted".into()))
    }
}

fn call(name: &str, argument: &str) -> Turn {
    Turn::ToolCalls(vec![ToolCall {
        id: format!("t-{name}"),
        name: name.to_string(),
        input: json!({ "argument": argument }),
    }])
}
