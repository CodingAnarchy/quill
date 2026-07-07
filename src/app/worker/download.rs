//! Local-model download / status / removal against the `hf-hub` cache (#382),
//! plus the `hf-hub` progress sink that feeds Quill's shared download counter
//! (#813). Split out of `worker.rs` (#839); `#[cfg(feature = "local")]` gating
//! kept intact so the default build carries only the erroring stubs.

use super::super::*;

/// Shown when the local backend is selected but wasn't compiled in. One source
/// so the fast-fail (UI thread) and the `build_local` stub can't drift.
pub(in crate::app) const LOCAL_UNAVAILABLE: &str =
    "local backend isn't in this build — rebuild Quill with `--features local`, or set `backend = \"claude\"`";

/// Whether `model`'s GGUF is already in the `hf-hub` cache (#382) — a no-download
/// lookup, so the manager can show on-disk status. Always `false` without the
/// `local` feature (nothing can be downloaded there).
#[cfg(feature = "local")]
pub(in crate::app) fn model_downloaded(model: &ai::model_select::Model) -> bool {
    hf_hub::Cache::from_env()
        .model(model.gguf_repo.to_string())
        .get(model.gguf_file)
        .is_some()
}
#[cfg(not(feature = "local"))]
pub(in crate::app) const fn model_downloaded(_model: &ai::model_select::Model) -> bool {
    false
}

/// The single hardware-recommended model this machine would load when no set is
/// configured (#725) — resolved without downloading, so the router's "is a usable
/// local model on disk?" gate can consider it. `None` when the machine is too
/// small for any model.
#[cfg(feature = "local")]
pub(in crate::app) fn recommended_model() -> Option<&'static ai::model_select::Model> {
    ai::local::LocalBackend::recommended_model()
}

/// Whether the embedding `model`'s weights are already in the `hf-hub` cache
/// (#831) — a no-download lookup, so grounded-Ask can decide whether to use the
/// semantic retrieval index or fall back to sending the whole book. Always `false`
/// without the `local` feature (nothing on-device can embed there).
#[cfg(feature = "local")]
pub(in crate::app) fn embed_model_downloaded(model: &ai::model_select::EmbedModel) -> bool {
    hf_hub::Cache::from_env()
        .model(model.repo.to_string())
        .get(model.weights_file)
        .is_some()
}
#[cfg(not(feature = "local"))]
pub(in crate::app) const fn embed_model_downloaded(_model: &ai::model_select::EmbedModel) -> bool {
    false
}

/// A `hf-hub` progress sink that feeds Quill's shared [`DownloadProgress`] counter
/// (#813) instead of the disabled indicatif bar (#727). `hf-hub` calls `init` with
/// each file's byte size and `update` with every chunk read; we fold both into the
/// combined counter the UI thread renders. `finish` is a no-op — completion is
/// reported by the download task's terminal poll, not the per-file callback.
#[cfg(feature = "local")]
struct ProgressSink(std::sync::Arc<super::super::models::DownloadProgress>);

#[cfg(feature = "local")]
impl hf_hub::api::Progress for ProgressSink {
    fn init(&mut self, size: usize, _filename: &str) {
        self.0.add_total(size as u64);
    }
    fn update(&mut self, size: usize) {
        self.0.add_done(size as u64);
    }
    fn finish(&mut self) {}
}

/// Fetch `model`'s GGUF + tokenizer into the `hf-hub` cache (#382), without
/// loading it — the explicit "download now" the manager offers (resumable, cached
/// outside the project). Run on a worker thread. `progress` is the shared counter
/// the fetch drives so Quill can render live progress (#813).
#[cfg(feature = "local")]
pub(in crate::app) fn download_model(
    model: &ai::model_select::Model,
    progress: &std::sync::Arc<super::super::models::DownloadProgress>,
) -> Result<(), String> {
    // Disable hf-hub's indicatif progress bar (#727): Quill owns the alternate
    // screen, so the library's raw stderr writes would corrupt the TUI. Progress
    // is surfaced through Quill's own UI instead, via the shared counter (#813).
    let hub = hf_hub::api::sync::ApiBuilder::new()
        .with_progress(false)
        .build()
        .map_err(|e| format!("hf-hub: {e}"))?;
    fetch_with_progress(
        model.gguf_repo,
        model.gguf_file,
        &hub.model(model.gguf_repo.to_string()),
        progress,
    )
    .map_err(|e| format!("download {}: {e}", model.gguf_file))?;
    fetch_with_progress(
        model.tokenizer_repo,
        "tokenizer.json",
        &hub.model(model.tokenizer_repo.to_string()),
        progress,
    )
    .map_err(|e| format!("download tokenizer: {e}"))?;
    Ok(())
}

/// Fetch `filename` from the `repo`-named repo, reusing the cached copy if present
/// (like `ApiRepo::get`) and otherwise downloading it while driving `progress`
/// (#813). `ApiRepo::download_with_progress` always re-downloads and `ApiRepo::get`
/// gives no progress, so the cache is probed directly here: a cached file
/// short-circuits the fetch (preserving the manager's "already downloaded"
/// resumability) and still adds its size to `done` + `total`, so a partly-cached
/// multi-file download reads an honest 100%.
#[cfg(feature = "local")]
fn fetch_with_progress(
    repo_id: &str,
    filename: &str,
    api_repo: &hf_hub::api::sync::ApiRepo,
    progress: &std::sync::Arc<super::super::models::DownloadProgress>,
) -> Result<std::path::PathBuf, hf_hub::api::sync::ApiError> {
    if let Some(path) = hf_hub::Cache::from_env()
        .model(repo_id.to_string())
        .get(filename)
    {
        // Already cached: count its bytes so the combined percentage stays honest.
        if let Ok(len) = std::fs::metadata(&path).map(|m| m.len()) {
            progress.add_total(len);
            progress.add_done(len);
        }
        return Ok(path);
    }
    api_repo.download_with_progress(filename, ProgressSink(progress.clone()))
}

#[cfg(not(feature = "local"))]
pub(in crate::app) fn download_model(
    _model: &ai::model_select::Model,
    _progress: &std::sync::Arc<super::super::models::DownloadProgress>,
) -> Result<(), String> {
    Err(LOCAL_UNAVAILABLE.to_string())
}

/// Delete `model`'s GGUF from the `hf-hub` cache to reclaim disk (#382). Only the
/// large GGUF repo is removed; the tiny tokenizer is left (it may be shared).
#[cfg(feature = "local")]
pub(in crate::app) fn remove_model_files(model: &ai::model_select::Model) -> Result<(), String> {
    let cache = hf_hub::Cache::from_env();
    let dir = cache
        .path()
        .join(hf_hub::Repo::model(model.gguf_repo.to_string()).folder_name());
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("remove {}: {e}", dir.display()))?;
    }
    Ok(())
}
#[cfg(not(feature = "local"))]
pub(in crate::app) fn remove_model_files(_model: &ai::model_select::Model) -> Result<(), String> {
    Err(LOCAL_UNAVAILABLE.to_string())
}

#[cfg(all(test, feature = "local"))]
mod local_model_files_tests {
    //! Vets the download / status / remove round-trip (#382) against the real
    //! `hf-hub` cache. Ignored: needs network + disk. Uses the smallest model
    //! (~400 MB). Run with:
    //! `cargo test --features local download_status_remove -- --ignored --nocapture`
    use std::sync::Arc;

    use super::super::super::models::DownloadProgress;
    use crate::ai::model_select::MODELS;

    #[test]
    #[ignore = "integration: needs network / a downloaded model; not hermetic"]
    fn download_status_remove_round_trip() {
        let model = &MODELS[0]; // 0.5B — keep the download small
        super::remove_model_files(model).expect("clean start"); // ignore prior state
        assert!(!super::model_downloaded(model), "absent before download");

        // The real fetch drives the shared progress counter; a live download ends
        // with `done == total` (both files fully fetched).
        let progress = Arc::new(DownloadProgress::new());
        super::download_model(model, &progress).expect("download");
        let (done, total) = progress.snapshot();
        assert!(
            total > 0 && done == total,
            "progress reached 100%: {done}/{total}"
        );
        assert!(super::model_downloaded(model), "present after download");

        super::remove_model_files(model).expect("remove");
        assert!(!super::model_downloaded(model), "absent after remove");
    }
}
