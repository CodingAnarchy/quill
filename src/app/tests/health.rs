//! The manuscript-health dashboard (#827): findings carry a kind, the dashboard
//! groups them by kind/pass and chapter with counts, cross-pass dedup uses the
//! kind, and jumping from the dashboard reaches the finding's chapter. Hermetic —
//! no network, no real AI (#531).

use super::*;

use crate::app::health::{consensus, dashboard_rows, revision_plan_text};
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

/// Regression for the KIND_ORDER drift (#1069): before this refactor the dashboard's
/// kind-order list was hand-maintained and had silently dropped Accuracy,
/// Relationship, Foreshadow, FactCheck, and Pacing — so a finding of any of those
/// kinds vanished from the dashboard, the health trend, and the revision plan even
/// though it was in triage. Deriving the order from `FindingKind::ALL` fixes it.
/// This asserts every finding kind that maps to a real pass surfaces in BOTH the
/// dashboard rows and the revision plan, so the class of bug can't recur.
#[test]
fn every_pass_kind_reaches_the_dashboard_and_the_revision_plan() {
    // One finding of each pass kind (skip `Other` — it's the non-pass catch-all, not
    // a pass finding). The five that used to be dropped are the point of the test.
    let pass_kinds: Vec<FindingKind> = FindingKind::ALL
        .iter()
        .copied()
        .filter(|k| *k != FindingKind::Other)
        .collect();
    let findings: Vec<Finding> = pass_kinds
        .iter()
        .enumerate()
        .map(|(i, &k)| finding(k, "One", &format!("{}: issue {i}", k.label())))
        .collect();

    // The dashboard renders a kind header for every pass kind — none dropped.
    let rows = dashboard_rows(&findings);
    let headers = kind_headers(&rows);
    for &k in &pass_kinds {
        assert!(
            headers
                .iter()
                .any(|(label, count)| *label == k.label() && *count == 1),
            "dashboard dropped {} — KIND_ORDER drift (#1069)",
            k.label()
        );
    }
    // Specifically the five the old hand-list omitted.
    for k in [
        FindingKind::Accuracy,
        FindingKind::Relationship,
        FindingKind::Foreshadow,
        FindingKind::FactCheck,
        FindingKind::Pacing,
    ] {
        assert!(
            headers.iter().any(|(label, _)| *label == k.label()),
            "previously-dropped kind missing from dashboard: {}",
            k.label()
        );
    }

    // The revision plan lists a checklist line labelled with every pass kind too — it
    // reads from the same open findings, so a dropped kind would silently vanish there.
    let plan = revision_plan_text(&findings);
    for &k in &pass_kinds {
        assert!(
            plan.contains(&format!("[{}]", k.label())),
            "revision plan dropped {} — KIND_ORDER drift (#1069)",
            k.label()
        );
    }
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
    // agreement, with the primary lens the highest-precedence pass by KIND_ORDER
    // (#921/#1119) — continuity outranks plot, so it's primary here.
    let same = "One — a dangling thread";
    let merged = consensus(&[
        finding(FindingKind::Continuity, "One", same),
        finding(FindingKind::Plot, "One", same),
    ]);
    assert_eq!(merged.len(), 1, "the shared span collapses to one finding");
    assert_eq!(
        merged[0].kind,
        FindingKind::Continuity,
        "primary is the highest-precedence pass by KIND_ORDER (continuity < plot)"
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
fn consensus_attribution_is_stable_regardless_of_pass_order() {
    // Per-kind attribution must not depend on which pass ran first (#1119). The same
    // cross-pass span is supplied to consensus in both pass orders; the primary kind
    // (the one the health-trend tallies under) is identical either way — the
    // highest-precedence pass by KIND_ORDER, not first-seen. Plot outranks Voice in
    // KIND_ORDER, so plot is primary whichever order the passes were appended in.
    let same = "One — a sagging middle";
    let plot_then_voice = consensus(&[
        finding(FindingKind::Voice, "One", same),
        finding(FindingKind::Plot, "One", same),
    ]);
    let voice_then_plot = consensus(&[
        finding(FindingKind::Plot, "One", same),
        finding(FindingKind::Voice, "One", same),
    ]);

    assert_eq!(plot_then_voice.len(), 1);
    assert_eq!(voice_then_plot.len(), 1);
    assert_eq!(
        plot_then_voice[0].kind, voice_then_plot[0].kind,
        "the primary kind is the same whatever order the passes ran"
    );
    assert_eq!(
        plot_then_voice[0].kind,
        FindingKind::Plot,
        "the primary is the highest-precedence pass by KIND_ORDER (plot < voice)"
    );
    // The full agreement set matches too, so the displayed consensus signal is stable.
    assert_eq!(
        plot_then_voice[0].agreement(),
        voice_then_plot[0].agreement()
    );
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

// --- Health trend across drafts (#1052) ------------------------------------

/// The trend line the health snapshot leads with — read back from the saved
/// `export/health-<date>.md`, the `_Trend …_` line. `None` when no trend was
/// written (the first recorded run has no prior to compare against).
fn saved_trend_line(dir: &std::path::Path) -> Option<String> {
    let name = format!("health-{}.md", crate::date::iso(crate::date::today()));
    let body = fs::read_to_string(dir.join("export").join(&name)).ok()?;
    body.lines()
        .find(|l| l.starts_with("_Trend "))
        .map(str::to_string)
}

#[test]
fn the_first_recorded_run_has_nothing_to_trend_against() {
    // A single recorded run seeds the side-file but has no prior run to compare
    // against, so its snapshot carries no trend line.
    let (mut app, dir) = two_chapter_app("health-trend-first");
    app.accumulate_triage(&[finding(FindingKind::Continuity, "One", "object: locket")]);
    app.export_health_trend();
    assert!(
        saved_trend_line(&dir).is_none(),
        "the first recorded run has nothing to trend against"
    );
    // But the run *was* recorded: the side-file exists with today's tally.
    let recorded = fs::read_to_string(dir.join("bible").join("health-trend.toml"))
        .expect("the run is persisted to the side-file");
    assert!(recorded.contains("continuity = 1"), "{recorded}");
    drop(dir);
}

#[test]
fn a_run_records_counts_and_the_next_run_shows_the_change() {
    // A prior draft's run is on file (an earlier date, as it would be across
    // drafts); today's run mutates the standing findings — a continuity finding
    // cleared (down), a style finding added (up) — and the trend it writes reflects
    // that delta by kind, computed from the small persisted tallies, never a
    // manuscript re-scan.
    let (mut app, dir) = two_chapter_app("health-trend-change");

    // Seed the previous draft's recorded run: two continuity findings, an earlier
    // date, so today's run has a genuine cross-draft baseline to compare against.
    fs::create_dir_all(dir.join("bible")).unwrap();
    fs::write(
        dir.join("bible").join("health-trend.toml"),
        "[[runs]]\ndate = \"2000-01-01\"\n[runs.counts]\ncontinuity = 2\n",
    )
    .unwrap();

    // Today: one continuity finding remains, and a style one has appeared.
    app.accumulate_triage(&[
        finding(FindingKind::Continuity, "Two", "time: sunset twice"),
        finding(FindingKind::Style, "One", "pov: head-hop"),
    ]);
    app.export_health_trend();

    let trend = saved_trend_line(&dir).expect("the second run writes a trend line");
    assert!(
        trend.contains("since 2000-01-01"),
        "the trend names the prior run: {trend}"
    );
    assert!(
        trend.contains("Continuity down 1"),
        "the cleared continuity finding shows as a fall: {trend}"
    );
    assert!(
        trend.contains("Prose & style up 1"),
        "the new style finding shows as a rise: {trend}"
    );
    // Net: -1 continuity + 1 style = no net change in total open findings.
    assert!(
        trend.contains("no net change"),
        "net total is flat: {trend}"
    );
    drop(dir);
}

#[test]
fn opening_the_dashboard_surfaces_the_trend_since_a_prior_run() {
    // A run recorded on a previous open, then a second open after the counts fall:
    // the dashboard opens and its status message is the trend since the prior run,
    // so the point-in-time view reads as a trend.
    let (mut app, dir) = two_chapter_app("health-trend-dashboard");
    app.accumulate_triage(&[
        finding(FindingKind::Continuity, "One", "object: locket"),
        finding(FindingKind::Continuity, "Two", "time: sunset twice"),
    ]);
    // Seed a prior run under an earlier date directly in the side-file, so the
    // second open (today) has a genuine baseline to compare against without needing
    // to advance the clock.
    let trend_file = dir.join("bible").join("health-trend.toml");
    fs::create_dir_all(dir.join("bible")).unwrap();
    fs::write(
        &trend_file,
        "[[runs]]\ndate = \"2000-01-01\"\n[runs.counts]\ncontinuity = 5\n",
    )
    .unwrap();

    app.open_health_dashboard();
    assert!(matches!(app.mode, Mode::Dashboard(_)), "dashboard opened");
    let msg = app.message.as_deref().unwrap_or("");
    assert!(
        msg.contains("since 2000-01-01"),
        "the trend names the prior run's date: {msg}"
    );
    assert!(
        msg.contains("Continuity down 3"),
        "5 → 2 continuity findings reads as a fall of 3: {msg}"
    );
    assert!(msg.contains("fewer findings"), "the net total fell: {msg}");
    drop(dir);
}

// --- Revision-plan export (#1055) ------------------------------------------

#[test]
fn revision_plan_is_chapter_ordered_and_confidence_ranked() {
    // Two chapters, each with findings of differing cross-pass agreement. The plan
    // groups by chapter in first-seen order, and within a chapter ranks the
    // higher-confidence (more passes agreeing) finding first.
    let consensus_span = "One — a dangling thread";
    let findings = consensus(&[
        finding(FindingKind::Style, "One", "pov: head-hop"), // lone (confidence 1)
        finding(FindingKind::Continuity, "One", consensus_span), // corroborated below
        finding(FindingKind::Plot, "One", consensus_span),   // → confidence 2
        finding(FindingKind::Setting, "Two", "the manor moved"),
    ]);
    let plan = revision_plan_text(&findings);

    // Chapter One's section comes before Chapter Two's (first-seen order).
    let one = plan.find("## One").expect("chapter One section");
    let two = plan.find("## Two").expect("chapter Two section");
    assert!(one < two, "chapters are in reading order:\n{plan}");

    // Within Chapter One, the two-pass consensus finding outranks the lone one.
    let consensus_at = plan.find(consensus_span).expect("consensus finding listed");
    let lone_at = plan.find("pov: head-hop").expect("lone finding listed");
    assert!(
        consensus_at < lone_at,
        "the higher-confidence finding is ranked first:\n{plan}"
    );

    // Confidence is shown as stars: the consensus finding carries two, the lone one.
    assert!(
        plan.contains("★★ [Continuity]"),
        "the consensus finding shows two-star confidence:\n{plan}"
    );
    // The plan is a to-do (checkboxes) and folds in the agreement signal.
    assert!(plan.contains("- [ ] "), "entries are checkboxes:\n{plan}");
    assert!(
        plan.contains("flagged by continuity + plot"),
        "cross-pass agreement is folded in:\n{plan}"
    );
}

#[test]
fn empty_findings_yield_a_nothing_to_plan_document() {
    let plan = revision_plan_text(&[]);
    assert!(plan.contains("# Revision plan"), "titled: {plan}");
    assert!(
        plan.contains("Nothing to plan"),
        "empty findings give a clean nothing-to-plan state: {plan}"
    );
}

#[test]
fn revision_plan_export_writes_only_reformatted_findings_no_prose() {
    // The export writes to `export/revision-plan-<date>.md` and its content is drawn
    // entirely from the findings' own fields — summaries, kinds, chapters — never
    // any manuscript prose (the chapters' bodies are "first chapter" / "second
    // chapter"; neither may leak into the plan).
    let (mut app, dir) = two_chapter_app("revision-plan-export");
    app.accumulate_triage(&[
        finding(FindingKind::Continuity, "One", "object: locket"),
        finding(FindingKind::Style, "Two", "pov: head-hop"),
    ]);
    app.export_revision_plan();

    let name = format!(
        "revision-plan-{}.md",
        crate::date::iso(crate::date::today())
    );
    let body = fs::read_to_string(dir.join("export").join(&name))
        .expect("the revision plan is written under export/");
    assert!(body.contains("object: locket") && body.contains("pov: head-hop"));
    assert!(
        !body.contains("first chapter") && !body.contains("second chapter"),
        "no manuscript prose is written into the plan:\n{body}"
    );
    drop(dir);
}

#[test]
fn revision_plan_with_no_open_findings_exports_the_clean_state() {
    let (mut app, dir) = two_chapter_app("revision-plan-empty");
    app.export_revision_plan();
    let name = format!(
        "revision-plan-{}.md",
        crate::date::iso(crate::date::today())
    );
    let body = fs::read_to_string(dir.join("export").join(&name))
        .expect("the plan is written even when empty");
    assert!(body.contains("Nothing to plan"), "{body}");
    drop(dir);
}

/// Performance guard (`cargo perf`). Recording and rendering the health trend
/// (#1052) must cost the same whether the project is two chapters or a series of
/// thousands: it reads only the small persisted tallies and folds a fixed set of
/// findings, never the manuscript. Opening the dashboard over a large project with
/// a bounded findings set must cost the same at series scale as at two chapters —
/// a regression that made the trend re-scan chapters would show as the large
/// project growing far more expensive. Mirrors `tally_is_independent_of_project_size`.
///
/// The guard is *comparative*: it times opening the dashboard over a 2-chapter and
/// a 2000-chapter project with the identical (small) findings set, and asserts the
/// large project isn't meaningfully slower. This isolates the "no manuscript-
/// proportional recompute" claim from the fixed per-open cost (the small side-file
/// write's fsync), which is the same at either scale.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn health_trend_render_is_independent_of_project_size() {
    use std::time::{Duration, Instant};

    // A project of `n` chapters, with a fixed four-finding triage set spread across
    // the first few chapters — the tally and trend fold over these, never the
    // chapters — timed over repeated dashboard opens (each records + renders).
    fn time_open(name: &str, n: usize) -> Duration {
        let dir = crate::testutil::scratch_dir("perf", name);
        let dir = TempProject::new(dir);
        let mut manifest = String::from("title = \"Series\"\n\n");
        for i in 0..n {
            let file = format!("ch{i}.md");
            write!(
                manifest,
                "[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
            )
            .unwrap();
            fs::write(dir.join(&file), "Some prose in this chapter.\n").unwrap();
        }
        fs::write(Project::manifest_path(&dir), manifest).unwrap();

        let project = Project::open(&dir).unwrap();
        let buffer = Buffer::from_path(project.active_path()).unwrap();
        let mut app = App::new(buffer, project);
        app.accumulate_triage(&[
            finding(FindingKind::Continuity, "Ch0", "object: locket"),
            finding(FindingKind::Continuity, "Ch1", "time: sunset twice"),
            finding(FindingKind::Style, "Ch2", "pov: head-hop"),
            finding(FindingKind::Plot, "Ch3", "thread: the letter"),
        ]);

        app.open_health_dashboard(); // warm (seeds the side-file)
        let iters = 200;
        let t = Instant::now();
        for _ in 0..iters {
            app.open_health_dashboard(); // records the run + renders the trend
        }
        t.elapsed() / iters
    }

    let small = time_open("health-trend-small", 2);
    let large = time_open("health-trend-large", 2000);
    println!("health-trend open: 2 chapters = {small:?}, 2000 chapters = {large:?}");

    // The large project must not cost appreciably more than the small one: the work
    // is over the four findings and the small side-file, not the manuscript. Allow a
    // generous multiple for scheduling / filesystem noise on the fixed per-open cost;
    // a per-chapter re-scan would blow this by orders of magnitude.
    let ceiling = small * 3 + Duration::from_millis(2);
    assert!(
        large <= ceiling,
        "series-scale open ({large:?}) must not exceed ~small-scale ({small:?}); \
         trend render must not scan the manuscript"
    );
}
