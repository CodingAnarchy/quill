//! Commit history, diff, and restore through the Git palette (#587/#704/#833/#964/
//! #1013): the history view, per-chapter diff/restore, the pre-commit review, and
//! the changes/restore actions. Pure move from `vcs.rs`.
use super::super::*;
use super::*;

#[test]
fn git_palette_shows_commit_history() {
    let (mut app, _dir) = two_chapter_app("vc-log");
    git_menu(&mut app, 0); // Initialize
    git_menu_label(&mut app, "History"); // before any commit: a message, no view
    assert!(!matches!(app.mode, Mode::History(_)));
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("No commits yet"));

    commit_via_menu(&mut app, "first draft");

    git_menu_label(&mut app, "History"); // now a navigable view
    assert!(
        history_summaries(&app).contains(&"first draft".to_string()),
        "history lists the commit"
    );
    app.on_key(key(KeyCode::Esc)); // close it
    assert!(!matches!(app.mode, Mode::History(_)));
}

#[test]
fn history_renders_the_commit_graph_and_changed_chapters() {
    // #964: the history view draws a commit graph (a `●` node per commit) and lists
    // the chapters each commit changed. Two commits touching different chapters, then
    // render the overlay and check both the node glyph and a changed-chapter name show.
    let (mut app, _dir) = two_chapter_app("vc-hist-graph");
    git_menu(&mut app, 0); // Initialize

    // First commit touches the active chapter (one.md).
    type_str(&mut app, "The night was dark.");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "first draft");

    // A second commit that changes the same chapter again.
    type_str(&mut app, " Then it was long.");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "revise it");

    git_menu_label(&mut app, "History");
    let screen = crate::ui::render_to_string(&app, 100, 24);
    assert!(
        screen.contains('●'),
        "the history view draws a commit-graph node:\n{screen}"
    );
    assert!(
        screen.contains("one.md"),
        "each commit lists the chapters it changed:\n{screen}"
    );
    assert!(
        screen.contains("first draft") && screen.contains("revise it"),
        "the commit summaries still render:\n{screen}"
    );
}

#[test]
fn history_diffs_the_chapter_against_a_chosen_commit() {
    let (mut app, _dir) = two_chapter_app("vc-hist-diff");
    git_menu(&mut app, 0); // Initialize

    // Commit an original version of the active chapter (one.md).
    type_str(&mut app, "The night was dark.");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "original");

    // Edit and save a newer version (the cursor sits after the committed text).
    type_str(&mut app, " Then it was long.");
    app.on_key(ctrl('s'));

    // Open history, select the "original" commit, and diff.
    git_menu_label(&mut app, "History"); // newest first: [original]
    app.on_key(key(KeyCode::Char('d')));
    let diff = overlay_text(&app);
    assert!(
        diff.contains("+") && diff.contains("Then it was long"),
        "the diff shows what changed since that commit: {diff}"
    );
}

#[test]
fn history_diff_highlights_only_the_changed_word_within_a_line() {
    // A one-word revision should light up only that word, not the whole line
    // (#1013): the shared words render dim, the swapped word bold in the
    // added / removed color. Line-level context (the `+`/`-` markers, the
    // unchanged surrounding text) is preserved.
    use ratatui::style::Modifier;

    let (mut app, _dir) = two_chapter_app("vc-word-diff");
    git_menu(&mut app, 0); // Initialize

    // Commit "dark", then revise the single adjective to "bright" and save. Each
    // version replaces the whole buffer (a clean one-line change), the way the
    // author would retype the line.
    let path = app.project.active_path();
    app.buffer = Buffer::from_file_text(&path, "The night was dark and cold.\n");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "original");
    app.buffer = Buffer::from_file_text(&path, "The night was bright and cold.\n");
    app.on_key(ctrl('s'));

    git_menu_label(&mut app, "History");
    app.on_key(key(KeyCode::Char('d')));

    let lines = app.overlay_lines().expect("diff overlay open").to_vec();
    // The added line: its spans carry per-word styling now.
    let added = lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains("bright")))
        .expect("an added line mentioning the new word");
    let removed = lines
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains("dark")))
        .expect("a removed line mentioning the old word");

    let bold_added: Vec<&str> = added
        .spans
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    let bold_removed: Vec<&str> = removed
        .spans
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    // Only the changed word is emphasized on each side.
    assert_eq!(bold_added, vec!["bright"], "only the new word is bold");
    assert_eq!(bold_removed, vec!["dark"], "only the old word is bold");

    // The shared words are present but *not* emphasized — visually distinct from
    // the change, so the line still reads as context.
    let night = added
        .spans
        .iter()
        .find(|s| s.content.contains("night"))
        .expect("shared context word present");
    assert!(
        !night.style.add_modifier.contains(Modifier::BOLD),
        "unchanged words within the line are not emphasized"
    );

    // The changed word carries the added color; the shared word does not.
    let theme = crate::theme::current();
    let bright = added
        .spans
        .iter()
        .find(|s| s.content == "bright")
        .expect("changed span");
    assert_eq!(bright.style.fg, Some(theme.added));
    assert_ne!(
        night.style.fg,
        Some(theme.added),
        "shared context is not colored like an addition"
    );

    // Line-level context preserved: the diff still carries its `+`/`-` markers.
    assert_eq!(added.spans.first().map(|s| s.content.as_ref()), Some("+"));
    assert_eq!(removed.spans.first().map(|s| s.content.as_ref()), Some("-"));
}

#[test]
fn history_restores_the_chapter_from_a_chosen_commit() {
    let (mut app, _dir) = two_chapter_app("vc-hist-restore");
    git_menu(&mut app, 0); // Initialize

    // First committed version of the active chapter.
    type_str(&mut app, "The first draft.");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "draft one");

    // A second, different committed version.
    type_str(&mut app, " Now revised.");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "draft two");
    assert!(
        app.buffer.text().contains("Now revised"),
        "the revision is live"
    );

    // Open history (newest first: [draft two, draft one]); pick the older commit.
    git_menu_label(&mut app, "History");
    app.on_key(key(KeyCode::Down)); // move to "draft one"
    app.on_key(key(KeyCode::Char('r'))); // restore from it

    // The older version is in the editable buffer (review-and-save), not written.
    assert!(!matches!(app.mode, Mode::History(_)), "the view closed");
    assert!(
        app.buffer.text().contains("The first draft") && !app.buffer.text().contains("Now revised"),
        "the older version loaded into the buffer: {:?}",
        app.buffer.text()
    );
    assert!(
        app.edit_dirty(),
        "the recovered version is dirty, awaiting save"
    );
    // Unsaved: the file on disk still holds the newer, committed version.
    assert!(
        crate::vcs::show_file_at(&app.project.root, "HEAD", "one.md")
            .unwrap()
            .unwrap()
            .contains("Now revised")
    );
}

/// Write the durable findings store (`bible/findings.toml`) with the given open
/// findings — a helper for the findings-diff tests (#1051). Each entry is one open
/// (not dismissed) finding on chapter "One". Mirrors the [`FindingsStore`] record
/// shape so the app reads it back through `show_file_at` exactly as it would a real
/// store committed at a past milestone.
fn write_findings_store(root: &std::path::Path, summaries: &[&str]) {
    let entries: Vec<String> = summaries
        .iter()
        .map(|s| {
            format!(
                "[[findings]]\nsummary = \"{s}\"\nkind = \"continuity\"\n\
                 chapters = [\"One\"]\ndismissed = false\n"
            )
        })
        .collect();
    let dir = root.join("bible");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("findings.toml"), entries.join("\n")).unwrap();
}

// The findings diff (#1051): the author picks two points in history and sees which
// findings resolved, which were introduced, and which stayed open — a pure local
// comparison of the durable store committed at each point, no AI request. Commit a
// store at point A, revise it at point B, then anchor A with `f` and diff against B.
#[test]
fn findings_diff_classifies_between_two_history_points() {
    let (mut app, _dir) = two_chapter_app("vc-findings-diff");
    git_menu(&mut app, 0); // Initialize

    // Point A: two open findings.
    write_findings_store(&app.project.root, &["a plot hole", "the locket color"]);
    commit_via_menu(&mut app, "point A");

    // Point B: the plot hole is resolved (gone), the locket carries over, and a new
    // dangling thread is introduced.
    write_findings_store(
        &app.project.root,
        &["the locket color", "a dangling thread"],
    );
    commit_via_menu(&mut app, "point B");

    // History lists [point B (newest), point A]. Anchor the *earlier* point A first,
    // then diff against the current selection at B.
    git_menu_label(&mut app, "History");
    app.on_key(key(KeyCode::Down)); // move to "point A"
    app.on_key(key(KeyCode::Char('f'))); // anchor point A
    assert!(
        app.message.as_deref().unwrap_or("").contains("point A set"),
        "the first f anchors point A: {:?}",
        app.message
    );
    app.on_key(key(KeyCode::Up)); // back to "point B"
    app.on_key(key(KeyCode::Char('f'))); // diff A → B

    let report = overlay_text(&app);
    assert!(
        report.contains("Resolved (1)") && report.contains("a plot hole"),
        "the plot hole gone at B is resolved: {report}"
    );
    assert!(
        report.contains("Introduced (1)") && report.contains("a dangling thread"),
        "the new finding at B is introduced: {report}"
    );
    assert!(
        report.contains("Still open (1)") && report.contains("the locket color"),
        "the carried-over finding stays open: {report}"
    );
    // The history view stays open behind the overlay, and the anchor was cleared.
    match &app.mode {
        Mode::History(h) => assert!(
            h.findings_anchor.is_none(),
            "the anchor resets after the diff"
        ),
        _ => panic!("history stays open behind the diff overlay"),
    }
}

// The findings diff never asks the AI (#1051): it's a pure read of two committed
// stores. We drive the whole flow with no backend wired and assert no AI request was
// dispatched — the overlay still renders from local data alone.
#[test]
fn findings_diff_makes_no_ai_request() {
    let (mut app, _dir) = two_chapter_app("vc-findings-diff-no-ai");
    git_menu(&mut app, 0); // Initialize

    write_findings_store(&app.project.root, &["only at A"]);
    commit_via_menu(&mut app, "A");
    write_findings_store(&app.project.root, &["only at B"]);
    commit_via_menu(&mut app, "B");

    git_menu_label(&mut app, "History");
    app.on_key(key(KeyCode::Down)); // point A
    app.on_key(key(KeyCode::Char('f'))); // anchor
    app.on_key(key(KeyCode::Up)); // point B
    app.on_key(key(KeyCode::Char('f'))); // diff — purely local

    assert!(
        !app.ai_task.is_busy(),
        "the findings diff dispatches no AI request"
    );
    let report = overlay_text(&app);
    assert!(
        report.contains("Resolved (1)") && report.contains("only at A"),
        "the diff rendered from the committed stores alone: {report}"
    );
}

// Empty stores diff cleanly (#1051): two points with no findings compare to nothing,
// and the flow reports "nothing to compare" rather than opening an empty overlay.
#[test]
fn findings_diff_of_empty_stores_reports_nothing() {
    let (mut app, _dir) = two_chapter_app("vc-findings-diff-empty");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "A"); // no findings store at either point
    type_str(&mut app, "a small edit");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "B");

    git_menu_label(&mut app, "History");
    app.on_key(key(KeyCode::Down)); // point A
    app.on_key(key(KeyCode::Char('f'))); // anchor
    app.on_key(key(KeyCode::Up)); // point B
    app.on_key(key(KeyCode::Char('f'))); // diff

    assert!(app.overlay.is_none(), "no overlay for an empty comparison");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("nothing to compare"),
        "an empty diff reports it: {:?}",
        app.message
    );
}

#[test]
fn git_palette_commits_with_a_message() {
    let (mut app, _dir) = two_chapter_app("vcs-commit");
    git_menu(&mut app, 0); // Initialize

    // Commit first opens the pre-commit review (#704): the changed-file list + diff.
    git_menu(&mut app, 0); // Commit
    assert!(app.overlay.is_some(), "the pre-commit review overlay opens");
    let review = overlay_text(&app);
    assert!(
        review.contains("one.md") && review.contains("will be committed"),
        "the review lists the changed files: {review}"
    );

    // Confirm the review → the message prompt.
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "a commit-message prompt opens after the review"
    );
    type_str(&mut app, "first draft");
    app.on_key(key(KeyCode::Enter));
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .starts_with("Committed"),
        "{:?}",
        app.message
    );
    // Everything was captured → the tree is now clean.
    assert_eq!(crate::vcs::commit(&app.project.root, "x").unwrap(), None);
}

#[test]
fn pre_commit_review_can_be_cancelled_without_committing() {
    let (mut app, _dir) = two_chapter_app("vcs-review-cancel");
    git_menu(&mut app, 0); // Initialize
    let dirty_before = app.vc_changes;
    assert!(dirty_before > 0, "the fresh repo has uncommitted files");

    // Open the review, then cancel with Esc — nothing is committed.
    git_menu(&mut app, 0); // Commit
    assert!(app.overlay.is_some(), "the review overlay opens");
    app.on_key(key(KeyCode::Esc));
    assert!(app.overlay.is_none(), "the review closed");
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "cancelling doesn't open the message prompt"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("cancelled"),
        "{:?}",
        app.message
    );
    app.refresh_vc_status();
    assert_eq!(
        app.vc_changes, dirty_before,
        "nothing was committed — the tree is still dirty"
    );
}

#[test]
fn commit_on_a_clean_tree_reports_nothing_to_commit() {
    let (mut app, _dir) = two_chapter_app("vcs-clean");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "everything"); // capture the initial files
    assert_eq!(app.vc_changes, 0, "clean after the first commit");

    // A second Commit on a clean tree: no review overlay, just a message.
    git_menu(&mut app, 0); // Commit
    assert!(app.overlay.is_none(), "no review overlay on a clean tree");
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "and no message prompt either"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Nothing to commit"),
        "{:?}",
        app.message
    );
}

#[test]
fn pre_commit_review_shows_the_diff_of_what_will_be_committed() {
    let (mut app, _dir) = two_chapter_app("vcs-review-diff");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "base"); // capture the starting files

    // Edit and save the active chapter, then open the review.
    type_str(&mut app, "The night was long.");
    app.on_key(ctrl('s'));
    git_menu(&mut app, 0); // Commit → the pre-commit review
    let review = overlay_text(&app);
    assert!(
        review.contains("1 file will be committed") && review.contains("one.md"),
        "the review lists the single changed file: {review}"
    );
    assert!(
        review.contains("+The night was long."),
        "the review shows the added line the commit would capture: {review}"
    );

    // Confirming commits it; the tree goes clean.
    app.on_key(key(KeyCode::Enter));
    type_str(&mut app, "add a line");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.vc_changes, 0, "committed → clean working tree");
}

#[test]
fn git_palette_shows_changes_diff() {
    let (mut app, _dir) = two_chapter_app("vc-diff");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "base");

    // Clean tree → a message, no overlay.
    git_menu_label(&mut app, "Changes");
    assert!(app.overlay.is_none());
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("No uncommitted changes"));

    // Edit + save → the diff overlay shows the changed file.
    app.on_key(key(KeyCode::Char('Q')));
    app.on_key(ctrl('s'));
    git_menu_label(&mut app, "Changes");
    assert!(
        overlay_text(&app).contains("one.md"),
        "diff names the changed file"
    );
}

#[test]
fn git_palette_restores_a_chapter() {
    let (mut app, _dir) = two_chapter_app("vc-restore");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "base");

    // Edit + save, then restore.
    app.on_key(key(KeyCode::Char('Z')));
    app.on_key(ctrl('s'));
    assert!(app.buffer.text().contains('Z'), "the edit landed");
    git_menu_label(&mut app, "Restore chapter");
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "restore asks for confirmation"
    );
    app.on_key(key(KeyCode::Char('y')));
    assert!(
        !app.buffer.text().contains('Z'),
        "the edit was reverted: {:?}",
        app.buffer.text()
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("Restored"),
        "{:?}",
        app.message
    );
}
