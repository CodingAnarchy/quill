//! The launch sequence's perf guard: `App::launch` — the startup `main` runs —
//! at series scale.

use super::*;
use crate::config::{set_config_dir, LoadSession};
use crate::lastpos;
use crate::testutil::perf_budget_ms;
use std::time::{Duration, Instant};

/// Write a `[[books]]` series of `books` × `per_book` chapters (front matter +
/// ~500 words each) with a `[quill.editor]` override in its manifest, and
/// remember chapter 3 of the first book as the last position.
fn write_series(dir: &Path, books: usize, per_book: usize) {
    let mut manifest = String::from("title = \"Series\"\n\n[quill.editor]\ncolumn_width = 80\n\n");
    for b in 0..books {
        fs::create_dir_all(dir.join(format!("b{b}"))).unwrap();
        write!(manifest, "[[books]]\ntitle = \"Book {b}\"\n\n").unwrap();
        for i in 0..per_book {
            let file = format!("b{b}/ch{i}.md");
            write!(
                manifest,
                "[[books.chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
            )
            .unwrap();
            let mut body = String::from("---\nstatus: draft\npov: Mara\n---\n# Chapter\n\n");
            for _ in 0..50 {
                body.push_str("The night was dark and the road was very long indeed.\n");
            }
            fs::write(dir.join(&file), body).unwrap();
        }
    }
    fs::write(Project::manifest_path(dir), manifest).unwrap();
    let pos = lastpos::Position {
        chapter_file: "b0/ch3.md".into(),
        cursor: 10,
    };
    lastpos::write(dir, &pos).unwrap();
}

/// The median of `runs` timings of `f`.
fn median(runs: usize, mut f: impl FnMut()) -> Duration {
    let mut times: Vec<Duration> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .collect();
    times.sort();
    times[runs / 2]
}

/// The whole launch — open, every config section, the remembered chapter, the
/// on-open recovery / damage / pass-queue offers — costs about one project open.
/// Opening a 1500-chapter series reads every chapter file once (word counts for
/// the series rollups); everything else the launch does is small next to that.
/// Catches a regression to opening the project twice (the damage check used to
/// re-open it and re-read every chapter), parsing the series-sized `book.toml`
/// once per config section, or loading chapter 0 before switching to the
/// remembered one. Compared against a bare open on the same machine, so the
/// ratio holds on any hardware.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn startup_is_fast_for_a_series() {
    let (books, per_book) = (6, 250); // 1500 chapters
    let dir = TempProject::new(app_scratch_dir("perf-launch-series"));
    write_series(&dir, books, per_book);
    let cfg = app_scratch_dir("perf-launch-series-cfg");
    fs::write(cfg.join("config.toml"), "[editor]\ncolumn_width = 72\n").unwrap();
    set_config_dir(Some(cfg.clone()));

    // Warm the filesystem cache so both measurements see the same disk.
    drop(Project::open(&dir).unwrap());
    let open = median(5, || drop(Project::open(&dir).unwrap()));
    let launch = median(5, || {
        // `main` also holds a session across launch + onboarding.
        let _session = LoadSession::begin();
        let mut app = App::launch(&dir).unwrap();
        app.maybe_offer_onboarding();
        assert_eq!(app.project.active_file(), "b0/ch3.md");
    });

    println!(
        "launch {} chapters = {launch:?} (bare open = {open:?})",
        books * per_book
    );
    assert!(
        launch < open * 3 / 2 + Duration::from_millis(10),
        "launch ({launch:?}) should cost about one open ({open:?})"
    );
    assert!(
        launch.as_millis() < perf_budget_ms(500),
        "launch took {launch:?}"
    );

    set_config_dir(None);
    fs::remove_dir_all(&cfg).ok();
}
