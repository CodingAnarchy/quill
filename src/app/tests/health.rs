//! The manuscript-health dashboard (#827): findings carry a kind, the dashboard
//! groups them by kind/pass and chapter with counts, cross-pass dedup uses the
//! kind, and jumping from the dashboard reaches the finding's chapter. Hermetic —
//! no network, no real AI (#531).

use super::*;

use crate::app::health::{consensus, dashboard_rows};
use crate::mode::{DashboardRow, Finding, FindingKind};

/// A UI finding of `kind` on `chapter` with the given summary — the shape a pass
/// maps its typed findings into.
fn finding(kind: FindingKind, chapter: &str, summary: &str) -> Finding {
    Finding {
        summary: summary.into(),
        kind,
        chapters: vec![chapter.into()],
        book: None,
        action: None,
        corroborators: Vec::new(),
    }
}

/// The kind headers of a built row list, in order, with their counts.
fn kind_headers(rows: &[DashboardRow]) -> Vec<(&'static str, usize)> {
    rows.iter()
        .filter_map(|r| match r {
            DashboardRow::Kind { label, count } => Some((*label, *count)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_pass_stamps_its_kind_onto_the_finding() {
    // A continuity pass's findings map to UI findings carrying `Continuity`, and
    // that kind survives into the standing triage list.
    let (mut app, _dir) = two_chapter_app("health-kind");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "the locket changes color".into(),
        chapters: vec!["One".into()],
    }])));
    assert_eq!(app.triage.len(), 1);
    assert_eq!(app.triage[0].kind, FindingKind::Continuity);
}

#[test]
fn dashboard_groups_by_kind_and_chapter_with_counts() {
    let findings = vec![
        finding(FindingKind::Continuity, "One", "object: locket"),
        finding(FindingKind::Continuity, "One", "object: knife"),
        finding(FindingKind::Continuity, "Two", "time: sunset twice"),
        finding(FindingKind::Style, "One", "pov: head-hop"),
    ];
    let rows = dashboard_rows(&findings);

    // Two kind groups, in declaration order (Continuity before Style), with the
    // right open counts.
    assert_eq!(
        kind_headers(&rows),
        vec![("Continuity", 3), ("Prose & style", 1)]
    );

    // The chapter sub-headers under Continuity carry their per-chapter counts.
    let chapters: Vec<(String, usize)> = rows
        .iter()
        .filter_map(|r| match r {
            DashboardRow::Chapter { title, count } => Some((title.clone(), *count)),
            DashboardRow::Kind { .. } | DashboardRow::Finding(_) => None,
        })
        .collect();
    assert_eq!(
        chapters,
        vec![
            ("One".to_string(), 2),
            ("Two".to_string(), 1),
            ("One".to_string(), 1),
        ]
    );

    // Every finding shows up as a jumpable row.
    let finding_rows = rows
        .iter()
        .filter(|r| matches!(r, DashboardRow::Finding(_)))
        .count();
    assert_eq!(finding_rows, 4);
}

#[test]
fn cross_pass_dedup_uses_the_kind() {
    // Two passes flag the same span with coincidentally identical text: because
    // the kinds differ, they stay distinct in triage (before #827 they'd collapse
    // to one).
    let (mut app, _dir) = two_chapter_app("health-dedup");
    let same_text = "One — flagged";
    app.accumulate_triage(&[
        finding(FindingKind::Continuity, "One", same_text),
        finding(FindingKind::Style, "One", same_text),
    ]);
    assert_eq!(app.triage.len(), 2, "different kinds don't dedup");

    // Re-accumulating the very same continuity finding is a no-op (same kind +
    // content), so dedup within a kind still holds.
    app.accumulate_triage(&[finding(FindingKind::Continuity, "One", same_text)]);
    assert_eq!(app.triage.len(), 2, "same kind + text dedups");
}

#[test]
fn a_dismissal_suppresses_the_same_span_from_a_later_pass() {
    // The author dismisses a dangling thread the continuity pass raised; a later
    // plot pass flags the identical span and must not re-surface it — across a
    // reload — because the dismissal is span-anchored (#921).
    let (mut app, dir) = two_chapter_app("health-cross-dismiss");
    let same = "a dangling thread";
    app.accumulate_triage(&[finding(FindingKind::Continuity, "One", same)]);
    app.open_triage();
    app.on_key(key(KeyCode::Char('d'))); // dismiss the only (continuity) finding
    assert!(app.triage.is_empty(), "dismissed leaves the session list");

    // A fresh session: the plot pass raises the same span, which stays suppressed.
    let mut reopened = open_test_app(&dir);
    reopened.accumulate_triage(&[finding(FindingKind::Plot, "One", same)]);
    assert!(
        reopened.triage.is_empty(),
        "another pass's finding on the dismissed span stays suppressed"
    );

    // But a different span the plot pass flags is unaffected.
    reopened.accumulate_triage(&[finding(FindingKind::Plot, "One", "a separate issue")]);
    assert_eq!(reopened.triage.len(), 1, "an unrelated span still surfaces");
    drop(dir);
}

#[test]
fn kind_persists_through_the_durable_store() {
    // A finding restored from disk keeps its kind even though the store drops the
    // re-runnable `action` handle.
    let (mut app, dir) = two_chapter_app("health-persist");
    app.accumulate_triage(&[finding(FindingKind::Style, "One", "pov: head-hop")]);

    let reopened = open_test_app(&dir);
    assert_eq!(reopened.triage.len(), 1);
    assert_eq!(reopened.triage[0].kind, FindingKind::Style);
}

#[test]
fn consensus_merges_a_span_two_passes_flag_and_weights_it_by_agreement() {
    // Continuity and plot independently flag the same dangling thread (identical
    // summary + chapter). Consensus collapses them to one finding weighted by the
    // agreement, keeping the first pass as the primary lens (#921).
    let same = "One — a dangling thread";
    let merged = consensus(&[
        finding(FindingKind::Continuity, "One", same),
        finding(FindingKind::Plot, "One", same),
    ]);
    assert_eq!(merged.len(), 1, "the shared span collapses to one finding");
    assert_eq!(
        merged[0].kind,
        FindingKind::Continuity,
        "primary is first-seen"
    );
    assert_eq!(
        merged[0].agreement(),
        vec![FindingKind::Continuity, FindingKind::Plot],
        "both passes are recorded as agreeing"
    );
    assert_eq!(
        merged[0].agreement_signal().as_deref(),
        Some("flagged by continuity + plot & arc"),
        "the consensus signal names both passes"
    );

    // A lone finding carries no signal.
    let lone = consensus(&[finding(FindingKind::Style, "One", "pov: head-hop")]);
    assert_eq!(lone.len(), 1);
    assert_eq!(lone[0].agreement_signal(), None, "one pass → no consensus");
}

#[test]
fn consensus_keeps_distinct_spans_apart() {
    // Two passes flag *different* spans (different summaries): no merge, each stays
    // its own finding with no cross-pass signal.
    let merged = consensus(&[
        finding(FindingKind::Continuity, "One", "object: locket"),
        finding(FindingKind::Plot, "One", "thread: the letter"),
    ]);
    assert_eq!(merged.len(), 2, "distinct spans don't merge");
    assert!(merged.iter().all(|f| f.agreement_signal().is_none()));
}

#[test]
fn opening_the_dashboard_selects_the_first_finding_row() {
    let (mut app, _dir) = two_chapter_app("health-open");
    app.accumulate_triage(&[finding(FindingKind::Continuity, "One", "object: locket")]);
    app.open_health_dashboard();

    let Mode::Dashboard(d) = &app.mode else {
        panic!("dashboard should open, got a different mode");
    };
    // The selection lands on a finding row, not a header.
    assert!(d.finding_at(d.selected).is_some());
    assert_eq!(d.total, 1);
}

#[test]
fn dashboard_with_no_findings_reports_instead_of_opening() {
    let (mut app, _dir) = two_chapter_app("health-empty");
    app.open_health_dashboard();
    assert!(!matches!(app.mode, Mode::Dashboard(_)));
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("No findings yet"));
}

#[test]
fn jumping_from_the_dashboard_reaches_the_findings_chapter() {
    // The finding targets chapter "Two"; Enter on it activates that chapter and
    // leaves the dashboard.
    let (mut app, _dir) = two_chapter_app("health-jump");
    assert_eq!(app.project.active_index(), 0, "opens on chapter one");
    app.accumulate_triage(&[finding(
        FindingKind::Continuity,
        "Two",
        "time: sunset twice",
    )]);
    app.open_health_dashboard();

    app.on_key_dashboard(key(KeyCode::Enter));
    assert!(!matches!(app.mode, Mode::Dashboard(_)), "dashboard closes");
    assert_eq!(
        app.project.active_index(),
        1,
        "jumped to the finding's chapter"
    );
}

#[test]
fn arrow_keys_skip_header_rows() {
    // Down from the last finding of one kind lands on the first finding of the
    // next kind, stepping over the intervening kind/chapter headers.
    let (mut app, _dir) = two_chapter_app("health-nav");
    app.accumulate_triage(&[
        finding(FindingKind::Continuity, "One", "object: locket"),
        finding(FindingKind::Style, "One", "pov: head-hop"),
    ]);
    app.open_health_dashboard();

    let first = match &app.mode {
        Mode::Dashboard(d) => d.selected,
        _ => panic!("dashboard"),
    };
    app.on_key_dashboard(key(KeyCode::Down));
    let Mode::Dashboard(d) = &app.mode else {
        panic!("dashboard");
    };
    assert_ne!(d.selected, first, "moved to the next finding row");
    // The new selection is a finding row (a header would return `None`).
    assert!(d.finding_at(d.selected).is_some());
    // And it's the Style finding — the group after Continuity.
    assert_eq!(
        d.finding_at(d.selected).map(|f| f.kind),
        Some(FindingKind::Style)
    );
}
