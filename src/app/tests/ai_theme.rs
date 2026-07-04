//! AI theme / motif tracker (#828): writing the tracker, dangling-motif findings
//! through triage, scoped re-check of a motif finding, and the incremental
//! bible-key round-trip (#563).

use super::*;
use crate::ai::theme::{Motif, MotifFinding, Theme as ThemeBible};
use crate::mode::AiAction;
use std::fs;

// --- Theme / motif tracker (#828) ------------------------------------------

/// A theme result with the given dangling-motif findings and one always-present
/// motif, so the tracker file always has content to write.
fn theme_with(findings: Vec<MotifFinding>) -> ThemeBible {
    ThemeBible {
        motifs: vec![Motif {
            name: "Mirrors".into(),
            status: "recurring".into(),
            meaning: "self-deception".into(),
            appearances: vec!["One".into()],
            notes: String::new(),
        }],
        findings,
    }
}

#[test]
fn theme_writes_tracker_and_opens_dangling_findings() {
    let (mut app, dir) = two_chapter_app("theme-write");
    app.apply_theme(Ok(theme_with(vec![MotifFinding {
        motif: "The recurring storm".into(),
        issue: "introduced in ch. 1, never returns".into(),
        chapters: vec!["One".into()],
    }])));
    let file = fs::read_to_string(dir.join("bible/theme.md")).expect("theme tracker written");
    assert!(file.contains("title: Themes & motifs"));
    assert!(file.contains("### Mirrors — recurring"));
    assert!(file.contains("## Dangling motifs"));
    assert!(
        matches!(app.mode, Mode::Findings(_)),
        "dangling-motif findings overlay opens"
    );
}

#[test]
fn theme_with_no_dangling_motifs_summarizes() {
    let (mut app, dir) = two_chapter_app("theme-clean");
    app.apply_theme(Ok(theme_with(vec![])));
    assert!(dir.join("bible/theme.md").exists());
    assert!(
        app.overlay.is_some(),
        "summary overlay when nothing dangles"
    );
    assert!(!matches!(app.mode, Mode::Findings(_)));
}

#[test]
fn theme_findings_flow_through_triage_deduped() {
    let (mut app, _dir) = two_chapter_app("theme-triage");
    let finding = MotifFinding {
        motif: "The recurring storm".into(),
        issue: "opened in ch. 3, never returns".into(),
        chapters: vec!["Two".into()],
    };
    app.apply_theme(Ok(theme_with(vec![finding.clone()])));
    // A re-run with the same dangling finding must not double-count in triage.
    app.apply_theme(Ok(theme_with(vec![finding])));
    app.open_triage();
    match &app.mode {
        Mode::Findings(f) => assert_eq!(f.items.len(), 1, "one dangling finding, deduped"),
        _ => panic!("expected the triage findings overlay"),
    }
}

/// A theme UI finding on `chapter`, tagged to the theme pass so it can be
/// re-checked. Mirrors how `apply_theme` builds a finding's summary
/// (`motif: issue`), so a re-run that re-parses to the same content matches it.
fn theme_ui_finding(chapter: &str, motif: &str, issue: &str) -> crate::mode::Finding {
    crate::mode::Finding {
        summary: format!("{motif}: {issue}"),
        chapters: vec![chapter.into()],
        book: None,
        action: Some(AiAction::Theme),
    }
}

/// A theme result (as JSON) carrying one dangling-motif finding of `issue` on
/// `chapter` — the JSON a scoped re-check parses, wrapped through the pass's own
/// parser so its summary exactly matches `theme_ui_finding`.
fn theme_json(chapter: &str, motif: &str, issue: &str) -> String {
    format!(
        r#"{{"motifs":[],"findings":[{{"motif":"{motif}","issue":"{issue}","chapters":["{chapter}"]}}]}}"#
    )
}

#[test]
fn recheck_of_a_theme_finding_is_scoped_to_its_chapters() {
    let (app, _dir) = two_chapter_app("theme-recheck-scope");
    // A dangling-motif finding on chapter Two only. Its re-check request must carry
    // Two's prose and not One's — a scoped theme re-run.
    let finding = theme_ui_finding("Two", "The recurring storm", "never returns");
    let req = app
        .recheck_request_for(&finding)
        .expect("a theme finding builds a re-check request");
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
        "uses the theme revise system prompt"
    );
}

#[test]
fn recheck_of_a_theme_finding_that_still_flags_keeps_it() {
    let (mut app, _dir) = two_chapter_app("theme-recheck-keep");
    // Surface a dangling-motif finding into the overlay + triage the normal way.
    app.apply_theme(Ok(theme_with(vec![MotifFinding {
        motif: "The recurring storm".into(),
        issue: "never returns".into(),
        chapters: vec!["One".into()],
    }])));
    assert!(matches!(app.mode, Mode::Findings(_)));
    assert_eq!(app.triage.len(), 1);
    let finding = theme_ui_finding("One", "The recurring storm", "never returns");

    // The scoped re-run re-produces the same finding: the motif still dangles.
    let fresh = app.recheck_findings_from(
        &finding,
        &theme_json("One", "The recurring storm", "never returns"),
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
fn recheck_of_a_theme_finding_that_clears_drops_it() {
    let (mut app, _dir) = two_chapter_app("theme-recheck-clear");
    app.apply_theme(Ok(theme_with(vec![MotifFinding {
        motif: "The recurring storm".into(),
        issue: "never returns".into(),
        chapters: vec!["One".into()],
    }])));
    assert_eq!(app.triage.len(), 1);
    let finding = theme_ui_finding("One", "The recurring storm", "never returns");

    // The scoped re-run reports the motif as now paid off (no dangling findings):
    // the fix held. A findings-empty theme reply parses to no UI findings.
    let fresh = app.recheck_findings_from(
        &finding,
        r#"{"motifs":[{"name":"The recurring storm","status":"resolved","meaning":"m","appearances":["One","Two"],"notes":""}],"findings":[]}"#,
    );
    assert!(
        fresh.is_empty(),
        "a resolved motif yields no dangling finding"
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
fn theme_pass_scopes_incrementally_and_round_trips_its_bible_key() {
    let (mut app, dir) = two_chapter_app("theme-incr");
    // The tracker's bible key is the single source shared by the state file, the
    // incremental cache, and the series rollup (#563).
    let key = AiAction::Theme.bible_key().expect("theme is a bible pass");
    assert_eq!(key, "theme");
    // Seed a tracker file + recorded state so the up-to-date / incremental paths
    // are reachable, then edit one chapter and confirm only it is in the delta.
    fs::create_dir_all(dir.join("bible")).unwrap();
    fs::write(
        dir.join("bible/theme.md"),
        "---\ntitle: Themes & motifs\n---\n\n<!-- generated:start -->\nmotifs\n<!-- generated:end -->\n",
    )
    .unwrap();
    ai::incremental::State::record(&dir, key, "2026-06-21", app.chapter_digests()).unwrap();
    // Nothing changed → the pass is up to date and sends nothing.
    assert_eq!(
        app.pass_context_len(AiAction::Theme, false),
        0,
        "up to date"
    );

    app.on_key(ctrl('n')); // active → Two
    type_str(&mut app, "NEWWORD");
    app.on_key(ctrl('s'));
    let state = ai::incremental::State::load(&dir);
    let scope = ai::incremental::decide(state.built_from(key), &app.chapter_digests(), 5);
    assert_eq!(
        scope,
        ai::incremental::Scope::Incremental(vec!["two.md".into()]),
        "only the edited chapter is in the theme delta"
    );
}
