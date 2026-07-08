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

/// This host's name, matching the lock module's derivation, so a seeded lockfile
/// reads as same-host (mirrors the project hardening tests).
#[cfg(unix)]
fn host() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "unknown-host".to_string())
}

/// Like [`onboarding_app`], but seeds a *live* other-instance lock before opening
/// so the project opens read-only (#871/#885): a lockfile owned by pid 1 (always
/// alive on Unix), the shape a genuine second Quill's lock drives. A unit test
/// can't fork a second editor, so this stands in for one. Unix-only, matching the
/// lock's platform coverage.
#[cfg(unix)]
fn read_only_onboarding_app(tag: &str) -> (App, TempProject) {
    let cfg = crate::testutil::scratch_dir("onboarding-cfg", tag);
    crate::config::set_config_dir(Some(cfg));
    let dir = make_project(tag, "Book", "first chapter\n");
    std::fs::create_dir_all(dir.join(".quill")).unwrap();
    let body = format!("1\n{}\n0\n", host());
    std::fs::write(dir.join(".quill").join("lock"), body).unwrap();
    let app = open_test_app(&dir);
    assert!(
        app.project.is_read_only(),
        "the seeded live lock opens the project read-only"
    );
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

/// Whether the currently-open onboarding card (if any) carries the read-only
/// banner, flattened to plain text so the check isn't coupled to styling.
fn onboarding_shows_read_only(app: &App) -> bool {
    app.overlay_lines().is_some_and(|lines| {
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        text.contains("Read-only") && text.contains("another Quill instance")
    })
}

/// #885: a project opened read-only (a live other-instance lock) must surface the
/// read-only state to a first-run author even though the onboarding card would
/// overlay the momentary open-time warning. The card leads with the read-only
/// banner, and the status bar carries a persistent read-only tag underneath — so
/// the state is visible while the card is up.
#[cfg(unix)]
#[test]
fn read_only_state_is_visible_even_when_onboarding_shows() {
    let (mut app, _dir) = read_only_onboarding_app("readonly-onboarding");

    // First run: the card auto-opens, and it leads with the read-only banner so
    // the author sees why saves are refused rather than only the tour.
    app.maybe_offer_onboarding();
    assert!(
        onboarding_open(&app),
        "the quick-start card is still shown on a read-only first run"
    );
    assert!(
        onboarding_shows_read_only(&app),
        "the read-only banner leads the onboarding card"
    );

    crate::config::set_config_dir(None);
}

/// #885: dismissing the onboarding card must leave the read-only state visible —
/// the transient open-time message clears on the next keystroke, so the status
/// bar keeps a persistent "read-only" tag for as long as the lock is held.
#[cfg(unix)]
#[test]
fn dismissing_onboarding_leaves_the_read_only_state_visible() {
    let (mut app, _dir) = read_only_onboarding_app("readonly-persists");

    app.maybe_offer_onboarding();
    assert!(
        onboarding_open(&app),
        "card shows on the read-only first run"
    );

    // Dismiss the card, then step the buffer so any transient open-time message
    // would clear — the persistent status-bar tag must remain.
    app.viewport_height = 24;
    app.on_key(key(KeyCode::Esc));
    assert!(app.overlay.is_none(), "the card is dismissed");
    app.on_key(key(KeyCode::Char('x')));

    let screen = crate::ui::render_to_string(&app, 100, 24);
    assert!(
        screen.contains("read-only"),
        "the status bar keeps a persistent read-only tag after dismissal:\n{screen}"
    );

    crate::config::set_config_dir(None);
}

/// #885 guard: a normal (writable) first run still shows the onboarding card as
/// before, with no read-only banner and no read-only tag in the status bar — the
/// fix must not leak the read-only surface onto a project that isn't locked.
#[test]
fn a_writable_first_run_still_shows_plain_onboarding() {
    let (mut app, _dir) = onboarding_app("writable-first-run");
    assert!(!app.project.is_read_only(), "a free project opens writable");

    app.maybe_offer_onboarding();
    assert!(
        onboarding_open(&app),
        "the card shows on a writable first run"
    );
    assert!(
        !onboarding_shows_read_only(&app),
        "no read-only banner on a writable project"
    );

    app.viewport_height = 24;
    app.on_key(key(KeyCode::Esc));
    let screen = crate::ui::render_to_string(&app, 100, 24);
    assert!(
        !screen.contains("read-only"),
        "no read-only tag in the status bar for a writable project:\n{screen}"
    );

    crate::config::set_config_dir(None);
}
