//! AI plot-thread / arc tracker (#606): writing the tracker, dangling-thread
//! findings through triage, scoped re-check of a plot finding (#640), and the
//! incremental bible-key round-trip (#563).

use super::*;
use crate::mode::AiAction;
use std::fs;

// --- Plot-thread / arc tracker (#606) --------------------------------------

/// A plot result with the given dangling-thread findings and one always-present
/// thread, so the tracker file always has content to write.
fn plot_with(findings: Vec<plot::ThreadFinding>) -> Plot {
    Plot {
        threads: vec![plot::Thread {
            name: "The stolen locket".into(),
            status: plot::ThreadStatus::Open,
            setup: "Mara pockets it in ch. 1".into(),
            payoff: String::new(),
            last_seen: "One".into(),
            notes: String::new(),
        }],
        findings,
    }
}

#[test]
fn plot_writes_tracker_and_opens_dangling_findings() {
    let (mut app, dir) = two_chapter_app("plot-write");
    app.apply_plot(Ok(plot_with(vec![plot::ThreadFinding {
        thread: "The stolen locket".into(),
        issue: "introduced in ch. 1, never resolved".into(),
        chapters: vec!["One".into()],
    }])));
    let file = fs::read_to_string(dir.join("bible/plot.md")).expect("plot tracker written");
    assert!(file.contains("title: Plot threads"));
    assert!(file.contains("### The stolen locket — open"));
    assert!(file.contains("## Dangling threads"));
    assert!(
        matches!(app.mode, Mode::Findings(_)),
        "dangling-thread findings overlay opens"
    );
}

#[test]
fn plot_with_no_dangling_threads_summarizes() {
    let (mut app, dir) = two_chapter_app("plot-clean");
    app.apply_plot(Ok(plot_with(vec![])));
    assert!(dir.join("bible/plot.md").exists());
    assert!(
        app.overlay.is_some(),
        "summary overlay when nothing dangles"
    );
    assert!(!matches!(app.mode, Mode::Findings(_)));
}

#[test]
fn plot_findings_flow_through_triage_deduped() {
    let (mut app, _dir) = two_chapter_app("plot-triage");
    let finding = plot::ThreadFinding {
        thread: "Who burned the archive".into(),
        issue: "opened in ch. 3, never answered".into(),
        chapters: vec!["Two".into()],
    };
    app.apply_plot(Ok(plot_with(vec![finding.clone()])));
    // A re-run with the same dangling finding must not double-count in triage.
    app.apply_plot(Ok(plot_with(vec![finding])));
    app.open_triage();
    match &app.mode {
        Mode::Findings(f) => assert_eq!(f.items.len(), 1, "one dangling finding, deduped"),
        _ => panic!("expected the triage findings overlay"),
    }
}

/// A plot UI finding on `chapter`, tagged to the plot pass so it can be
/// re-checked. Mirrors how `apply_plot` builds a finding's summary
/// (`thread: issue`), so a re-run that re-parses to the same content matches it.
fn plot_ui_finding(chapter: &str, thread: &str, issue: &str) -> crate::mode::Finding {
    crate::mode::Finding {
        summary: format!("{thread}: {issue}"),
        kind: crate::mode::FindingKind::Plot,
        chapters: vec![chapter.into()],
        book: None,
        action: Some(AiAction::Plot),
        corroborators: Vec::new(),
    }
}

/// A plot result (as JSON) carrying one dangling-thread finding of `issue` on
/// `chapter` — the JSON a scoped re-check parses, wrapped through the pass's own
/// parser so its summary exactly matches `plot_ui_finding`.
fn plot_json(chapter: &str, thread: &str, issue: &str) -> String {
    format!(
        r#"{{"threads":[],"findings":[{{"thread":"{thread}","issue":"{issue}","chapters":["{chapter}"]}}]}}"#
    )
}

#[test]
fn recheck_of_a_plot_finding_is_scoped_to_its_chapters() {
    let (app, _dir) = two_chapter_app("plot-recheck-scope");
    // A dangling-thread finding on chapter Two only. Its re-check request must
    // carry Two's prose and not One's — a scoped plot re-run (#640).
    let finding = plot_ui_finding("Two", "The stolen locket", "never resolved");
    let req = app
        .recheck_request_for(&finding)
        .expect("a plot finding builds a re-check request");
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
    // Revises from the existing tracker (the incremental path), like the other
    // bible passes' re-checks.
    assert!(
        req.system.as_deref().unwrap_or("").contains("dangling"),
        "uses the plot revise system prompt"
    );
}

#[test]
fn recheck_of_a_plot_finding_that_still_flags_keeps_it() {
    let (mut app, _dir) = two_chapter_app("plot-recheck-keep");
    // Surface a dangling-thread finding into the overlay + triage the normal way.
    app.apply_plot(Ok(plot_with(vec![plot::ThreadFinding {
        thread: "The stolen locket".into(),
        issue: "never resolved".into(),
        chapters: vec!["One".into()],
    }])));
    assert!(matches!(app.mode, Mode::Findings(_)));
    assert_eq!(app.triage.len(), 1);
    let finding = plot_ui_finding("One", "The stolen locket", "never resolved");

    // The scoped re-run re-produces the same finding: the thread still dangles.
    let fresh = app.recheck_findings_from(
        &finding,
        &plot_json("One", "The stolen locket", "never resolved"),
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
fn recheck_of_a_plot_finding_that_clears_drops_it() {
    let (mut app, _dir) = two_chapter_app("plot-recheck-clear");
    app.apply_plot(Ok(plot_with(vec![plot::ThreadFinding {
        thread: "The stolen locket".into(),
        issue: "never resolved".into(),
        chapters: vec!["One".into()],
    }])));
    assert_eq!(app.triage.len(), 1);
    let finding = plot_ui_finding("One", "The stolen locket", "never resolved");

    // The scoped re-run reports the thread as now resolved (no dangling findings):
    // the fix held. A findings-empty plot reply parses to no UI findings.
    let fresh = app.recheck_findings_from(
        &finding,
        r#"{"threads":[{"name":"The stolen locket","status":"resolved","setup":"s","payoff":"returned","last_seen":"One","notes":""}],"findings":[]}"#,
    );
    assert!(
        fresh.is_empty(),
        "a resolved thread yields no dangling finding"
    );
    app.pending_recheck = Some(finding.clone());
    app.apply_recheck(Ok(fresh));

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
fn plot_pass_scopes_incrementally_and_round_trips_its_bible_key() {
    let (mut app, dir) = two_chapter_app("plot-incr");
    // The tracker's bible key is the single source shared by the state file, the
    // incremental cache, and the series rollup (#563).
    let key = AiAction::Plot.bible_key().expect("plot is a bible pass");
    assert_eq!(key, "plot");
    // Seed a tracker file + recorded state so the up-to-date / incremental paths
    // are reachable, then edit one chapter and confirm only it is in the delta.
    fs::create_dir_all(dir.join("bible")).unwrap();
    fs::write(
        dir.join("bible/plot.md"),
        "---\ntitle: Plot threads\n---\n\n<!-- generated:start -->\nthreads\n<!-- generated:end -->\n",
    )
    .unwrap();
    ai::incremental::State::record(&dir, key, "2026-06-21", app.chapter_digests()).unwrap();
    // Nothing changed → the pass is up to date and sends nothing.
    assert_eq!(app.pass_context_len(AiAction::Plot, false), 0, "up to date");

    app.on_key(ctrl('n')); // active → Two
    type_str(&mut app, "NEWWORD");
    app.on_key(ctrl('s'));
    let state = ai::incremental::State::load(&dir);
    let scope = ai::incremental::decide(state.built_from(key), &app.chapter_digests(), 5);
    assert_eq!(
        scope,
        ai::incremental::Scope::Incremental(vec!["two.md".into()]),
        "only the edited chapter is in the plot delta"
    );
}
