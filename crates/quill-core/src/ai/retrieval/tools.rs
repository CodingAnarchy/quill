//! The read-only tool executor (#856): the [`Tools`] struct that resolves a
//! [`ToolCall`] to its result text and cited chapters — the closed allowlist of
//! lookups that is the code-layer guarantee the loop can only read, never write —
//! and [`run_calls`], which drives one batch and records the retained excerpts.

use std::fmt::Write as _;

use serde_json::Value;

use crate::ai::tool_loop::{self, tool_result_message, tool_use_message, ToolCall};

use super::search::ManuscriptSearch;
use super::seed::{BibleSeed, Excerpt};
use super::{LIST_CHAPTERS, READ_BIBLE, READ_CHAPTER, SEARCH_MANUSCRIPT};

/// How many search locations one `search:manuscript` call returns.
pub(super) const SEARCH_LIMIT: usize = 6;

/// A read-only tool executor over the corpus (#856): resolves a [`ToolCall`] to its
/// result text and the chapters it cited. The closed set of lookups — the code-layer
/// guarantee that the loop can only read, never write (the boundary, enforced).
pub(super) struct Tools<'a, S: ManuscriptSearch> {
    pub(super) search: &'a S,
    pub(super) bible: &'a BibleSeed,
}

impl<S: ManuscriptSearch> Tools<'_, S> {
    /// Run one tool call, returning `(result text, cited chapters)`. An unknown tool
    /// or a missing target returns a plain note (never an error that kills the loop).
    fn run(&self, name: &str, argument: &str) -> (String, Vec<String>) {
        match name {
            READ_CHAPTER => self
                .search
                .chapters()
                .iter()
                .find(|c| c.title.eq_ignore_ascii_case(argument.trim()))
                .map_or_else(
                    || (format!("No chapter titled \"{argument}\"."), Vec::new()),
                    |ch| (ch.body.clone(), vec![ch.title.clone()]),
                ),
            SEARCH_MANUSCRIPT => {
                let hits = self.search.search(argument, SEARCH_LIMIT);
                if hits.is_empty() {
                    return (format!("No matches for \"{argument}\"."), Vec::new());
                }
                let mut out = String::new();
                let mut cites = Vec::new();
                for h in &hits {
                    let _ = writeln!(out, "[{}] {}", h.chapter, h.excerpt);
                    cites.push(h.chapter.clone());
                }
                (out, cites)
            }
            READ_BIBLE => self.bible.section(argument).map_or_else(
                || {
                    (
                        format!("No bible section \"{argument}\". {}", self.bible.overview()),
                        Vec::new(),
                    )
                },
                |text| (text.to_string(), Vec::new()),
            ),
            LIST_CHAPTERS => {
                let list = self
                    .search
                    .chapters()
                    .iter()
                    .map(|c| format!("- {}", c.title))
                    .collect::<Vec<_>>()
                    .join("\n");
                (list, Vec::new())
            }
            other => (format!("Unknown tool \"{other}\"."), Vec::new()),
        }
    }
}

/// Run a batch of tool calls, appending the model's `tool_use` turn and the results
/// (both the wire `tool_result`s and the retained [`Excerpt`]s) to the running state.
pub(super) fn run_calls<S: ManuscriptSearch>(
    tools: &Tools<'_, S>,
    calls: &[ToolCall],
    messages: &mut Vec<Value>,
    excerpts: &mut Vec<Excerpt>,
) {
    messages.push(tool_use_message(calls));
    let mut results = Vec::new();
    for call in calls {
        let argument = call.input["argument"].as_str().unwrap_or("").to_string();
        let (body, citations) = tools.run(&call.name, &argument);
        results.push(tool_loop::tool_result_block(&call.id, &body));
        excerpts.push(Excerpt {
            tool: call.name.clone(),
            argument,
            body,
            citations,
        });
    }
    messages.push(tool_result_message(results));
}
