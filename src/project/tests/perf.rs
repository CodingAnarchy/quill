//! The `#[ignore]`d open-performance guards (`cargo perf`): opening a project
//! reads and parses every chapter file exactly once, so a large novel and a
//! series-sized project both open near-instantly. They catch a regression to
//! per-chapter re-reads or O(n²) work at open. See `docs/performance.md`.

use super::*;
use crate::testutil::perf_budget_ms;
use std::fmt::Write;
use std::time::Instant;

/// Write an `n`-chapter project corpus (each chapter has front matter and a
/// ~2.7 KB body) under `dir`, returning the manifest text for the caller to
/// write. Shared by the open-performance guards.
fn write_chapter_project(dir: &Path, n: usize, title: &str) -> String {
    let mut manifest = format!("title = \"{title}\"\n\n");
    for i in 0..n {
        let file = format!("ch{i}.md");
        write!(
            manifest,
            "[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
        )
        .unwrap();
        let mut body = String::from("---\nstatus: draft\npov: Mara\n---\n# Chapter\n\n");
        for _ in 0..50 {
            body.push_str("The night was dark and the road was very long indeed.\n");
        }
        fs::write(dir.join(&file), body).unwrap();
    }
    manifest
}

/// Performance guard (run with `cargo test --release -- --ignored
/// --nocapture`). Opening a project reads and parses every chapter file once
/// (word count + front matter); even a large novel must open near-instantly.
/// Catches a regression to per-chapter re-reads or O(n²) work at open.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn open_is_fast_for_a_large_novel() {
    let dir = temp_dir("perf-open");
    fs::create_dir_all(&dir).unwrap();
    let manifest = write_chapter_project(&dir, 150, "Big");
    fs::write(Project::manifest_path(&dir), manifest).unwrap();

    let t = Instant::now();
    let project = Project::open(&dir).unwrap();
    let dt = t.elapsed();

    println!("open 150 chapters (front matter + word counts) = {dt:?}");
    assert_eq!(project.manifest.chapters.len(), 150);
    assert!(dt.as_millis() < perf_budget_ms(250), "open took {dt:?}");

    fs::remove_dir_all(&dir).ok();
}

/// Performance guard (`cargo perf`). Opening a series-sized project (many
/// books' worth of chapters) stays within budget — the per-chapter cache read
/// is the only work proportional to the project, and it's bounded. See
/// `docs/performance.md`. Catches a regression to O(n²) work at open or to
/// re-reading files after the initial cache pass.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn open_is_fast_for_a_series_sized_project() {
    let n = 1500;
    let dir = temp_dir("perf-open-series");
    fs::create_dir_all(&dir).unwrap();
    let manifest = write_chapter_project(&dir, n, "Series");
    fs::write(Project::manifest_path(&dir), manifest).unwrap();

    let t = Instant::now();
    let project = Project::open(&dir).unwrap();
    let dt = t.elapsed();

    println!("open {n} chapters (series-sized) = {dt:?}");
    assert_eq!(project.manifest.chapters.len(), n);
    assert!(dt.as_millis() < perf_budget_ms(500), "open took {dt:?}");

    fs::remove_dir_all(&dir).ok();
}
