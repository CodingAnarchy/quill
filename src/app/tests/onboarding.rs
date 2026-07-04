//! First-run onboarding / quick-start card (#837): the launch-time card shows once
//! (flag unset), dismissing it records the flag, a subsequent launch (flag set)
//! shows nothing, and the help path re-opens it any time. Hermetic (#528/#531) —
//! every test pins the config dir at a scratch directory, so the first-run flag is
//! read and written there, never the developer's real `~/.config/quill`.

use super::*;
use crate::mode::{Action, OverlayKind};

/// Point the config dir at a fresh scratch directory for this test thread, then
/// build a one-chapter app there. The config dir is set before `App::new`, so the
/// editor and the onboarding flag both resolve to the scratch config.
fn onboarding_app(tag: &str) -> (App, TempProject) {
    let cfg = crate::testutil::scratch_dir("onboarding-cfg", tag);
    crate::config::set_config_dir(Some(cfg));
    let dir = make_project(tag, "Book", "first chapter\n");
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

/// Whether the open overlay (if any) is the quick-start card.
fn onboarding_open(app: &App) -> bool {
    app.overlay
        .as_ref()
        .is_some_and(|o| matches!(o.kind, OverlayKind::Onboarding))
}

#[test]
fn first_launch_shows_the_card_and_dismissing_sets_the_flag() {
    let (mut app, _dir) = onboarding_app("first-launch");

    // Fresh install: the launch-time offer shows the card.
    assert!(
        !crate::onboarding::already_shown(),
        "the flag starts unset on a fresh install"
    );
    app.maybe_offer_onboarding();
    assert!(
        onboarding_open(&app),
        "the quick-start card is shown on first launch"
    );

    // Any key dismisses it — and dismissing records the shown-once flag so it never
    // returns on a later launch.
    app.viewport_height = 24;
    app.on_key(key(KeyCode::Esc));
    assert!(app.overlay.is_none(), "the card is dismissed");
    assert!(
        crate::onboarding::already_shown(),
        "dismissing the card records the first-run flag"
    );

    crate::config::set_config_dir(None);
}

#[test]
fn a_subsequent_launch_does_not_show_the_card() {
    let (mut app, _dir) = onboarding_app("second-launch");
    // Simulate a prior run that already showed and dismissed the card.
    crate::onboarding::mark_shown();

    app.maybe_offer_onboarding();
    assert!(
        !onboarding_open(&app),
        "the card never reappears once the flag is set"
    );
    assert!(
        app.overlay.is_none(),
        "no overlay is opened on a later launch"
    );

    crate::config::set_config_dir(None);
}

#[test]
fn the_help_path_reopens_the_card_even_after_dismissal() {
    let (mut app, _dir) = onboarding_app("help-reopen");
    // The card has already been shown once, so it won't auto-open again.
    crate::onboarding::mark_shown();
    app.maybe_offer_onboarding();
    assert!(!onboarding_open(&app), "no auto-open after the flag is set");

    // The "Quick-start" run-action (reachable from the Help group of the palette)
    // re-opens it on demand.
    app.run_action(Action::QuickStart, false);
    assert!(
        onboarding_open(&app),
        "the Quick-start help action re-opens the card"
    );

    crate::config::set_config_dir(None);
}

#[test]
fn the_offer_waits_when_a_launch_prompt_is_already_up() {
    let (mut app, _dir) = onboarding_app("waits-for-prompt");
    // Stand in for a crash-recovery prompt already occupying the screen at launch.
    app.open_prompt(crate::mode::Pending::Quit, "quit?", "", None);

    app.maybe_offer_onboarding();
    assert!(
        !onboarding_open(&app),
        "the card doesn't stack over a launch-time prompt"
    );
    // The flag is untouched — the card is still owed, just deferred to next launch.
    assert!(
        !crate::onboarding::already_shown(),
        "a deferred card leaves the flag unset"
    );

    crate::config::set_config_dir(None);
}
