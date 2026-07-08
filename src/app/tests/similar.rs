//! Semantic find (#924) and echo / near-duplicate detection (#920): the app-side
//! integration — driving the `apply_*` handlers with injected results (no embedder
//! / no model download, hermetic #531) to prove the findings, triage/dashboard
//! wiring, the off-switch, and graceful degradation.

use super::*;
use crate::ai::embed::{EchoPair, Hit, Passage};
use crate::mode::{DashboardRow, FindingKind};

fn passage(title: &str, text: &str) -> Passage {
    Passage {
        title: title.into(),
        file: format!("{}.md", title.to_lowercase()),
        text: text.into(),
    }
}

#[test]
fn semantic_find_opens_a_ranked_jumpable_list_without_touching_triage() {
    let (mut app, _dir) = two_chapter_app("find-similar");
    let hits = vec![
        Hit {
            passage: passage("Two", "the storm returned to the bay"),
            score: 0.91,
        },
        Hit {
            passage: passage("One", "a distant, quieter squall"),
            score: 0.4,
        },
    ];
    app.apply_semantic_find(Ok(Some(hits)));

    // A jumpable findings list opens, one row per hit, highest score first.
    match &app.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items.len(), 2, "one row per similar passage");
            assert!(
                f.items[0].summary.contains("91%") && f.items[0].summary.contains("Two"),
                "the top hit shows its score + citation: {}",
                f.items[0].summary
            );
            // The row targets the passage's chapter, so Enter jumps there.
            assert_eq!(f.items[0].chapters, vec!["Two".to_string()]);
        }
        _ => panic!("semantic find should open a findings list"),
    }
    // Retrieval for this query — not standing revision findings — so nothing enters
    // durable triage (like a Q&A citation).
    assert!(app.triage.is_empty(), "semantic find never writes triage");
}

#[test]
fn semantic_find_with_no_hits_reports_cleanly() {
    let (mut app, _dir) = two_chapter_app("find-empty");
    app.apply_semantic_find(Ok(Some(vec![])));
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(app.message.as_deref().unwrap().contains("No similar"));
}

#[test]
fn semantic_find_without_a_model_degrades_gracefully() {
    // `None` from the worker means no embedding model on disk — a clear message,
    // no crash, no findings.
    let (mut app, _dir) = two_chapter_app("find-nomodel");
    app.apply_semantic_find(Ok(None));
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(
        app.message.as_deref().unwrap().contains("embedding model"),
        "explains the model is needed: {:?}",
        app.message
    );
}

#[test]
fn echo_pairs_become_triage_findings_grouped_in_the_dashboard() {
    let (mut app, _dir) = two_chapter_app("echo-triage");
    let pairs = vec![
        EchoPair {
            a: passage("One", "the lighthouse blinked twice"),
            b: passage("Two", "the lighthouse blinked twice again"),
            score: 0.95, // near-duplicate
        },
        EchoPair {
            a: passage("One", "a cold wind off the water"),
            b: passage("Two", "the chill breeze from the sea"),
            score: 0.8, // soft echo
        },
    ];
    app.apply_echo(Ok(Some(pairs)));

    // Both pairs surface as findings, and they accumulate into durable triage.
    assert_eq!(app.triage.len(), 2, "both echo pairs reach triage");
    assert!(
        app.triage.iter().all(|f| f.kind == FindingKind::Echo),
        "echo findings carry the Echo kind"
    );
    // The near-duplicate is labelled as such and names both citations + a score.
    let near = app
        .triage
        .iter()
        .find(|f| f.summary.contains("near-duplicate"))
        .expect("the 0.95 pair is a near-duplicate");
    assert!(
        near.summary.contains("95%"),
        "score shown: {}",
        near.summary
    );
    assert_eq!(near.chapters, vec!["One".to_string(), "Two".to_string()]);

    // The dashboard groups them under the Echo header.
    app.mode = Mode::Insert;
    app.open_health_dashboard();
    match &app.mode {
        Mode::Dashboard(d) => {
            assert!(
                d.rows.iter().any(|r| matches!(
                    r,
                    DashboardRow::Kind { label, count }
                        if *label == FindingKind::Echo.label() && *count == 2
                )),
                "the dashboard has an Echo group with both findings"
            );
        }
        _ => panic!("dashboard should open with the echo findings"),
    }
}

#[test]
fn echo_detection_respects_the_off_switch() {
    let (mut app, _dir) = two_chapter_app("echo-off");
    app.ai.enabled = true;
    app.ai.echo_detection = false;
    app.start_echo_detection();
    // Nothing runs; a message explains how to turn it on, and no task is in flight.
    assert!(
        app.message.as_deref().unwrap().contains("off"),
        "the off-switch is reported: {:?}",
        app.message
    );
    assert!(
        !app.ai_busy(),
        "no scan is started when echo detection is off"
    );
    assert!(app.triage.is_empty());
}

#[test]
fn echo_scan_with_no_pairs_reports_a_clean_bill() {
    let (mut app, _dir) = two_chapter_app("echo-clean");
    app.apply_echo(Ok(Some(vec![])));
    assert!(app.triage.is_empty());
    assert!(app.message.as_deref().unwrap().contains("No echoes"));
}

#[test]
fn echo_scan_without_a_model_degrades_gracefully() {
    let (mut app, _dir) = two_chapter_app("echo-nomodel");
    app.apply_echo(Ok(None));
    assert!(app.triage.is_empty());
    assert!(
        app.message.as_deref().unwrap().contains("embedding model"),
        "explains the model is needed: {:?}",
        app.message
    );
}

#[test]
fn a_dismissed_echo_stays_gone_on_a_re_scan() {
    // "Mark intentional reprise": dismissing an echo finding keeps a later re-scan
    // from re-surfacing it, via the durable findings store.
    let (mut app, _dir) = two_chapter_app("echo-reprise");
    let pair = EchoPair {
        a: passage("One", "the refrain of the tide"),
        b: passage("Two", "the tide's refrain once more"),
        score: 0.93,
    };
    app.apply_echo(Ok(Some(vec![pair.clone()])));
    assert_eq!(app.triage.len(), 1);

    // Dismiss it (the author declares it a deliberate reprise).
    let finding = app.triage[0].clone();
    app.findings_store.dismiss(&app.project.root, &finding);
    app.triage.clear();

    // A re-scan producing the same pair must not re-surface the dismissed echo.
    app.apply_echo(Ok(Some(vec![pair])));
    assert!(
        app.triage.is_empty(),
        "a dismissed reprise isn't nagged again"
    );
}
