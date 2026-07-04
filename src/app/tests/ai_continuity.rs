//! AI timeline & continuity: writing the timeline, the delta on a clean revise,
//! persistent revision triage (#591), scoped re-check of a single finding (#608),
//! and the setting / worldbuilding pass.

use super::*;
use std::fs;

// --- AI: timeline & continuity ---------------------------------------------

#[test]
fn continuity_writes_timeline_and_opens_findings() {
    let (mut app, dir) = two_chapter_app("cont-write");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "the locket changes color".into(),
        chapters: vec!["One".into()],
    }])));
    let tl = fs::read_to_string(dir.join("bible/timeline.md")).expect("timeline written");
    assert!(tl.contains("alpha event"));
    assert!(
        matches!(app.mode, Mode::Findings(_)),
        "findings overlay should open"
    );
}

#[test]
fn continuity_with_no_findings_summarizes() {
    let (mut app, dir) = two_chapter_app("cont-clean");
    app.apply_continuity(Ok(cont_with(vec![])));
    assert!(dir.join("bible/timeline.md").exists());
    assert!(app.overlay.is_some(), "summary overlay");
    assert!(!matches!(app.mode, Mode::Findings(_)));
}

/// A continuity result with a single event of the given description and no
/// findings — for the revise-delta tests, where a second pass rewrites the same
/// clean (AI-written) timeline with different content.
fn cont_event(description: &str) -> Continuity {
    Continuity {
        events: vec![continuity::Event {
            description: description.into(),
            chapter: "One".into(),
            story_order: 1,
        }],
        findings: Vec::new(),
    }
}

#[test]
fn clean_revise_surfaces_the_delta() {
    let (mut app, dir) = two_chapter_app("cont-delta");
    // First pass establishes a clean, AI-written timeline (no findings).
    app.apply_continuity(Ok(cont_event("alpha event")));
    assert!(dir.join("bible/timeline.md").exists());

    // A second pass rewrites the same file with different content. The file was
    // AI-written (not hand-edited), so it's a clean write — and the author should
    // see what changed (#607).
    app.apply_continuity(Ok(cont_event("beta event")));
    let panel = app
        .overlay_lines()
        .expect("a clean revise that changed the tracker opens the delta overlay");
    let text: String = panel
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.starts_with("Updated timeline: +"),
        "the panel leads with a `+N/-M` delta summary: {text:?}"
    );
    assert!(
        text.contains("+- **One:** beta event") && text.contains("-- **One:** alpha event"),
        "the diff shows the change from alpha to beta: {text:?}"
    );
}

#[test]
fn revise_with_findings_puts_the_delta_summary_in_the_message() {
    let (mut app, _dir) = two_chapter_app("cont-delta-findings");
    // A clean first pass, then a revise that both changes the tracker and
    // surfaces a finding. Findings lead (the overlay), but the `+N/-M` summary
    // rides the message line so the delta isn't lost.
    app.apply_continuity(Ok(cont_event("alpha event")));
    app.apply_continuity(Ok(Continuity {
        events: vec![continuity::Event {
            description: "beta event".into(),
            chapter: "One".into(),
            story_order: 1,
        }],
        findings: vec![continuity::Finding {
            kind: "object".into(),
            detail: "the locket changes color".into(),
            chapters: vec!["One".into()],
        }],
    }));
    assert!(
        matches!(app.mode, Mode::Findings(_)),
        "findings still lead the review"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .starts_with("Updated timeline: +"),
        "the delta summary rides the message line: {:?}",
        app.message
    );
}

#[test]
fn no_change_revise_shows_no_delta() {
    let (mut app, _dir) = two_chapter_app("cont-nodelta");
    app.apply_continuity(Ok(cont_event("alpha event")));

    // Re-run the identical pass: the file is unchanged, so there's no delta to
    // show — the historic clean-write summary, not a diff.
    app.apply_continuity(Ok(cont_event("alpha event")));
    let text = app
        .overlay_lines()
        .expect("the clean-write summary still shows")
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !text.contains("Updated") && !text.contains("+beta"),
        "a no-change write surfaces no delta: {text:?}"
    );
}

#[test]
fn findings_dismiss_removes_and_closes_when_empty() {
    let (mut app, _dir) = two_chapter_app("cont-dismiss");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "age".into(),
        detail: "Mara is 30 then 28".into(),
        chapters: vec!["One".into()],
    }])));
    assert!(matches!(app.mode, Mode::Findings(_)));
    app.on_key(key(KeyCode::Char('d'))); // dismiss the only finding
    assert!(matches!(app.mode, Mode::Insert), "empty list closes");
}

// --- Persistent revision triage (#591) -------------------------------------

/// A continuity result carrying a single finding with the given detail.
fn one_finding(detail: &str) -> Continuity {
    cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: detail.into(),
        chapters: vec!["One".into()],
    }])
}

#[test]
fn dismissed_finding_stays_dismissed_after_reload() {
    let (mut app, dir) = two_chapter_app("triage-persist-dismiss");
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    assert!(matches!(app.mode, Mode::Findings(_)));
    app.on_key(key(KeyCode::Char('d'))); // dismiss the only finding
    assert!(
        app.triage.is_empty(),
        "dismissed leaves the session triage list"
    );

    // Reopen the project (a new session): the dismissed finding must not return.
    let reopened = open_test_app(&dir);
    assert!(
        reopened.triage.is_empty(),
        "a dismissed finding stays dismissed across sessions"
    );
    drop(dir);
}

#[test]
fn rerun_does_not_resurface_a_dismissed_finding() {
    let (mut app, dir) = two_chapter_app("triage-persist-rerun");
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    app.on_key(key(KeyCode::Char('d'))); // dismiss it
    assert!(matches!(app.mode, Mode::Insert), "list emptied and closed");

    // Re-run the same pass: it re-produces the identical finding, which must be
    // filtered out rather than popping the triage overlay again.
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    assert!(
        app.triage.is_empty(),
        "a re-produced dismissed finding does not re-surface"
    );
    assert!(
        !matches!(app.mode, Mode::Findings(_)),
        "no overlay for an already-dismissed finding"
    );
    drop(dir);
}

#[test]
fn open_triage_list_survives_a_restart() {
    let (mut app, dir) = two_chapter_app("triage-persist-survive");
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    assert_eq!(app.triage.len(), 1);
    app.on_key(key(KeyCode::Esc)); // leave the finding open (neither dismiss nor mark)

    // A new session restores the still-open finding as a standing to-do.
    let mut reopened = open_test_app(&dir);
    assert_eq!(
        reopened.triage.len(),
        1,
        "an un-triaged finding survives a restart"
    );
    reopened.open_triage();
    assert!(
        matches!(reopened.mode, Mode::Findings(_)),
        "the restored finding opens in triage"
    );
    drop(dir);
}

#[test]
fn marking_xxx_resolves_and_drops_the_finding() {
    let (mut app, dir) = two_chapter_app("triage-persist-mark");
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    assert!(matches!(app.mode, Mode::Findings(_)));
    app.on_key(key(KeyCode::Char('i'))); // mark xxx in the chapter
    let body = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(
        body.contains("xxx —"),
        "the marker is written (unchanged behavior)"
    );
    assert!(app.triage.is_empty(), "a resolved finding drops off triage");

    // It stays gone across a restart.
    let reopened = open_test_app(&dir);
    assert!(
        reopened.triage.is_empty(),
        "a resolved finding does not return"
    );
    drop(dir);
}

#[test]
fn findings_jump_activates_the_chapter() {
    let (mut app, _dir) = two_chapter_app("cont-jump");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "location".into(),
        detail: "two places at once".into(),
        chapters: vec!["Two".into()],
    }])));
    app.on_key(key(KeyCode::Enter)); // jump to the finding's chapter
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 1, "jumped to chapter Two");
}

#[test]
fn hand_edited_timeline_reviews_then_shows_findings() {
    let (mut app, dir) = two_chapter_app("cont-conflict");
    // First pass: clean timeline (with a finding so findings are pending later too).
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "first".into(),
        chapters: vec!["One".into()],
    }])));
    let tl = dir.join("bible/timeline.md");
    // Hand-edit the generated block.
    let edited = fs::read_to_string(&tl)
        .unwrap()
        .replace("alpha event", "TAMPERED");
    fs::write(&tl, edited).unwrap();

    // Second pass with findings → timeline conflict first.
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "second".into(),
        chapters: vec!["One".into()],
    }])));
    assert!(
        matches!(app.mode, Mode::BibleReview(_)),
        "timeline conflict reviewed first"
    );

    app.on_key(key(KeyCode::Char('k'))); // keep mine → drains queue → findings open
    assert!(
        matches!(app.mode, Mode::Findings(_)),
        "findings follow the review"
    );
}

// --- AI: setting & worldbuilding -------------------------------------------

#[test]
fn setting_writes_bible_and_opens_rule_findings() {
    let (mut app, dir) = two_chapter_app("setting-write");
    let s = Setting {
        categories: vec![setting::Category {
            name: "Magic system".into(),
            entries: vec![setting::Entry {
                name: "Casting".into(),
                detail: "needs a focus".into(),
            }],
        }],
        gazetteer: vec!["Aldermoor".into()],
        findings: vec![setting::RuleFinding {
            rule: "magic needs a focus".into(),
            violation: "casts barehanded".into(),
            chapters: vec!["Two".into()],
        }],
    };
    app.apply_setting(Ok(s));
    let file = fs::read_to_string(dir.join("bible/setting.md")).expect("setting written");
    assert!(file.contains("## Magic system"));
    assert!(file.contains("## Gazetteer"));
    assert!(
        matches!(app.mode, Mode::Findings(_)),
        "rule findings overlay opens"
    );
}

#[test]
fn setting_with_no_violations_summarizes() {
    let (mut app, dir) = two_chapter_app("setting-clean");
    let s = Setting {
        categories: vec![setting::Category {
            name: "Geography".into(),
            entries: vec![],
        }],
        gazetteer: vec![],
        findings: vec![],
    };
    app.apply_setting(Ok(s));
    assert!(dir.join("bible/setting.md").exists());
    assert!(app.overlay.is_some(), "summary overlay");
    assert!(!matches!(app.mode, Mode::Findings(_)));
}

// --- Re-check a single finding (#608) --------------------------------------

/// A continuity UI finding on chapter `chapter`, tagged to the continuity pass so
/// it can be re-checked. Mirrors how `apply_continuity` builds a finding's summary
/// (`kind: detail`), so a re-run that re-parses to the same content matches it.
fn continuity_ui_finding(chapter: &str, detail: &str) -> crate::mode::Finding {
    crate::mode::Finding {
        summary: format!("object: {detail}"),
        chapters: vec![chapter.into()],
        book: None,
        action: Some(crate::mode::AiAction::Continuity),
    }
}

/// A continuity result carrying one finding of `detail` on `chapter` — the JSON a
/// scoped re-check parses, wrapped through the pass's own parser so its summary
/// exactly matches `continuity_ui_finding`.
fn continuity_json(chapter: &str, detail: &str) -> String {
    format!(
        r#"{{"events":[],"findings":[{{"kind":"object","detail":"{detail}","chapters":["{chapter}"]}}]}}"#
    )
}

#[test]
fn recheck_request_is_scoped_to_the_findings_chapters() {
    let (app, _dir) = two_chapter_app("recheck-scope");
    // A finding on chapter Two only. Its re-check request must carry Two's prose
    // and not One's — the whole point of a scoped re-check (#608).
    let finding = continuity_ui_finding("Two", "the locket changes color");
    let req = app
        .recheck_request_for(&finding)
        .expect("a continuity finding builds a re-check request");
    assert!(
        req.cached_context.contains("# Two") && req.cached_context.contains("second chapter"),
        "the finding's chapter is in the request: {}",
        req.cached_context
    );
    assert!(
        !req.cached_context.contains("first chapter"),
        "the other chapter is not sent: {}",
        req.cached_context
    );
}

#[test]
fn recheck_that_still_flags_keeps_the_finding() {
    let (mut app, _dir) = two_chapter_app("recheck-keep");
    // Surface a continuity finding into the overlay + triage the normal way.
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    assert!(matches!(app.mode, Mode::Findings(_)));
    assert_eq!(app.triage.len(), 1);
    let finding = continuity_ui_finding("One", "the locket changes color");

    // The scoped re-run re-produces the same finding: the fix didn't hold. Parse
    // the mock response through the real re-check parser, so the fresh finding's
    // content key exactly matches the one under re-check.
    let fresh = app.recheck_findings_from(
        &finding,
        &continuity_json("One", "the locket changes color"),
    );
    assert_eq!(
        fresh,
        vec![finding.clone()],
        "parser reproduces the finding"
    );
    app.pending_recheck = Some(finding.clone());
    app.apply_recheck(Ok(fresh));

    assert!(app.triage.contains(&finding), "still-flagged finding stays");
    assert!(
        matches!(app.mode, Mode::Findings(f) if f.items.contains(&finding)),
        "it remains in the overlay"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Still flagged"),
        "{:?}",
        app.message
    );
}

#[test]
fn recheck_that_clears_drops_the_finding() {
    let (mut app, _dir) = two_chapter_app("recheck-clear");
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    assert_eq!(app.triage.len(), 1);
    let finding = continuity_ui_finding("One", "the locket changes color");

    // The scoped re-run produces no findings: the fix resolved it.
    app.pending_recheck = Some(finding.clone());
    app.apply_recheck(Ok(Vec::new()));

    assert!(
        !app.triage.contains(&finding),
        "a cleared finding leaves triage"
    );
    assert!(
        matches!(app.mode, Mode::Insert),
        "the last overlay item cleared closes the overlay"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("Cleared"),
        "{:?}",
        app.message
    );
}

#[test]
fn recheck_accumulates_new_findings_silently() {
    let (mut app, _dir) = two_chapter_app("recheck-new");
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    let rechecked = continuity_ui_finding("One", "the locket changes color");
    // The scoped re-run clears the original but surfaces a *different* one in the
    // same chapter. The new one enters triage but doesn't re-open a fresh sweep.
    let fresh = vec![continuity_ui_finding("One", "the candle is lit then unlit")];
    app.pending_recheck = Some(rechecked.clone());
    app.apply_recheck(Ok(fresh.clone()));

    assert!(
        !app.triage.contains(&rechecked),
        "the verified finding cleared"
    );
    assert!(
        app.triage.contains(&fresh[0]),
        "the newly-surfaced finding accumulated into triage"
    );
}

#[test]
fn recheck_is_a_noop_for_a_finding_without_a_pass() {
    let (mut app, _dir) = two_chapter_app("recheck-noop");
    // A finding with no originating action (a Q&A citation, or one restored from
    // disk) can't be re-checked — a status, no request in flight.
    app.mode = Mode::Findings(Box::new(Findings {
        items: vec![crate::mode::Finding {
            summary: "cited: One".into(),
            chapters: vec!["One".into()],
            book: None,
            action: None,
        }],
        selected: 0,
        filter: String::new(),
        typing: false,
    }));
    app.recheck_finding(0);
    assert!(!app.ai_task.is_busy(), "nothing is sent");
    assert!(app.pending_recheck.is_none());
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Nothing to re-check"),
        "{:?}",
        app.message
    );
}

// --- Findings overlay: type-to-filter (#822) -------------------------------

/// Build a findings overlay over three findings whose summaries name the given
/// chapters, opened in a two-chapter project so Enter has a real jump target.
fn findings_over(name: &str, summaries: &[(&str, &str)]) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    let items = summaries
        .iter()
        .map(|(summary, chapter)| crate::mode::Finding {
            summary: (*summary).into(),
            chapters: vec![(*chapter).into()],
            book: None,
            action: None,
        })
        .collect();
    app.mode = Mode::Findings(Box::new(crate::mode::Findings {
        items,
        ..Default::default()
    }));
    (app, dir)
}

fn findings_state(app: &App) -> (usize, String, bool) {
    match &app.mode {
        Mode::Findings(f) => (f.selected, f.filter.clone(), f.typing),
        _ => panic!("expected the findings overlay"),
    }
}

#[test]
fn slash_filters_findings_by_summary_and_moves_within_matches() {
    let (mut app, _dir) = findings_over(
        "find-filter-move",
        &[
            ("locket color drift", "One"),
            ("timeline gap", "One"),
            ("locket ownership", "Two"),
        ],
    );

    app.on_key(key(KeyCode::Char('/')));
    assert!(findings_state(&app).2, "typing after /");
    type_str(&mut app, "locket"); // matches rows 0 and 2, not "timeline gap"
    assert_eq!(findings_state(&app).0, 0);

    // Down steps to the next matching finding (2), skipping the hidden row 1.
    app.on_key(key(KeyCode::Down));
    assert_eq!(findings_state(&app).0, 2);
    app.on_key(key(KeyCode::Down)); // no further match — stays
    assert_eq!(findings_state(&app).0, 2);
}

#[test]
fn dismissing_a_filtered_finding_drops_the_right_one() {
    let (mut app, _dir) = findings_over(
        "find-filter-dismiss",
        &[
            ("keep alpha", "One"),
            ("drop beta", "Two"),
            ("keep gamma", "One"),
        ],
    );

    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "beta");
    app.on_key(key(KeyCode::Enter)); // leave input, keep filter
    app.on_key(key(KeyCode::Char('d'))); // dismiss the filtered match

    // Only "drop beta" is gone; the two kept findings remain, filter still on.
    match &app.mode {
        Mode::Findings(f) => {
            let summaries: Vec<&str> = f.items.iter().map(|i| i.summary.as_str()).collect();
            assert_eq!(summaries, vec!["keep alpha", "keep gamma"]);
        }
        _ => panic!("the overlay stays open with matches remaining"),
    }
}

#[test]
fn findings_esc_clears_the_filter_then_closes() {
    let (mut app, _dir) = findings_over(
        "find-filter-esc",
        &[("alpha note", "One"), ("beta note", "Two")],
    );

    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "beta");
    app.on_key(key(KeyCode::Esc)); // leave input, keep filter
    assert_eq!(findings_state(&app).1, "beta");

    app.on_key(key(KeyCode::Esc)); // clear the filter, list restored
    assert_eq!(findings_state(&app).1, "");
    assert!(matches!(app.mode, Mode::Findings(_)));

    app.on_key(key(KeyCode::Esc)); // close
    assert!(!matches!(app.mode, Mode::Findings(_)));
}
