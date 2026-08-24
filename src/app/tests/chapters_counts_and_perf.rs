//! Rolled-up word counts, body-vs-section counting, and the navigation / tally performance guards.

use super::*;
use crate::project::Project;
use crate::testutil::{perf_budget_ms, perf_budget_us};
use std::fmt::Write;
use std::fs;
use std::time::Instant;

#[test]
fn report_counts_todo_markers_across_chapters() {
    // The non-active chapter's markers are on disk at open, so its TODO count is
    // cached like any file-derived count (#867); the active chapter's are read live
    // from the buffer. Seed both, then open, so the cache reflects the files.
    let dir = crate::testutil::scratch_dir("rep", "todo");
    let dir = TempProject::new(dir);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first xxx chapter XXX\n").unwrap();
    fs::write(dir.join("two.md"), "second xxx chapter\n").unwrap();
    let mut app = open_test_app(&dir);

    // Active chapter (one.md) from the buffer: 2; chapter two from its cache: 1.
    assert_eq!(app.count_todo(), 3);

    // A live edit to the active chapter is reflected immediately (buffer, not cache).
    app.buffer.replace_text("no markers here\n");
    assert_eq!(app.count_todo(), 1);
}

#[test]
fn word_counts_roll_up_and_update_live() {
    let (mut app, _dir) = two_chapter_app("counts"); // "first chapter", "second chapter"
    assert_eq!(app.chapter_word_count(0), 2); // active, live from buffer
    assert_eq!(app.chapter_word_count(1), 2); // cached from file
    assert_eq!(app.project_word_count(), 4);

    // Add a word to the active chapter; its count and the total grow live.
    app.on_key(key(KeyCode::Char('X')));
    app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(app.chapter_word_count(0), 3);
    assert_eq!(app.project_word_count(), 5);
}

#[test]
fn the_report_counts_body_words_not_section_words() {
    let dir = crate::testutil::scratch_dir("rep", "sect");
    let dir = TempProject::new(dir);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[goals]\nwords = 100\nchapters = 5\n\n\
         [[chapters]]\ntitle = \"Body\"\nfile = \"b.md\"\n\n\
         [[chapters]]\ntitle = \"Appendix\"\nfile = \"a.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("b.md"), "one two three four five\n").unwrap(); // 5 body words
    fs::write(
        dir.join("a.md"),
        "---\nrole: appendix\n---\nalpha beta gamma\n",
    )
    .unwrap(); // 3 section words

    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let app = App::new(buffer, project);

    assert_eq!(app.body_word_count(), 5);
    assert_eq!(app.section_word_count(), 3);

    let report: String = app
        .report_lines()
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
        report.contains("5 / 100  words"),
        "the goal measures body words only:\n{report}"
    );
    assert!(
        report.contains("(+3 words in front/back matter, not counted)"),
        "{report}"
    );
}

#[test]
fn word_count_cache_tracks_add_and_delete() {
    let (mut app, _dir) = two_chapter_app("counts-structure");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('a'))); // add an empty chapter after ch.0
    type_str(&mut app, "New");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.chapter_count(), 3);
    assert_eq!(app.project_word_count(), 4); // the new chapter is empty

    // Delete it (the new chapter is the current selection).
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('y')));
    assert_eq!(app.chapter_count(), 2);
    assert_eq!(app.project_word_count(), 4);
}

#[test]
fn saving_the_active_chapter_refreshes_its_cached_todo_count() {
    // The save-path TODO roll-up reads cached per-chapter counts (#867). Editing the
    // active chapter and saving must refresh its cache so a later count from a
    // *different* active chapter still totals the markers this one now holds.
    let (mut app, _dir) = two_chapter_app("save-todo"); // one.md active, two.md cached
    assert_eq!(app.count_todo(), 0);

    // Type two markers into the active chapter and save; its cached count updates.
    app.buffer.replace_text("xxx and another xxx here\n");
    assert_eq!(app.count_todo(), 2); // live from the buffer
    app.on_key(ctrl('s'));
    assert_eq!(app.project.chapter_todo(0), 2, "the save cached the count");

    // Switch away so chapter 0 is counted from its cache, not the buffer — the
    // roll-up still sees the saved markers.
    app.on_key(ctrl('n'));
    assert_ne!(app.project.active_index(), 0);
    assert_eq!(app.count_todo(), 2);
}

#[test]
fn project_word_count_reads_the_maintained_total() {
    // The total is kept in step as chapters are cached/edited/saved (#867), so a
    // live edit to the active chapter shows immediately, and a save leaves the
    // cached total consistent when the chapter is later counted from its cache.
    let (mut app, _dir) = two_chapter_app("total"); // 2 + 2 words
    assert_eq!(app.project_word_count(), 4);

    app.buffer.replace_text("alpha beta gamma delta epsilon\n"); // 5 words
    assert_eq!(app.project_word_count(), 7); // 5 (live) + 2 (cache)
    app.on_key(ctrl('s'));
    assert_eq!(app.project_word_count(), 7);

    // From another active chapter, chapter 0 is counted from its (saved) cache.
    app.on_key(ctrl('n'));
    assert_eq!(app.project_word_count(), 7);
}

#[test]
fn continuous_view_refuses_to_open_with_an_unreadable_chapter() {
    let (mut app, dir) = two_chapter_app("cont-unreadable");
    fs::remove_file(dir.join("two.md")).unwrap(); // a chapter file vanishes
    app.on_key(ctrl('l')); // try to enter the continuous view
    assert!(app.manuscript.is_none(), "stays in the single-chapter view");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Couldn't open continuous view"),
        "the failure is surfaced: {:?}",
        app.message
    );
}

/// Performance guard (`cargo perf`). Switching chapters in a series-sized
/// project must stay instant: only a window is ever loaded, so a switch
/// reads/parses one chapter file — never work proportional to the project. If a
/// future change makes navigation re-read or re-scan other chapters, this catches
/// it. See `docs/performance.md`.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn navigation_is_fast_in_a_series_sized_project() {
    // Far past a single novel: a multi-book series' worth of chapter files.
    let n = 600;
    let dir = crate::testutil::scratch_dir("perf", "nav");
    let dir = TempProject::new(dir);
    let mut manifest = String::from("title = \"Series\"\n\n");
    for i in 0..n {
        let file = format!("ch{i}.md");
        write!(
            manifest,
            "[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
        )
        .unwrap();
        let mut body = String::from("# Chapter\n\n");
        for _ in 0..40 {
            body.push_str("The night was dark and the road was very long indeed.\n");
        }
        fs::write(dir.join(&file), body).unwrap();
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();

    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);

    // One untimed switch first: the first parse pays one-off allocation/CPU-ramp
    // costs that aren't part of the steady-state per-switch cost we're guarding.
    app.on_key(ctrl('n'));
    let switches = 50;
    let t = Instant::now();
    for _ in 0..switches {
        app.on_key(ctrl('n'));
    }
    let per = t.elapsed() / switches; // average over many switches smooths noise

    println!("chapter switch in a {n}-chapter project = {per:?}");
    assert!(
        per.as_millis() < perf_budget_ms(10),
        "chapter switch took {per:?}"
    );
}

/// Performance guard (`cargo perf`). The daily word tally runs on every idle
/// tick; its cost must not grow with the project — it touches only the chapters
/// that changed (the live active one, plus any whose cache moved), not all of
/// them (#220). See `docs/performance.md`.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn tally_is_independent_of_project_size() {
    let n = 2000;
    let dir = crate::testutil::scratch_dir("perf", "tally");
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
    // Edit a chapter near the end — the worst case for any active-chapter lookup.
    app.activate_chapter(n - 1);

    app.on_idle(); // warm
    let iters = 200;
    let t = Instant::now();
    for _ in 0..iters {
        app.buffer.insert_char('x'); // an edit, so the active chapter is re-checked
        app.tally_words();
    }
    let per = t.elapsed() / iters;

    // A full per-chapter walk (the old behavior) would be ~50–100µs at this size;
    // touching only the changed chapter keeps it an order of magnitude cheaper.
    println!("tally over a {n}-chapter project = {per:?}");
    assert!(per.as_micros() < perf_budget_us(30), "tally took {per:?}");
}

/// Performance guard (`cargo perf`). "Verify on idle" (#1053) re-checks findings
/// whose chapters changed, and its planning runs on every idle tick when the opt-in
/// is on. Its cost must not grow with the project: it digests only the *open
/// findings'* chapters (a small bounded set), never the whole manuscript. With a
/// handful of open findings in a huge project, planning stays flat — the guard the
/// "no whole-manuscript re-analysis" contract rests on. See `docs/performance.md`.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn verify_on_idle_plan_is_independent_of_project_size() {
    let n = 2000;
    let dir = crate::testutil::scratch_dir("perf", "verify-idle");
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
    app.ai.verify_on_idle = true;
    // Two open findings, on the first and last chapters (the worst case for lookups),
    // baselined at open. The plan should touch only these two chapters, never all n.
    let findings = vec![
        crate::mode::Finding {
            summary: "object: a drift".into(),
            kind: crate::mode::FindingKind::Continuity,
            chapters: vec!["Ch0".into()],
            book: None,
            action: Some(crate::mode::AiAction::Continuity),
            corroborators: Vec::new(),
        },
        crate::mode::Finding {
            summary: "time: a gap".into(),
            kind: crate::mode::FindingKind::Continuity,
            chapters: vec![format!("Ch{}", n - 1)],
            book: None,
            action: Some(crate::mode::AiAction::Continuity),
            corroborators: Vec::new(),
        },
    ];
    app.accumulate_triage(&findings);

    let iters = 200;
    let t = Instant::now();
    for _ in 0..iters {
        let _ = app.verify_on_idle_targets(); // the planning cost the idle tick pays
    }
    let per = t.elapsed() / iters;

    // A whole-manuscript digest sweep would be O(n) file reads (~ms at this size);
    // digesting only the two open findings' chapters keeps it flat and cheap.
    println!("verify-on-idle plan over a {n}-chapter project = {per:?}");
    assert!(
        per.as_micros() < perf_budget_us(500),
        "verify-on-idle plan took {per:?}"
    );
}

/// Open an `n`-chapter project whose chapters each hold prose plus one `xxx`
/// marker, ready for the save-path perf guard. Each chapter is deliberately
/// non-trivial so that a regression to per-chapter disk reads/scans would show up
/// in the timing. Returns the app with the last chapter active (the worst case for
/// any active-index lookup).
#[cfg(test)]
fn marked_project(name: &str, n: usize) -> (App, TempProject) {
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
        let mut body = String::from("# Chapter\n\n");
        for _ in 0..20 {
            body.push_str("The night was dark and the road was very long xxx indeed.\n");
        }
        fs::write(dir.join(&file), body).unwrap();
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);
    app.activate_chapter(n - 1); // worst case for the active-index lookup
    (app, dir)
}

/// The O(project) roll-up a save performs: the TODO count and the project word
/// total the snapshot records. Timed by the guard below; both must read caches, so
/// this is flat in the chapter count.
#[cfg(test)]
fn save_rollup_cost(app: &App) -> (usize, usize) {
    (app.count_todo(), app.project_word_count())
}

/// Performance guard (`cargo perf`). Saving (Ctrl-S / autosave / chapter-switch
/// flush) records a history snapshot whose TODO count and word total used to be an
/// O(project) disk sweep — every non-active chapter re-read and re-scanned on each
/// save (#867). Both now read maintained caches, so a save's roll-up cost is
/// independent of project size: it must stay flat from 150 to 1500 chapters, never
/// scale with the ~10× larger project. A regression to the per-chapter reads would
/// make the 1500-chapter roll-up ~10× the 150-chapter one and blow the ceilings.
/// See `docs/performance.md`.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn save_is_independent_of_project_size() {
    // Time the save roll-up at a novel and a series scale, averaging many calls to
    // smooth filesystem/CPU noise. If the fix holds, neither reads any chapter file.
    let measure = |app: &App, iters: u32| {
        // Warm: the first call pays one-off allocation / CPU-ramp costs.
        std::hint::black_box(save_rollup_cost(app));
        let t = Instant::now();
        for _ in 0..iters {
            std::hint::black_box(save_rollup_cost(app));
        }
        t.elapsed() / iters
    };

    let (small, _small_dir) = marked_project("save-small", 150);
    let (large, _large_dir) = marked_project("save-large", 1500);
    let iters = 500;
    let small_per = measure(&small, iters);
    let large_per = measure(&large, iters);

    println!("save roll-up: 150 chapters = {small_per:?}, 1500 chapters = {large_per:?}");
    // Absolute ceiling: the roll-up is a couple of cache sums plus one live scan of
    // the active chapter — well under a frame even at series scale.
    assert!(
        large_per.as_micros() < perf_budget_us(500),
        "save roll-up at 1500 chapters took {large_per:?} — should be flat"
    );
    // Flatness: 10× the chapters must not cost ~10× the time. A generous 4× ceiling
    // (over a floor, so sub-µs noise on the small case can't trip it) still catches
    // a regression to O(project) disk reads, which would be an order of magnitude.
    let floor = std::time::Duration::from_micros(20);
    let budget = std::cmp::max(small_per, floor) * 4;
    assert!(
        large_per <= budget,
        "save roll-up scaled with project size: 150 = {small_per:?}, 1500 = {large_per:?}"
    );
}
