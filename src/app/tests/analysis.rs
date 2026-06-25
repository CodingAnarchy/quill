//! The analysis report honors per-project word-list overrides (#119): a stopword
//! the project adds is excluded from the most-used-words list.

use super::*;
use crate::project::Analysis;

/// The plain text of each report line.
fn line_texts(lines: &[ratatui::text::Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect()
}

/// Whether the "Most-used words" list has an entry for exactly `word`
/// (`"N×  word"`), distinct from the same word appearing in a quoted sentence or
/// a multi-word phrase entry.
fn most_used_has(lines: &[ratatui::text::Line<'static>], word: &str) -> bool {
    line_texts(lines)
        .iter()
        .any(|t| t.trim().split_once("×  ").is_some_and(|(_, rest)| rest == word))
}

#[test]
fn analysis_report_honors_stopword_overrides() {
    let (mut app, _dir) = two_chapter_app("analysis-override");
    // The active chapter's prose drives the report's word tally.
    app.buffer = Buffer::from_text("the locket gleamed and the locket gleamed");

    // By default the invented content word surfaces in the most-used list.
    assert!(most_used_has(&app.analysis_lines(), "locket"), "locket in most-used by default");

    // Adding it as a project stopword excludes it.
    app.project.manifest.analysis = Some(Analysis {
        stopwords_add: vec!["locket".into()],
        ..Default::default()
    });
    assert!(!most_used_has(&app.analysis_lines(), "locket"), "locket excluded after the override");
    // A different content word is still listed — only the override changed.
    assert!(most_used_has(&app.analysis_lines(), "gleamed"), "gleamed still listed");
}
