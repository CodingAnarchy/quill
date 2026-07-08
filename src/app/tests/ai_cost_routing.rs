//! AI cost confirmation, revision triage, incremental pass scoping, the backend switch, and hybrid routing.

use super::*;
use crate::ai::router::Policy;
use crate::app::palette::AiStart;
use crate::mode::AiAction;
use std::fs;

// --- AI: cost confirm & revision triage -------------------------------------

fn findings_len(app: &App) -> usize {
    match &app.mode {
        Mode::Findings(f) => f.items.len(),
        _ => panic!("expected the findings overlay"),
    }
}

#[test]
fn an_expensive_pass_asks_for_confirmation_first() {
    let (mut app, _dir) = two_chapter_app("cost-confirm");
    app.ai.cost_threshold = 0.0; // any non-empty manuscript exceeds it
    app.start_ai_action(crate::mode::AiAction::Continuity, false);
    match &app.mode {
        Mode::Prompt(s) => {
            assert!(
                matches!(s.pending, Pending::ConfirmAi { .. }),
                "cost confirm opens"
            );
            assert!(s.label.contains("run?"), "{}", s.label);
        }
        _ => panic!("expected the cost confirm prompt"),
    }
}

#[test]
fn a_cheap_pass_runs_without_confirmation() {
    let (mut app, _dir) = two_chapter_app("cost-cheap");
    app.ai.cost_threshold = 1_000.0; // nothing this small could exceed it
    app.start_ai_action(crate::mode::AiAction::Style, false);
    // It dispatched straight through (no key configured → a status message),
    // never stopping at a confirm prompt.
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "no confirm for a cheap pass"
    );
}

#[test]
fn confirming_an_expensive_pass_dispatches_it() {
    let (mut app, _dir) = two_chapter_app("cost-yes");
    app.ai.cost_threshold = 0.0;
    app.start_ai_action(crate::mode::AiAction::Continuity, false);
    assert!(matches!(app.mode, Mode::Prompt(_)));
    app.on_key(key(KeyCode::Char('y'))); // confirm → dispatch (no key → message)
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "confirm closes the prompt"
    );
}

#[test]
fn declining_an_expensive_pass_cancels_it() {
    let (mut app, _dir) = two_chapter_app("cost-no");
    app.ai.cost_threshold = 0.0;
    app.start_ai_action(crate::mode::AiAction::Continuity, false);
    app.on_key(key(KeyCode::Char('n')));
    assert!(
        matches!(app.mode, Mode::Insert),
        "declining returns to editing"
    );
    assert!(!app.ai_task.is_busy(), "nothing was sent");
}

#[test]
fn triage_accumulates_findings_across_passes_deduped() {
    let (mut app, _dir) = two_chapter_app("triage-accum");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "the locket changes color".into(),
        chapters: vec!["One".into()],
    }])));
    app.apply_style(Ok(ai::style::Style {
        findings: vec![ai::style::StyleFinding {
            kind: "POV".into(),
            detail: "head-hop".into(),
            chapters: vec!["Two".into()],
        }],
    }));
    // A repeat of the continuity finding must not double-count.
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "the locket changes color".into(),
        chapters: vec!["One".into()],
    }])));
    app.open_triage();
    assert_eq!(findings_len(&app), 2, "two distinct findings, deduped");
}

#[test]
fn triage_with_no_findings_shows_a_message() {
    let (mut app, _dir) = two_chapter_app("triage-empty");
    app.open_triage();
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(
        app.message.as_deref().unwrap_or("").contains("No findings"),
        "{:?}",
        app.message
    );
}

// --- Incremental pass scoping (#90) ----------------------------------------

/// Write a character bible file so the "up to date" path (bible present + state
/// recorded) is reachable.
fn write_character_bible(dir: &std::path::Path) {
    let chars = dir.join("bible").join("characters");
    fs::create_dir_all(&chars).unwrap();
    fs::write(
        chars.join("mara.md"),
        "---\nname: Mara\n---\n\n<!-- generated:start -->\nprofile\n<!-- generated:end -->\n",
    )
    .unwrap();
}

#[test]
fn an_up_to_date_bible_pass_makes_no_request() {
    let (mut app, dir) = two_chapter_app("incr-uptodate");
    // A bible exists and its recorded state matches the manuscript → nothing to do.
    write_character_bible(&dir);
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests())
        .unwrap();
    app.analyze_characters(false);
    assert!(!app.ai_busy(), "an up-to-date bible spawns no request");
    assert!(
        app.message.as_deref().unwrap_or("").contains("current"),
        "{:?}",
        app.message
    );
}

#[test]
fn forcing_a_rebuild_overrides_an_up_to_date_bible() {
    let (app, dir) = two_chapter_app("incr-force");
    write_character_bible(&dir);
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests())
        .unwrap();
    let action = crate::mode::AiAction::Characters;
    assert_eq!(
        app.pass_context_len(action, false),
        0,
        "up to date by default"
    );
    assert_eq!(
        app.pass_context_len(action, true),
        app.manuscript_text().len(),
        "the palette's R (force) rebuilds from the whole manuscript anyway"
    );
}

#[test]
fn a_hand_deleted_bible_rebuilds_even_when_state_says_current() {
    let (app, dir) = two_chapter_app("incr-drift");
    // The state claims the bible is built from the current manuscript, but no
    // bible file exists on disk (deleted by hand) — rebuild rather than skip.
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests())
        .unwrap();
    assert_eq!(
        app.pass_context_len(crate::mode::AiAction::Characters, false),
        app.manuscript_text().len(),
        "a missing bible forces a full rebuild, not an up-to-date skip"
    );
}

#[test]
fn an_edited_chapter_scopes_the_pass_to_just_that_chapter() {
    let (mut app, dir) = two_chapter_app("incr-scope");
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests())
        .unwrap();
    // Edit chapter Two and save; only it is now part of the delta.
    app.on_key(ctrl('n')); // active → Two
    type_str(&mut app, "NEWWORD");
    app.on_key(ctrl('s'));

    let state = ai::incremental::State::load(&dir);
    let scope = ai::incremental::decide(state.built_from("characters"), &app.chapter_digests(), 5);
    assert_eq!(
        scope,
        ai::incremental::Scope::Incremental(vec!["two.md".into()])
    );
    // The incremental context is only the changed chapter, not the whole book.
    let changed = app.chapters_text(&["two.md".to_string()]);
    assert!(
        changed.contains("NEWWORD") && !changed.contains("first chapter"),
        "{changed}"
    );
}

#[test]
fn cost_context_is_the_whole_manuscript_first_then_zero_when_current() {
    let (app, dir) = two_chapter_app("incr-cost");
    let action = crate::mode::AiAction::Characters;
    assert_eq!(
        app.pass_context_len(action, false),
        app.manuscript_text().len(),
        "first run estimates the full manuscript"
    );
    write_character_bible(&dir);
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests())
        .unwrap();
    assert_eq!(
        app.pass_context_len(action, false),
        0,
        "an up-to-date bible costs nothing to confirm"
    );
}

#[test]
fn a_full_rebuild_estimates_the_batch_rate_but_an_incremental_pass_full() {
    // A from-scratch full rebuild runs through the Batch API at half price on the
    // Claude backend, so its estimate must halve; an incremental pass runs at the
    // standard rate and estimates full (#691). `two_chapter_app` defaults to the
    // Claude backend, so the batch path applies.
    let (mut app, dir) = two_chapter_app("batch-estimate");
    let action = AiAction::Characters;
    assert_eq!(app.ai.backend(), ai::BackendKind::Claude);

    // First run: no bible yet, so the pass rebuilds the whole book from scratch and
    // runs batched. Its estimate is half the same pass priced at the standard rate.
    assert!(
        app.pass_batches(action, false),
        "a from-scratch full rebuild runs batched"
    );
    let chars = app.pass_context_len(action, false);
    let batched = crate::ai::cost::estimate(chars, ai::MAX_OUTPUT_TOKENS, &app.ai.model, true);
    let standard = crate::ai::cost::estimate(chars, ai::MAX_OUTPUT_TOKENS, &app.ai.model, false);
    assert!(
        (batched.dollars - standard.dollars / 2.0).abs() < 1e-9,
        "the full rebuild estimates the batch (half) figure: {} vs {}",
        batched.dollars,
        standard.dollars
    );

    // Now a bible exists and only one chapter changed → an incremental pass, which
    // does not batch, so it estimates the full rate.
    write_character_bible(&dir);
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests())
        .unwrap();
    app.on_key(ctrl('n')); // active → Two
    type_str(&mut app, "NEWWORD");
    app.on_key(ctrl('s'));
    assert!(
        matches!(
            app.plan_bible_pass_for_test(&super::worker::bible_tracker(action).unwrap(), 0)
                .0,
            ai::incremental::Scope::Incremental(_)
        ),
        "the edited chapter scopes an incremental pass"
    );
    assert!(
        !app.pass_batches(action, false),
        "an incremental pass runs at the standard rate, not batched"
    );
}

#[test]
fn a_clean_bible_pass_records_what_it_built_from() {
    let (mut app, dir) = two_chapter_app("incr-record");
    let built = app.chapter_digests();
    // Stand in for what analyze_characters sets just before spawning the request.
    app.pending_pass = Some(("characters".to_string(), built.clone(), None));
    app.apply_characters(Ok(vec![Character {
        name: "Mara".into(),
        ..Default::default()
    }]));
    let state = ai::incremental::State::load(&dir);
    assert_eq!(
        state.built_from("characters"),
        Some(&built),
        "the pass recorded its manuscript state"
    );
    assert!(app.pending_pass.is_none(), "the snapshot was consumed");
}

// --- AI backend switch (#371) ----------------------------------------------

#[test]
fn offline_pin_blocks_the_backend_switch() {
    let (mut app, _dir) = two_chapter_app("be-offline");
    app.ai.enabled = true;
    app.ai.offline = true;
    let before = app.ai.backend;
    app.toggle_ai_backend();
    assert_eq!(
        app.ai.backend, before,
        "the offline pin leaves the configured backend untouched"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Offline mode pins"),
        "{:?}",
        app.message
    );
    assert_eq!(app.ai_backend_label(), "local (offline pinned)");
}

#[cfg(not(feature = "local"))]
#[test]
fn switching_to_local_without_the_feature_explains_how() {
    let (mut app, _dir) = two_chapter_app("be-nolocal");
    app.ai.enabled = true;
    app.ai.backend = ai::BackendKind::Claude;
    app.toggle_ai_backend(); // would flip to local, but it isn't compiled in
    assert_eq!(
        app.ai.backend,
        ai::BackendKind::Claude,
        "stays on Claude rather than pretending to switch"
    );
    let msg = app.message.as_deref().unwrap_or("");
    assert!(msg.contains("--features local"), "{msg}");
}

#[cfg(feature = "local")]
#[test]
fn switching_flips_between_claude_and_local() {
    let (mut app, _dir) = two_chapter_app("be-flip");
    app.ai.enabled = true;
    app.ai.backend = ai::BackendKind::Claude;

    app.toggle_ai_backend();
    assert_eq!(app.ai.backend, ai::BackendKind::Local);
    assert_eq!(app.ai_backend_label(), "local (on-device)");

    app.toggle_ai_backend();
    assert_eq!(app.ai.backend, ai::BackendKind::Claude);
    assert_eq!(app.ai_backend_label(), "Claude (cloud)");
}

// --- Hybrid routing (#372) -------------------------------------------------

#[test]
fn offline_routes_costed_actions_on_device() {
    let (mut app, _dir) = two_chapter_app("route-offline");
    app.ai.enabled = true;
    app.ai.offline = true; // the hard pin
    assert!(
        matches!(
            app.ai_action_start(AiAction::Continuity, false),
            AiStart::Run(ai::BackendKind::Local)
        ),
        "offline pins every pass on-device",
    );
}

/// A project with a single chapter big enough that its estimated tokens
/// (`bytes / 4`) exceed the on-device window (40960) — the offline-oversized case
/// (#849). ~250 KB of body ⇒ ~64k estimated tokens.
fn oversized_offline_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Big\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "word ".repeat(50_000)).unwrap();
    let mut app = open_test_app(&dir);
    app.ai.enabled = true;
    app.ai.offline = true; // the hard privacy pin
    app.test_local_present = Some(true); // a usable local model is on disk
    (app, TempProject::new(dir))
}

#[test]
fn offline_refuses_a_non_foldable_oversized_pass() {
    // Prose/style is a one-shot manuscript pass with no incremental-revise path, so
    // an oversized offline run can't fold → the graceful refusal (#849), never a run
    // that would feed candle a pass it can't hold.
    let (app, _dir) = oversized_offline_app("offline-oversized-refuse");
    match app.ai_action_start(AiAction::Style, false) {
        AiStart::Refuse(msg) => {
            assert!(msg.contains("40960"), "states the window: {msg}");
            assert!(msg.contains("offline"), "names the remedy: {msg}");
            assert!(msg.contains("narrow"), "offers the scope remedy: {msg}");
        }
        other => panic!("expected a refusal for an oversized non-foldable pass, got {other:?}"),
    }
}

#[test]
fn offline_folds_an_oversized_bible_pass_on_device() {
    // A persistent-bible pass (continuity) has an incremental-revise path, so an
    // oversized offline rebuild folds on-device instead of refusing (#849): it
    // routes Local and the fold runs chapter-by-chapter under the window.
    let (app, _dir) = oversized_offline_app("offline-oversized-fold");
    assert!(
        matches!(
            app.ai_action_start(AiAction::Continuity, false),
            AiStart::Run(ai::BackendKind::Local)
        ),
        "a foldable oversized offline pass runs on-device (folds), not refused",
    );
}

#[test]
fn ask_policy_confirms_when_over_the_threshold() {
    let (mut app, _dir) = two_chapter_app("route-ask");
    app.ai.enabled = true;
    app.ai.routing = Policy::Ask;
    app.ai.cost_threshold = 0.0; // any non-empty manuscript is "over"
    assert!(
        matches!(
            app.ai_action_start(AiAction::Continuity, false),
            AiStart::Confirm { .. }
        ),
        "the ask policy surfaces the choice over the threshold",
    );
}

#[test]
fn the_default_policy_keeps_the_plain_cost_confirm() {
    let (mut app, _dir) = two_chapter_app("route-default");
    app.ai.enabled = true; // routing defaults to Configured
    app.ai.cost_threshold = 0.0;
    match app.ai_action_start(AiAction::Continuity, false) {
        AiStart::Confirm { offer_local, label } => {
            assert!(
                !offer_local,
                "the default policy doesn't offer a local detour"
            );
            assert!(label.contains("run?"), "{label}");
        }
        _ => panic!("expected the historical cost confirm"),
    }
}

#[cfg(feature = "local")]
#[test]
fn prefer_local_runs_a_fitting_pass_on_device() {
    let (mut app, _dir) = two_chapter_app("route-prefer-local");
    app.ai.enabled = true;
    app.ai.routing = Policy::PreferLocal;
    app.test_local_present = Some(true); // a usable local model is on disk (#725)
    assert!(
        matches!(
            app.ai_action_start(AiAction::Continuity, false),
            AiStart::Run(ai::BackendKind::Local)
        ),
        "prefer-local keeps a small pass on-device",
    );
}

// --- Local-not-downloaded fallback (#725) ----------------------------------

/// The bug: `cost-ceiling` + `--features local` compiled in, but no local model
/// downloaded. An over-threshold pass must NOT route on-device (which would kick
/// off a surprise HuggingFace fetch); a not-downloaded model is "local
/// unavailable," so the pass runs on Claude instead.
#[cfg(feature = "local")]
#[test]
fn cost_ceiling_does_not_route_local_when_the_model_isnt_downloaded() {
    let (mut app, _dir) = two_chapter_app("route-not-downloaded");
    app.ai.enabled = true;
    app.ai.routing = Policy::CostCeiling;
    app.ai.cost_threshold = 0.0; // any non-empty manuscript is "over the ceiling"
    app.test_local_present = Some(false); // compiled in, but nothing downloaded

    // Over the ceiling but no local model present → Claude with a notice, never
    // the on-device route that would trigger a download.
    match app.ai_action_start(AiAction::Continuity, false) {
        AiStart::Run(ai::BackendKind::Claude) => {}
        AiStart::Confirm { offer_local, .. } => {
            assert!(!offer_local, "no local detour when nothing is downloaded");
        }
        other => panic!("expected a Claude route/confirm, not {other:?}"),
    }
}

/// With a usable local model on disk, the same over-ceiling pass does route
/// on-device — confirming the not-downloaded case above is what gates it.
#[cfg(feature = "local")]
#[test]
fn cost_ceiling_routes_local_when_the_model_is_downloaded() {
    let (mut app, _dir) = two_chapter_app("route-downloaded");
    app.ai.enabled = true;
    app.ai.routing = Policy::CostCeiling;
    app.ai.cost_threshold = 0.0;
    app.test_local_present = Some(true); // the model's weights are on disk

    assert!(
        matches!(
            app.ai_action_start(AiAction::Continuity, false),
            AiStart::Run(ai::BackendKind::Local)
        ),
        "over the ceiling with a downloaded model → on-device",
    );
}
