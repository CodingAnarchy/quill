//! Rolled-up word counts, body-vs-section counting, and the navigation / tally performance guards.

use super::*;
use crate::project::Project;
use std::fs;

#[test]
fn report_counts_todo_markers_across_chapters() {
    let (mut app, dir) = two_chapter_app("report-todo");
    // Add markers (mixed case) and reload the active chapter's buffer.
    fs::write(dir.join("one.md"), "first xxx chapter XXX\n").unwrap();
    fs::write(dir.join("two.md"), "second xxx chapter\n").unwrap();
    app.buffer = Buffer::from_path(app.project.active_path()).unwrap();

    // Active chapter (one.md) from the buffer: 2; chapter two from disk: 1.
    assert_eq!(app.count_todo(), 3);
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
#[ignore]
fn navigation_is_fast_in_a_series_sized_project() {
    use std::time::Instant;

    // Far past a single novel: a multi-book series' worth of chapter files.
    let n = 600;
    let dir = crate::testutil::scratch_dir("perf", "nav");
    let dir = TempProject::new(dir);
    let mut manifest = String::from("title = \"Series\"\n\n");
    for i in 0..n {
        let file = format!("ch{i}.md");
        manifest.push_str(&format!(
            "[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
        ));
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
    assert!(per.as_millis() < 10, "chapter switch took {per:?}");
}

/// Performance guard (`cargo perf`). The daily word tally runs on every idle
/// tick; its cost must not grow with the project — it touches only the chapters
/// that changed (the live active one, plus any whose cache moved), not all of
/// them (#220). See `docs/performance.md`.
#[test]
#[ignore]
fn tally_is_independent_of_project_size() {
    use std::time::Instant;

    let n = 2000;
    let dir = crate::testutil::scratch_dir("perf", "tally");
    let dir = TempProject::new(dir);
    let mut manifest = String::from("title = \"Series\"\n\n");
    for i in 0..n {
        let file = format!("ch{i}.md");
        manifest.push_str(&format!(
            "[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
        ));
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
    assert!(per.as_micros() < 30, "tally took {per:?}");
}
