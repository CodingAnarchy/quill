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

use super::task::Poll;
use super::worker;
use super::App;
use crate::ai::model_select::{self, MODELS};
use crate::config;
use crate::mode::{Mode, Pending};

impl App {
    /// Open the model manager over the catalog, refreshing on-disk status first.
    pub(crate) fn open_model_manager(&mut self) {
        self.refresh_downloaded_models();
        self.mode = Mode::ModelManager { selected: 0 };
    }

    /// Key handling for the model manager: move; toggle the selected model in/out
    /// of the author's set (Enter / Space); download it (`d`); remove it (`x`);
    /// close (Esc).
    pub(crate) fn on_key_model_manager(&mut self, key: KeyEvent) {
        let Mode::ModelManager { selected } = self.mode else {
            return;
        };
        if let Some(s) = super::assist::list_move(selected, MODELS.len(), key.code) {
            self.mode = Mode::ModelManager { selected: s };
            return;
        }
        let id = MODELS.get(selected).map(|m| m.id());
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(id) = id {
                    self.toggle_model_in_set(id);
                }
            }
            KeyCode::Char('d') => {
                if let Some(id) = id {
                    self.start_model_download(id);
                }
            }
            KeyCode::Char('x') => {
                if let Some(id) = id {
                    self.confirm_remove_model(id);
                }
            }
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }

    /// Whether model `id` is downloaded, from the cached set (#382) — what the
    /// manager rows show, without probing the cache per frame.
    pub(crate) fn model_is_downloaded(&self, id: &str) -> bool {
        self.downloaded_models.iter().any(|m| m == id)
    }

    /// Whether a local model that can hold a pass of `input_tokens` is *actually
    /// present on disk* (#725) — a no-download `hf-hub` cache probe. The router
    /// gate (`local_fits`) uses this so it never routes a pass on-device when only
    /// the code, not the weights, is present: a compiled-in-but-not-downloaded
    /// model is "local unavailable," and an AI pass never triggers an implicit
    /// multi-GB fetch (that's the deliberate model-manager download's job). The
    /// candidates are the author's configured set (`[ai] local_models`), or — when
    /// none is set — the single hardware-recommended model. Always `false` without
    /// the `local` feature (nothing can be downloaded there).
    #[cfg(feature = "local")]
    pub(crate) fn local_model_present(&self, input_tokens: u32) -> bool {
        #[cfg(test)]
        if let Some(present) = self.test_local_present {
            return present;
        }
        let configured = model_select::available(&self.ai.local_models);
        let candidates = if configured.is_empty() {
            worker::recommended_model().into_iter().collect()
        } else {
            configured
        };
        candidates
            .iter()
            .any(|m| m.context_tokens >= input_tokens && worker::model_downloaded(m))
    }
    #[cfg(not(feature = "local"))]
    pub(crate) fn local_model_present(&self, _input_tokens: u32) -> bool {
        #[cfg(test)]
        if let Some(present) = self.test_local_present {
            return present;
        }
        false
    }

    /// Re-probe which catalog models are on disk (#382) and cache the result.
    /// Cheap I/O, run on open and after a download / remove — never per frame.
    fn refresh_downloaded_models(&mut self) {
        self.downloaded_models = MODELS
            .iter()
            .filter(|m| worker::model_downloaded(m))
            .map(|m| m.id().to_string())
            .collect();
    }

    /// Start downloading model `id` on a worker thread (#382): the editor stays
    /// responsive; [`poll_model_download`](Self::poll_model_download) surfaces the
    /// result. Refuses without the `local` feature, or when already downloaded /
    /// a download is in flight.
    fn start_model_download(&mut self, id: &str) {
        let Some(model) = model_select::by_id(id) else {
            return;
        };
        if cfg!(not(feature = "local")) {
            self.message = Some(worker::LOCAL_UNAVAILABLE.to_string());
            return;
        }
        if self.model_is_downloaded(id) {
            self.message = Some(format!("Qwen2.5-{id} is already downloaded."));
            return;
        }
        if self.download_task.is_busy() {
            self.message = Some("A model download is already running.".into());
            return;
        }
        let id = id.to_string();
        self.download_task
            .spawn(move || (id, worker::download_model(model)));
        self.message = Some(format!(
            "Downloading Qwen2.5-{}… (~{:.1} GB)",
            model.id(),
            model.size_gb
        ));
    }

    /// Surface a finished download (#382): refresh on-disk status and report.
    /// Called once per event-loop tick.
    pub fn poll_model_download(&mut self) {
        match self.download_task.poll() {
            Poll::Ready((id, Ok(()))) => {
                self.refresh_downloaded_models();
                self.message = Some(format!("Downloaded Qwen2.5-{id}."));
            }
            Poll::Ready((id, Err(e))) => {
                self.message = Some(format!("Download of Qwen2.5-{id} failed: {e}"))
            }
            Poll::Pending => {}
            Poll::Died => self.message = Some("Model download failed.".into()),
        }
    }

    /// Confirm removing a downloaded model from disk (#382).
    fn confirm_remove_model(&mut self, id: &'static str) {
        if !self.model_is_downloaded(id) {
            self.message = Some(format!("Qwen2.5-{id} isn't downloaded."));
            return;
        }
        self.open_prompt(
            Pending::ConfirmRemoveModel(id),
            format!("Remove Qwen2.5-{id} from disk? (y/N)"),
            "",
            None,
        );
    }

    /// Delete a downloaded model's files (#382), drop any cached loaded instance,
    /// and refresh status. Confirmed via [`Pending::ConfirmRemoveModel`].
    pub(crate) fn remove_model(&mut self, id: &str) {
        let Some(model) = model_select::by_id(id) else {
            return;
        };
        match worker::remove_model_files(model) {
            Ok(()) => {
                if let Ok(mut cache) = self.local_backend.lock() {
                    cache.remove(id); // forget the loaded model if it was cached
                }
                self.refresh_downloaded_models();
                self.message = Some(format!("Removed Qwen2.5-{id} from disk."));
            }
            Err(e) => self.message = Some(format!("Couldn't remove Qwen2.5-{id}: {e}")),
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
        if let Err(e) = config::set_values(&[("ai".to_string(), "local_models".to_string(), value)])
        {
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
        assert!(
            toggled(&added, "7B").is_empty(),
            "toggling again removes it"
        );
    }

    #[test]
    fn local_models_toml_is_a_canonical_array() {
        // Listed out of catalog order and with an unknown id → canonical, filtered.
        let ids = vec!["7B".to_string(), "0.5B".to_string(), "bogus".to_string()];
        assert_eq!(local_models_toml(&ids), "[\"0.5B\", \"7B\"]");
        assert_eq!(local_models_toml(&[]), "[]");
    }
}
