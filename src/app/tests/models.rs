//! The local AI model manager (#379/#382): opening it, and the download / remove
//! actions' behavior in a build without the `local` feature (the only behavior
//! exercisable offline — the actual download/remove run only with `--features
//! local` and a network, covered by the gated integration tests).

use std::sync::Mutex;

use super::*;
use crate::mode::Mode;

/// `removing_…` decides whether a model is on disk via `worker::model_downloaded`,
/// which under `--features local` reads the real Hugging Face cache
/// (`hf_hub::Cache::from_env`). Redirect that cache to an empty temp dir so the
/// outcome can't depend on what the developer happens to have cached. The cache
/// env is process-global, so serialize the cache-touching model tests under this
/// lock (mirrors the `ENV_LOCK`/`isolate` pattern in `projects.rs`).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Point the Hugging Face cache env vars at a fresh, empty temp dir so
/// `model_downloaded` finds nothing, never the developer's real
/// `~/.cache/huggingface`. Sets both vars `Cache::from_env` consults so the
/// redirect holds whichever one is set in the surrounding environment.
fn isolate_hf_cache(tag: &str) {
    let dir = crate::testutil::scratch_dir("hfcache", tag);
    std::env::set_var("HF_HOME", &dir);
    std::env::set_var("HUGGINGFACE_HUB_CACHE", &dir);
}

#[test]
fn opening_the_manager_enters_its_mode() {
    let (mut app, _dir) = two_chapter_app("models-open");
    app.open_model_manager();
    assert!(matches!(app.mode, Mode::ModelManager { selected: 0 }));
}

#[cfg(not(feature = "local"))]
#[test]
fn downloading_without_the_local_feature_explains_how() {
    let (mut app, _dir) = two_chapter_app("models-download");
    app.open_model_manager();
    app.on_key(key(KeyCode::Char('d'))); // download the selected (first) model
    let msg = app.message.as_deref().unwrap_or("");
    assert!(msg.contains("--features local"), "{msg}");
    assert!(
        !app.ai_task.is_busy(),
        "nothing is spawned when local isn't compiled in"
    );
}

#[test]
fn removing_a_model_thats_not_downloaded_is_a_noticed_no_op() {
    let _g = ENV_LOCK.lock().unwrap();
    isolate_hf_cache("models-remove");
    let (mut app, _dir) = two_chapter_app("models-remove");
    app.open_model_manager();
    app.on_key(key(KeyCode::Char('x'))); // remove the selected model
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("isn't downloaded"),
        "{:?}",
        app.message
    );
    // No confirm opens for a model that isn't on disk.
    assert!(
        matches!(app.mode, Mode::ModelManager { .. }),
        "stays in the manager"
    );
}
