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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};

use super::task::Poll;
use super::worker;
use super::App;
use crate::ai::model_select::{self, MODELS};
use crate::config;
use crate::mode::{Mode, Pending};

/// Live progress of a model download, shared between the worker thread doing the
/// `hf-hub` fetch and the UI thread that renders it (#813). `hf-hub`'s own
/// progress bar is disabled (#727) because it writes raw bytes to stderr over
/// Quill's alternate screen; instead the fetch drives this shared counter through
/// the [`hf_hub::api::Progress`] callback, and the poll tick reads it to render
/// progress in Quill's own status line. Feature-agnostic (plain atomics) so `App`
/// can hold it without a `local` cfg; only the `local`-gated worker writes it.
#[derive(Debug)]
pub struct DownloadProgress {
    /// Total bytes to fetch across all files in this download, once known. `0`
    /// until the first file's size arrives (`init`), so the UI shows a byte count
    /// with no percentage during that brief window.
    total: AtomicU64,
    /// Bytes fetched so far across the whole download (summed over files).
    done: AtomicU64,
}

impl DownloadProgress {
    /// A fresh, empty progress counter (nothing fetched, total unknown).
    pub const fn new() -> Self {
        Self {
            total: AtomicU64::new(0),
            done: AtomicU64::new(0),
        }
    }

    /// Add `bytes` to this download's total size — called as each file's size
    /// becomes known (`hf-hub`'s `init`), so a multi-file download accumulates one
    /// combined total rather than resetting per file. Written only by the
    /// `local`-gated worker's progress sink (and the plumbing tests).
    #[cfg_attr(not(any(feature = "local", test)), allow(dead_code))]
    pub fn add_total(&self, bytes: u64) {
        self.total.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record `bytes` more fetched (`hf-hub`'s `update` reports each read's size).
    /// Written only by the `local`-gated worker's progress sink (and the tests).
    #[cfg_attr(not(any(feature = "local", test)), allow(dead_code))]
    pub fn add_done(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::Relaxed);
    }

    /// A snapshot `(done, total)` of the byte counts, for rendering.
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}

/// The label for a download in flight: how far along the fetch is, in a form the
/// status line can show every tick (#813). Given `done`/`total` bytes and how
/// long the fetch has run, produces `"3.1/5.2 GB (60%, ~2m left)"` — a percentage
/// once the total is known, plus a rough ETA once enough has downloaded to
/// estimate a rate. Before any size arrives it's just the bytes fetched. Pure, so
/// the plumbing test can assert on the exact string.
fn download_progress_label(done: u64, total: u64, elapsed_secs: f64) -> String {
    if total == 0 {
        // Size not yet known (the first `init` hasn't landed) — show raw bytes.
        return format!("{:.1} GB downloaded", gb(done));
    }
    let pct = (done as f64 / total as f64 * 100.0).round() as u64;
    let eta = eta_suffix(done, total, elapsed_secs);
    // Unit stated once — "2.0/4.0 GB" reads cleaner than repeating "GB" on each.
    format!("{:.1}/{:.1} GB ({pct}%{eta})", gb(done), gb(total))
}

/// A byte count as a `f64` number of GiB (GiB base, matching how the catalog
/// quotes `size_gb`); the caller adds the one-decimal formatting and unit.
fn gb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// The `", ~2m left"` ETA suffix for a download, or `""` when there isn't enough
/// signal to estimate one (nothing fetched yet, already complete, or elapsed is
/// zero). Extrapolates the remaining bytes at the average rate so far.
fn eta_suffix(done: u64, total: u64, elapsed_secs: f64) -> String {
    if done == 0 || done >= total || elapsed_secs <= 0.0 {
        return String::new();
    }
    let rate = done as f64 / elapsed_secs; // bytes/sec so far
    if rate <= 0.0 {
        return String::new();
    }
    let remaining_secs = (total - done) as f64 / rate;
    format!(", ~{} left", human_duration(remaining_secs))
}

/// A short human duration for an ETA: `"2m"`, `"45s"`, `"1h 3m"`. Coarse on
/// purpose — an ETA that jitters to the second reads as noise.
fn human_duration(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

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
        let id = MODELS
            .get(selected)
            .map(super::super::ai::model_select::Model::id);
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
    // Mirrors the `local` variant's `&self` signature so callers are feature-agnostic;
    // `self` is only read under `cfg(test)`, so a non-local non-test build sees it unused.
    #[cfg(not(feature = "local"))]
    #[allow(clippy::unused_self)]
    pub(crate) const fn local_model_present(&self, _input_tokens: u32) -> bool {
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
            self.message = Some(format!("Qwen3-{id} is already downloaded."));
            return;
        }
        if self.download_task.is_busy() {
            self.message = Some("A model download is already running.".into());
            return;
        }
        let id = id.to_string();
        // Share a live progress counter with the worker: the fetch feeds it through
        // hf-hub's Progress callback (raw library output stays suppressed, #727) and
        // the poll tick reads it to render progress in Quill's own UI (#813). Kept
        // alongside the download start-time so the poll can extrapolate an ETA.
        let progress = Arc::new(DownloadProgress::new());
        self.download_progress = Some((progress.clone(), Instant::now()));
        self.download_task
            .spawn(move || (id, worker::download_model(model, &progress)));
        self.message = Some(format!(
            "Downloading Qwen3-{}… (~{:.1} GB)",
            model.id(),
            model.size_gb
        ));
    }

    /// Surface a finished download (#382), or its live progress (#813): while the
    /// fetch runs, render bytes / percent / ETA from the shared counter; on finish
    /// refresh on-disk status and report. Called once per event-loop tick.
    pub fn poll_model_download(&mut self) {
        match self.download_task.poll() {
            Poll::Ready((id, Ok(()))) => {
                self.download_progress = None; // the fetch is done — drop the live counter
                self.refresh_downloaded_models();
                self.message = Some(format!("Downloaded Qwen3-{id}."));
            }
            Poll::Ready((id, Err(e))) => {
                self.download_progress = None;
                self.message = Some(format!("Download of Qwen3-{id} failed: {e}"));
            }
            Poll::Pending => self.tick_download_progress(),
            Poll::Died => {
                self.download_progress = None;
                self.message = Some("Model download failed.".into());
            }
        }
    }

    /// Refresh the status line with the in-flight download's live progress (#813) —
    /// bytes fetched, percent, and a rough ETA read from the shared counter the
    /// worker's `hf-hub` fetch drives. The message updates each tick until the
    /// download's terminal poll clears it. A no-op when nothing is downloading.
    fn tick_download_progress(&mut self) {
        if let Some((progress, started)) = &self.download_progress {
            let (done, total) = progress.snapshot();
            let elapsed = started.elapsed().as_secs_f64();
            self.message = Some(format!(
                "Downloading… {}",
                download_progress_label(done, total, elapsed)
            ));
        }
    }

    /// Confirm removing a downloaded model from disk (#382).
    fn confirm_remove_model(&mut self, id: &'static str) {
        if !self.model_is_downloaded(id) {
            self.message = Some(format!("Qwen3-{id} isn't downloaded."));
            return;
        }
        self.open_prompt(
            Pending::ConfirmRemoveModel(id),
            format!("Remove Qwen3-{id} from disk? (y/N)"),
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
                self.message = Some(format!("Removed Qwen3-{id} from disk."));
            }
            Err(e) => self.message = Some(format!("Couldn't remove Qwen3-{id}: {e}")),
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
        .map(super::super::ai::model_select::Model::id)
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
        let added = toggled(&empty, "8B");
        assert_eq!(added, vec!["8B".to_string()]);
        assert!(
            toggled(&added, "8B").is_empty(),
            "toggling again removes it"
        );
    }

    #[test]
    fn local_models_toml_is_a_canonical_array() {
        // Listed out of catalog order and with an unknown id → canonical, filtered.
        let ids = vec!["8B".to_string(), "0.6B".to_string(), "bogus".to_string()];
        assert_eq!(local_models_toml(&ids), "[\"0.6B\", \"8B\"]");
        assert_eq!(local_models_toml(&[]), "[]");
    }

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn progress_label_shows_bytes_before_the_total_is_known() {
        // Until the first file size arrives (`total == 0`) there's no percentage —
        // just the bytes fetched, so the author sees the download is moving.
        assert_eq!(download_progress_label(GB / 2, 0, 3.0), "0.5 GB downloaded");
    }

    #[test]
    fn progress_label_shows_percent_and_eta_once_underway() {
        // Half of a 4 GB download in 60s → 50%, ~60s of bytes left at that rate.
        assert_eq!(
            download_progress_label(2 * GB, 4 * GB, 60.0),
            "2.0/4.0 GB (50%, ~1m left)",
        );
    }

    #[test]
    fn progress_label_omits_eta_when_it_cant_be_estimated() {
        // Nothing fetched yet: a percentage but no rate to extrapolate from.
        assert_eq!(download_progress_label(0, 4 * GB, 5.0), "0.0/4.0 GB (0%)");
        // Complete: no time "left" to report.
        assert_eq!(
            download_progress_label(4 * GB, 4 * GB, 30.0),
            "4.0/4.0 GB (100%)",
        );
    }

    #[test]
    fn human_duration_is_coarse() {
        assert_eq!(human_duration(45.0), "45s");
        assert_eq!(human_duration(150.0), "2m");
        assert_eq!(human_duration(3600.0 + 180.0), "1h 3m");
    }

    #[test]
    fn a_progress_counter_folds_multi_file_init_and_updates() {
        // A fake download source (no network) drives the shared counter exactly as
        // hf-hub's Progress callback would: two files' sizes accumulate into one
        // total, and each read adds to `done` — the manager renders this snapshot.
        let p = DownloadProgress::new();
        assert_eq!(p.snapshot(), (0, 0), "empty before any callback");
        p.add_total(3 * GB); // the GGUF
        p.add_total(GB / 4); // the tokenizer
        p.add_done(GB);
        p.add_done(GB / 2);
        let (done, total) = p.snapshot();
        assert_eq!(total, 3 * GB + GB / 4, "both files' sizes summed");
        assert_eq!(done, GB + GB / 2, "reads accumulated across files");
        // And the manager's rendered label reflects that snapshot.
        let label = download_progress_label(done, total, 30.0);
        assert!(label.starts_with("1.5/3.2 GB ("), "{label}");
        assert!(label.contains("left"), "an ETA once underway: {label}");
    }
}
