//! The continuous-manuscript view, the progress report, the daily word tally, and per-part momentum.

use super::*;
use crate::project::Project;
use std::fs;

#[test]
fn continuous_view_edits_persist_to_the_right_chapter() {
    let (mut app, dir) = two_chapter_app("continuous"); // "first chapter", "second chapter"
    app.on_key(ctrl('l')); // enter continuous view
    assert!(app.manuscript.is_some());

    // Move into chapter two and append to it.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::End));
    type_str(&mut app, "Z");

    app.on_key(ctrl('l')); // leave continuous — saves on the way out
    assert!(app.manuscript.is_none());

    // The edit landed in chapter two's file; chapter one is untouched.
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "second chapterZ\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "first chapter\n"
    );
    // Back in single-chapter view at the chapter the cursor was in.
    assert_eq!(app.project.active_index(), 1);
    assert!(app.buffer.line(0).contains("second"));
}

#[test]
fn continuous_view_undo_and_redo_revert_an_edit() {
    // #649: undo/redo must work in the continuous view, not be refused.
    let (mut app, dir) = three_chapter_app("cont-undo"); // alpha / beta / gamma
    app.on_key(ctrl('l')); // enter continuous view
    assert!(app.manuscript.is_some());

    // Move into chapter 1 ("beta") and append to it.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::End));
    type_str(&mut app, "ZZ");
    app.on_idle(); // seal the edit group as a typing pause would
    assert!(app.edit_buffer().line(1).ends_with("betaZZ"));

    // Undo reverts the edit — no "isn't available" message.
    app.on_key(ctrl('z'));
    assert_eq!(app.edit_buffer().line(1), "beta");
    assert!(
        app.message.as_deref() != Some("Undo isn't available"),
        "undo was performed, not refused: {:?}",
        app.message
    );

    // Redo re-applies it.
    app.on_key(ctrl('y'));
    assert!(app.edit_buffer().line(1).ends_with("betaZZ"));

    // Leaving persists the redone edit to chapter 1's file only.
    app.on_key(ctrl('l'));
    assert_eq!(fs::read_to_string(dir.join("c1.md")).unwrap(), "betaZZ\n");
    assert_eq!(fs::read_to_string(dir.join("c0.md")).unwrap(), "alpha\n");
    assert_eq!(fs::read_to_string(dir.join("c2.md")).unwrap(), "gamma\n");
}

#[test]
fn continuous_view_undo_of_a_second_chapters_edit_leaves_the_first_intact() {
    // Two separate edits in the window, to two chapters. Undo peels off the most
    // recent (chapter 2); chapter 0's edit survives and saves to its own file.
    let (mut app, dir) = three_chapter_app("cont-undo-two");
    app.on_key(ctrl('l'));

    // Edit chapter 0 ("alpha"), then pause so it's its own undo group.
    app.on_key(key(KeyCode::End));
    type_str(&mut app, "0");
    app.on_idle();
    // Move to chapter 2 ("gamma") and edit it.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::End));
    type_str(&mut app, "2");
    app.on_idle();
    assert!(app.edit_buffer().line(0).ends_with("alpha0"));
    assert!(app.edit_buffer().line(2).ends_with("gamma2"));

    // One undo reverts only the chapter-2 edit.
    app.on_key(ctrl('z'));
    assert!(app.edit_buffer().line(0).ends_with("alpha0"));
    assert_eq!(app.edit_buffer().line(2), "gamma");

    // Leaving saves chapter 0's surviving edit; chapter 2 is back to disk's value.
    app.on_key(ctrl('l'));
    assert_eq!(fs::read_to_string(dir.join("c0.md")).unwrap(), "alpha0\n");
    assert_eq!(fs::read_to_string(dir.join("c2.md")).unwrap(), "gamma\n");
}

/// Enter the continuous view with Vim mode forced on, resting in Normal — the
/// setup the range-edit Vim tests (#700) build on.
fn continuous_vim_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = three_chapter_app(name); // alpha / beta / gamma
    app.vim = true;
    app.mode = Mode::Normal;
    app.on_key(ctrl('l')); // enter continuous view
    assert!(app.manuscript.is_some());
    (app, dir)
}

#[test]
fn continuous_vim_word_delete_edits_the_right_chapter() {
    // #700: `dw` in the continuous view is no longer refused — it routes through
    // the range-edit primitive and writes back to the owning chapter's file.
    let (mut app, dir) = continuous_vim_app("cont-dw");
    // Move into chapter 1 ("beta") and delete the word.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('w')));
    assert_eq!(app.edit_buffer().line(1), "", "beta was deleted");
    assert_ne!(
        app.message.as_deref(),
        Some("Editing commands aren't available in the continuous view — Ctrl-l to return."),
        "the operator was performed, not refused"
    );

    // Leaving persists just chapter 1's edit; the seams held.
    app.on_key(ctrl('l'));
    assert_eq!(fs::read_to_string(dir.join("c1.md")).unwrap(), "");
    assert_eq!(fs::read_to_string(dir.join("c0.md")).unwrap(), "alpha\n");
    assert_eq!(fs::read_to_string(dir.join("c2.md")).unwrap(), "gamma\n");
}

#[test]
fn continuous_vim_x_and_dollar_delete_within_a_chapter() {
    let (mut app, dir) = continuous_vim_app("cont-x");
    // Chapter 0 ("alpha"): `x` drops the first char, `$`... use `D`/`d$`.
    app.on_key(key(KeyCode::Char('x'))); // delete 'a' → "lpha"
    assert_eq!(app.edit_buffer().line(0), "lpha");
    app.on_key(key(KeyCode::Char('l'))); // move onto 'p'
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('$'))); // delete to line end → "l"
    assert_eq!(app.edit_buffer().line(0), "l");

    app.on_key(ctrl('l'));
    assert_eq!(fs::read_to_string(dir.join("c0.md")).unwrap(), "l\n");
    assert_eq!(fs::read_to_string(dir.join("c1.md")).unwrap(), "beta\n");
}

#[test]
fn continuous_vim_visual_delete_edits_the_right_chapter() {
    let (mut app, dir) = continuous_vim_app("cont-visual");
    // Chapter 2 ("gamma"): select the whole word charwise and delete it.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('v')));
    for _ in 0..4 {
        app.on_key(key(KeyCode::Char('l'))); // extend across "gamma"
    }
    app.on_key(key(KeyCode::Char('d')));
    assert_eq!(app.edit_buffer().line(2), "", "the selection was deleted");
    assert!(matches!(app.mode, Mode::Normal));

    app.on_key(ctrl('l'));
    assert_eq!(fs::read_to_string(dir.join("c2.md")).unwrap(), "");
    assert_eq!(fs::read_to_string(dir.join("c0.md")).unwrap(), "alpha\n");
}

#[test]
fn continuous_vim_change_leaves_a_seam_intact() {
    // `cc` on a middle chapter clears the line's content but keeps the chapter's
    // terminating newline — the seam holds even though the line is emptied.
    let (mut app, dir) = continuous_vim_app("cont-cc");
    app.on_key(key(KeyCode::Down)); // into chapter 1 ("beta")
    app.on_key(key(KeyCode::Char('c')));
    app.on_key(key(KeyCode::Char('c'))); // clear the line, enter Insert
    assert!(matches!(app.mode, Mode::Insert));
    type_str(&mut app, "new");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.edit_buffer().line(1), "new");

    app.on_key(ctrl('l'));
    assert_eq!(fs::read_to_string(dir.join("c1.md")).unwrap(), "new\n");
    // The neighbours never merged in.
    assert_eq!(fs::read_to_string(dir.join("c0.md")).unwrap(), "alpha\n");
    assert_eq!(fs::read_to_string(dir.join("c2.md")).unwrap(), "gamma\n");
}

#[test]
fn continuous_vim_undo_reverts_a_range_delete() {
    let (mut app, dir) = continuous_vim_app("cont-op-undo");
    app.on_key(key(KeyCode::Down)); // chapter 1 ("beta")
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('w'))); // delete "beta"
    assert_eq!(app.edit_buffer().line(1), "");
    app.on_key(key(KeyCode::Char('u'))); // Vim undo
    assert_eq!(
        app.edit_buffer().line(1),
        "beta",
        "the range delete was undone"
    );

    // Nothing dirty after the undo, so leaving writes the file back unchanged.
    app.on_key(ctrl('l'));
    assert_eq!(fs::read_to_string(dir.join("c1.md")).unwrap(), "beta\n");
}

#[test]
fn find_works_within_the_continuous_view() {
    // #700: opening find no longer drops back to single-chapter view; the search
    // runs across the loaded window's chapters.
    let (mut app, _dir) = three_chapter_app("cont-find");
    app.mode = Mode::Insert;
    app.on_key(ctrl('l')); // enter continuous view
    assert!(app.manuscript.is_some());

    app.on_key(ctrl('f')); // open find — must stay in the continuous view
    assert!(
        app.manuscript.is_some(),
        "find did not leave the continuous view"
    );
    type_str(&mut app, "gamma"); // a word only in chapter 2
    app.on_key(key(KeyCode::Enter)); // to the navigate stage

    // The cursor jumped to "gamma" in chapter 2 — a cross-chapter match found
    // without leaving the view.
    let cursor_line = app.edit_buffer().cursor.line;
    assert_eq!(app.edit_buffer().line(cursor_line), "gamma");
}

#[test]
fn find_replace_within_the_continuous_view_edits_the_right_chapter() {
    let (mut app, dir) = three_chapter_app("cont-replace");
    app.mode = Mode::Insert;
    app.on_key(ctrl('l'));

    app.on_key(ctrl('f'));
    type_str(&mut app, "beta");
    app.on_key(key(KeyCode::Enter)); // navigate stage
    app.on_key(key(KeyCode::Char('r'))); // promote to replace
    type_str(&mut app, "delta");
    app.on_key(key(KeyCode::Enter)); // start stepping
    app.on_key(key(KeyCode::Char('a'))); // replace all remaining

    assert!(
        app.manuscript.is_some(),
        "replace stayed in the continuous view"
    );
    app.on_key(ctrl('l')); // leave — saves
    assert_eq!(fs::read_to_string(dir.join("c1.md")).unwrap(), "delta\n");
    assert_eq!(fs::read_to_string(dir.join("c0.md")).unwrap(), "alpha\n");
    assert_eq!(fs::read_to_string(dir.join("c2.md")).unwrap(), "gamma\n");
}

#[test]
fn entering_continuous_flushes_the_single_chapter() {
    let (mut app, dir) = two_chapter_app("continuous-flush");
    // Edit chapter one in single view, then enter continuous without saving.
    app.on_key(key(KeyCode::Char('X')));
    assert!(app.buffer.dirty);

    app.on_key(ctrl('l')); // entering must flush the dirty single chapter
    assert!(fs::read_to_string(dir.join("one.md"))
        .unwrap()
        .starts_with('X'));
}

#[test]
fn ctrl_r_opens_the_report_and_any_key_closes_it() {
    let (mut app, _dir) = two_chapter_app("report");
    app.project.manifest.start_date = Some("2026-01-11".into());
    app.project.manifest.goals = Some(crate::project::Goals {
        words: 150_000,
        chapters: 60,
        words_per_page: 250,
        daily: None,
    });

    app.on_key(ctrl('r'));
    let report = app.overlay_lines().expect("report should be open");
    // It rendered the story line against the goals (styled rows → span text).
    let line_text = |l: &Line| {
        l.spans
            .iter()
            .map(|s| s.content.to_string())
            .collect::<String>()
    };
    assert!(
        report
            .iter()
            .any(|l| line_text(l).contains("/ 150,000  words")),
        "no story line"
    );

    app.on_key(key(KeyCode::Char('x'))); // any key closes it (no edit)
    assert!(app.overlay.is_none());
    assert!(
        !app.buffer.line(0).contains('x'),
        "the dismissing key must not edit"
    );
}

#[test]
fn idle_tallies_words_added_then_removed() {
    let (mut app, _dir) = two_chapter_app("tally"); // ch0 "first chapter" (2 words)
    type_str(&mut app, "new words "); // → "new words first chapter" (4 words): +2
    app.on_idle();
    assert_eq!(app.added, 2);
    assert_eq!(app.removed, 0);

    for _ in 0..10 {
        app.on_key(key(KeyCode::Backspace)); // delete "new words " back out
    }
    app.on_idle();
    assert_eq!(app.added, 2); // unchanged
    assert_eq!(app.removed, 2);
}

#[test]
fn an_edited_chapter_is_tallied_even_after_navigating_away() {
    // Edit ch0, then switch to ch1 before an idle tick. The tally must still book
    // ch0's delta — it tracks which chapters changed (#220), not just the active
    // one. (The single-chapter analogue of a continuous-view chapter paging out.)
    let (mut app, _dir) = two_chapter_app("tally-away"); // ch0 "first chapter" (2 words)
    type_str(&mut app, "new words "); // ch0 → +2, no idle yet
    app.on_key(ctrl('n')); // switch to ch1 (flushes ch0, recording its new count)
    assert_eq!(app.project.active_index(), 1);
    app.on_idle();
    assert_eq!(
        app.added, 2,
        "ch0's edit was booked though it's no longer active"
    );
    assert_eq!(app.removed, 0);
}

#[test]
fn deleting_a_chapter_books_moved_to_unused_not_removed() {
    let (mut app, _dir) = two_chapter_app("moved"); // ch1 "second chapter" (2 words)
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('y'))); // confirm delete
    assert_eq!(app.moved_to_unused, 2);
    assert_eq!(app.removed, 0);

    app.on_idle(); // the structural drop is not re-booked as removed
    assert_eq!(app.removed, 0);
}

#[test]
fn momentum_persists_and_resumes_across_a_reopen() {
    let (mut app, dir) = two_chapter_app("resume");
    app.added = 100;
    app.removed = 5;
    app.moved_to_unused = 3;
    app.persist_snapshot();

    // A fresh session over the same project resumes today's counters.
    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut reopened = App::new(buffer, project);
    reopened.load_history();
    assert_eq!(reopened.added, 100);
    assert_eq!(reopened.removed, 5);
    assert_eq!(reopened.moved_to_unused, 3);
}

/// A two-chapter project where each chapter is its own part ("Beginning",
/// "End"), for exercising per-part attribution.
fn parts_app(name: &str) -> (App, TempProject) {
    let dir = crate::testutil::scratch_dir("parts", name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\npart_label = \"part\"\n\n\
         [[parts]]\nname = \"Beginning\"\nchapters = [1, 1]\n\n\
         [[parts]]\nname = \"End\"\nchapters = [2, 2]\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    fs::write(dir.join("two.md"), "second chapter\n").unwrap();

    // Pinned like every other fixture (#428) — this one previously skipped the
    // config pins, a latent flakiness source.
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn edits_are_attributed_to_their_part() {
    let (mut app, _dir) = parts_app("attribute");
    // Type in chapter 0 (the "Beginning" part).
    type_str(&mut app, "alpha beta "); // +2 words in ch0
    app.on_idle();
    assert_eq!(app.part_added, vec![2, 0]);

    // Switch to chapter 1 (the "End" part) and type — switching flushes ch0,
    // so its cached count matches its baseline and isn't re-booked.
    app.on_key(ctrl('n'));
    type_str(&mut app, "gamma "); // +1 word in ch1
    app.on_idle();
    assert_eq!(app.part_added, vec![2, 1]);

    // The global counters equal the sums of the parts.
    assert_eq!(app.added, app.part_added.iter().sum::<usize>());
    assert_eq!(app.removed, 0);
}

#[test]
fn deleting_a_chapter_does_not_book_a_part_removal() {
    let (mut app, _dir) = parts_app("part-delete");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1 ("End")
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('y'))); // confirm delete
    app.on_idle();
    // Structural drop → moved_to_unused, not a per-part removal.
    assert_eq!(app.moved_to_unused, 2);
    assert_eq!(app.part_removed, vec![0, 0]);
}

#[test]
fn per_part_counters_persist_and_resume() {
    let (mut app, dir) = parts_app("part-resume");
    app.part_added = vec![40, 12];
    app.part_removed = vec![3, 1];
    app.persist_snapshot();

    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut reopened = App::new(buffer, project);
    reopened.load_history();
    assert_eq!(reopened.part_added, vec![40, 12]);
    assert_eq!(reopened.part_removed, vec![3, 1]);
}

#[test]
fn resume_ignores_per_part_counts_when_the_parts_config_changed() {
    // Persist a two-part day, then reopen with a one-part manifest: the
    // mismatched per-part counters are dropped rather than mis-applied.
    let (mut app, dir) = parts_app("part-mismatch");
    app.part_added = vec![40, 12];
    app.persist_snapshot();

    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[parts]]\nname = \"Whole\"\nchapters = [1, 2]\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut reopened = App::new(buffer, project);
    reopened.load_history();
    assert_eq!(reopened.part_added, vec![0]); // one part, fresh
}
