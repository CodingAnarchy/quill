//! Tests for the idle-timer autosave and the save-state indicator (#409).

use super::*;
use crate::workflow::{Triggers, Workflow};
use std::time::{Duration, Instant};

/// Make the active buffer dirty, then pretend the last input was `secs` ago so a
/// `tick_autosave` sees `secs` of genuine idle.
fn dirty_and_idle(app: &mut App, secs: u64) {
    type_str(app, "X"); // an edit → dirty
    app.last_activity = Instant::now() - Duration::from_secs(secs);
}

#[test]
fn autosave_off_never_saves_on_idle() {
    let (mut app, _dir) = two_chapter_app("autosave-off");
    app.autosave_interval_secs = 0; // off — the default
    dirty_and_idle(&mut app, 600);
    app.tick_autosave();
    assert!(app.buffer.dirty, "with autosave off, idling never saves");
}

#[test]
fn autosave_saves_after_the_idle_interval() {
    let (mut app, dir) = two_chapter_app("autosave-on");
    app.autosave_interval_secs = 1;
    dirty_and_idle(&mut app, 2); // 2s idle > the 1s interval
    app.tick_autosave();
    assert!(
        !app.buffer.dirty,
        "the buffer is saved after the idle interval"
    );
    assert!(
        fs::read_to_string(dir.join("one.md"))
            .unwrap()
            .starts_with('X'),
        "the edit reached disk"
    );
}

#[test]
fn autosave_waits_while_the_author_is_active() {
    let (mut app, _dir) = two_chapter_app("autosave-active");
    app.autosave_interval_secs = 30;
    dirty_and_idle(&mut app, 0); // just typed — not idle yet
    app.tick_autosave();
    assert!(
        app.buffer.dirty,
        "autosave holds off until the author pauses"
    );
}

#[test]
fn autosave_does_not_fire_the_on_save_workflow() {
    let (mut app, dir) = two_chapter_app("autosave-no-trigger");
    // An on-save workflow whose report step would be observable if it fired.
    app.workflows = vec![Workflow {
        name: "autosave".into(),
        steps: vec!["report:progress".into()],
    }];
    app.triggers = Triggers {
        on_save: Some("autosave".into()),
        keys: Default::default(),
    };
    app.autosave_interval_secs = 1;
    dirty_and_idle(&mut app, 2);
    app.tick_autosave();
    assert!(!app.buffer.dirty, "autosave still saved the work");
    assert!(
        app.overlay.is_none(),
        "the on-save workflow did not run — autosave can't thrash it"
    );
    assert!(
        !app.message
            .as_deref()
            .unwrap_or("")
            .contains("Ran workflow"),
        "{:?}",
        app.message
    );
    assert!(fs::read_to_string(dir.join("one.md"))
        .unwrap()
        .starts_with('X'));
}

#[test]
fn the_status_bar_shows_saved_and_unsaved_states() {
    let (mut app, _dir) = two_chapter_app("autosave-indicator");
    let clean = crate::ui::render_to_string(&app, 80, 10);
    assert!(
        clean.contains('✓'),
        "a saved buffer shows the saved indicator:\n{clean}"
    );
    type_str(&mut app, "edit");
    let dirty = crate::ui::render_to_string(&app, 80, 10);
    assert!(
        dirty.contains('●'),
        "an unsaved buffer shows the unsaved indicator:\n{dirty}"
    );
}
