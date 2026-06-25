//! The local AI model manager (#379/#382): opening it, and the download / remove
//! actions' behavior in a build without the `local` feature (the only behavior
//! exercisable offline — the actual download/remove run only with `--features
//! local` and a network, covered by the gated integration tests).

use super::*;
use crate::mode::Mode;

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
    assert!(!app.ai_task.is_busy(), "nothing is spawned when local isn't compiled in");
}

#[test]
fn removing_a_model_thats_not_downloaded_is_a_noticed_no_op() {
    let (mut app, _dir) = two_chapter_app("models-remove");
    app.open_model_manager();
    app.on_key(key(KeyCode::Char('x'))); // remove the selected model
    assert!(app.message.as_deref().unwrap_or("").contains("isn't downloaded"), "{:?}", app.message);
    // No confirm opens for a model that isn't on disk.
    assert!(matches!(app.mode, Mode::ModelManager { .. }), "stays in the manager");
}
