//! One chapter's history (#1299): the commits that changed it, and comparing
//! two versions of its prose word by word.

use std::fs;

use super::temp_dir;
use crate::vcs::{commit, compare_prose, file_log, init, Change};

#[test]
fn a_chapters_log_lists_only_the_commits_that_changed_it() {
    let root = temp_dir("versions-log");
    init(&root).expect("init");
    fs::write(root.join("one.md"), "First.\n").expect("write");
    fs::write(root.join("two.md"), "Other.\n").expect("write");
    commit(&root, "start").expect("commit");
    fs::write(root.join("two.md"), "Other, changed.\n").expect("write");
    commit(&root, "only two").expect("commit");
    fs::write(root.join("one.md"), "First, revised.\n").expect("write");
    commit(&root, "one again").expect("commit");

    let versions = file_log(&root, "one.md", 10).expect("log");
    let summaries: Vec<&str> = versions.iter().map(|v| v.summary.as_str()).collect();
    assert_eq!(
        summaries,
        ["one again", "start"],
        "newest first, without the commit to two.md"
    );
    assert!(versions[0].seconds >= versions[1].seconds);
    assert_eq!(
        file_log(&root, "one.md", 1).expect("log").len(),
        1,
        "the limit holds"
    );
    assert!(file_log(&root, "gone.md", 10).expect("log").is_empty());
}

#[test]
fn a_repository_with_no_commits_has_no_versions() {
    let root = temp_dir("versions-empty");
    init(&root).expect("init");
    assert!(file_log(&root, "one.md", 10).expect("log").is_empty());
}

#[test]
fn comparing_prose_marks_words_kept_added_and_removed() {
    // A word carries its punctuation, as a reader sees it.
    let runs = compare_prose("The sea came in fast.", "The sea came in slowly.");
    assert_eq!(
        runs,
        vec![
            (Change::Same, "The sea came in ".to_string()),
            (Change::Removed, "fast.".to_string()),
            (Change::Added, "slowly.".to_string()),
        ]
    );
    // A rewritten phrase is one cut and one addition, not word by word.
    assert_eq!(
        compare_prose(
            "The sea came in. Then dark.",
            "The sea went out. Then dark."
        ),
        vec![
            (Change::Same, "The sea ".to_string()),
            (Change::Removed, "came in.".to_string()),
            (Change::Added, "went out.".to_string()),
            (Change::Same, " Then dark.".to_string()),
        ]
    );
    // An addition at the end keeps its leading space with the kept text.
    assert_eq!(
        compare_prose("The sea.", "The sea. Then dark."),
        vec![
            (Change::Same, "The sea.".to_string()),
            (Change::Added, " Then dark.".to_string()),
        ]
    );
    assert_eq!(
        compare_prose("Same.", "Same."),
        vec![(Change::Same, "Same.".to_string())]
    );
}
