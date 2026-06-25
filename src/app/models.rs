//! The local AI model manager (#379): an in-editor view of the model catalog —
//! each option with its size, RAM need, context window, and a one-line
//! description — where the author curates which models to keep in their set
//! (`[ai] local_models`, #377) without hand-editing config. The router then picks
//! among the chosen set per request (#377) and routes whole-book passes to a
//! long-context one when downloaded (#378).
//!
//! Downloading, removing, and on-disk status — the `local`-gated, async parts —
//! are the follow-up in #382; a model added to the set still downloads on first
//! use, as before.

use crossterm::event::{KeyCode, KeyEvent};

use super::App;
use crate::ai::model_select::MODELS;
use crate::config;
use crate::mode::Mode;

impl App {
    /// Open the model manager over the catalog.
    pub(crate) fn open_model_manager(&mut self) {
        self.mode = Mode::ModelManager { selected: 0 };
    }

    /// Key handling for the model manager: move, toggle the selected model in/out
    /// of the author's set (Enter / Space), close (Esc).
    pub(crate) fn on_key_model_manager(&mut self, key: KeyEvent) {
        let Mode::ModelManager { selected } = self.mode else {
            return;
        };
        if let Some(s) = super::assist::list_move(selected, MODELS.len(), key.code) {
            self.mode = Mode::ModelManager { selected: s };
            return;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(model) = MODELS.get(selected) {
                    self.toggle_model_in_set(model.id());
                }
            }
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }

    /// Add the model `id` to the author's set if absent, remove it if present
    /// (#379), then persist the set to `[ai] local_models`.
    fn toggle_model_in_set(&mut self, id: &str) {
        self.ai.local_models = toggled(&self.ai.local_models, id);
        let in_set = self.ai.local_models.iter().any(|m| m == id);
        self.persist_local_models();
        self.message = Some(format!(
            "{id} {} your local models.",
            if in_set { "added to" } else { "removed from" }
        ));
    }

    /// Write the model set to `[ai] local_models` in the home config, preserving
    /// the rest of the file (the same surgical writer the config/workflow
    /// generators use).
    fn persist_local_models(&mut self) {
        let value = local_models_toml(&self.ai.local_models);
        if let Err(e) = config::set_values(&[("ai".to_string(), "local_models".to_string(), value)]) {
            self.message = Some(format!("Couldn't save the model set: {e}"));
        }
    }
}

/// Toggle `id` in a model-id list: drop it if present, append it if not.
fn toggled(ids: &[String], id: &str) -> Vec<String> {
    if ids.iter().any(|x| x == id) {
        ids.iter().filter(|x| x.as_str() != id).cloned().collect()
    } else {
        let mut next = ids.to_vec();
        next.push(id.to_string());
        next
    }
}

/// A TOML array literal for the `local_models` config line, in catalog order and
/// dropping anything not in the table — so the written set is canonical.
fn local_models_toml(ids: &[String]) -> String {
    let quoted: Vec<String> = MODELS
        .iter()
        .map(|m| m.id())
        .filter(|id| ids.iter().any(|x| x == id))
        .map(|id| format!("\"{id}\""))
        .collect();
    format!("[{}]", quoted.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggled_adds_then_removes() {
        let empty: Vec<String> = vec![];
        let added = toggled(&empty, "7B");
        assert_eq!(added, vec!["7B".to_string()]);
        assert!(toggled(&added, "7B").is_empty(), "toggling again removes it");
    }

    #[test]
    fn local_models_toml_is_a_canonical_array() {
        // Listed out of catalog order and with an unknown id → canonical, filtered.
        let ids = vec!["7B".to_string(), "0.5B".to_string(), "bogus".to_string()];
        assert_eq!(local_models_toml(&ids), "[\"0.5B\", \"7B\"]");
        assert_eq!(local_models_toml(&[]), "[]");
    }
}
