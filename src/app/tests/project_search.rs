//! Manuscript-wide find & replace (#295): searching across every chapter,
//! jumping to a match in its chapter, and replacing across the whole project
//! (front matter left untouched).

use super::*;
use crate::mode::ProjectSearchStage;
use crossterm::event::KeyCode;

#[test]
fn project_search_finds_matches_across_every_chapter() {
    let (mut app, _dir) = two_chapter_app("psearch-find");
    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter));

    let ps = app.project_search();
    assert_eq!(ps.matches.len(), 2, "one match per chapter");
    assert_eq!(ps.matches[0].chapter, 0);
    assert_eq!(ps.matches[1].chapter, 1);
    assert_eq!(ps.chapter_span(), 2);
}

#[test]
fn a_failed_flush_aborts_the_search_instead_of_using_stale_ranges() {
    // Bug K: matches are computed against chapter bodies read from disk. If the
    // active chapter can't flush its pending edits first, its disk body is stale
    // while its live buffer has diverged — applying stale byte ranges to that buffer
    // can run an offset past its length and panic `Buffer::replace_byte_range`. So a
    // flush failure must abort the search, not proceed.
    let (mut app, _dir) = two_chapter_app("psearch-flush-fail");
    // Force the active buffer's save to fail: dirty, but with no path to write to.
    app.buffer.insert_char('x');
    assert!(app.buffer.dirty, "the active buffer has unsaved edits");
    app.buffer.path = None; // save() now errors with "no file path set"

    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter));

    // Still on the query stage (not showing results), with an explanatory message —
    // nothing was searched against stale content.
    match &app.mode {
        Mode::ProjectSearch(s) => assert!(
            matches!(s.stage, ProjectSearchStage::Query),
            "the search aborted on the query stage rather than listing results"
        ),
        _ => panic!("expected the search overlay to stay open on the query stage"),
    }
    assert!(
        app.message.as_deref().unwrap_or("").contains("aborted"),
        "the abort is surfaced: {:?}",
        app.message
    );
}

#[test]
fn no_matches_reports_and_closes() {
    let (mut app, _dir) = two_chapter_app("psearch-none");
    app.open_project_find();
    type_str(&mut app, "zzzznotfound");
    app.on_key(key(KeyCode::Enter));

    assert!(!matches!(app.mode, Mode::ProjectSearch(_)), "closed");
    assert!(
        app.message.as_deref().unwrap_or("").contains("No matches"),
        "{:?}",
        app.message
    );
}

#[test]
fn jumping_to_a_match_opens_its_chapter() {
    let (mut app, _dir) = two_chapter_app("psearch-jump");
    assert_eq!(app.project.active_index(), 0);

    app.open_project_find();
    type_str(&mut app, "second"); // only in chapter two
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Enter)); // jump to the selected match

    assert_eq!(
        app.project.active_index(),
        1,
        "switched to the match's chapter"
    );
    assert!(
        app.buffer.text().contains("second chapter"),
        "opened the right chapter"
    );
}

#[test]
fn replace_all_rewrites_every_chapter_on_disk() {
    let (mut app, dir) = two_chapter_app("psearch-replace-all");
    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Char('r'))); // → Replacement
    type_str(&mut app, "section");
    app.on_key(key(KeyCode::Enter)); // → Step
    app.on_key(key(KeyCode::Char('a'))); // → Confirm
    app.on_key(key(KeyCode::Char('y'))); // replace all

    assert!(
        app.message.as_deref().unwrap_or("").contains("2 chapters"),
        "{:?}",
        app.message
    );
    // The active chapter (one.md) is flushed; the other is written directly.
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "first section\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "second section\n"
    );
}

#[test]
fn replace_all_needs_confirmation() {
    let (mut app, dir) = two_chapter_app("psearch-confirm");
    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('r')));
    type_str(&mut app, "section");
    app.on_key(key(KeyCode::Enter)); // Step
    app.on_key(key(KeyCode::Char('a'))); // Confirm prompt
    app.on_key(key(KeyCode::Esc)); // decline — backs out to stepping

    assert!(
        matches!(app.mode, Mode::ProjectSearch(_)),
        "still open, nothing replaced"
    );
    // Files untouched (the non-active one is the unambiguous on-disk check).
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "second chapter\n"
    );
}

#[test]
fn replacing_one_match_leaves_the_others() {
    let (mut app, dir) = two_chapter_app("psearch-one");
    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Char('r'))); // → Replacement
    type_str(&mut app, "section");
    app.on_key(key(KeyCode::Enter)); // → Step, selected = match 0 (chapter one, active)
    app.on_key(key(KeyCode::Char('y'))); // replace just that one
    app.on_key(key(KeyCode::Enter)); // done

    // Only chapter two's match remains untouched on disk.
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "second chapter\n"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("Replaced 1"),
        "{:?}",
        app.message
    );
}

#[test]
fn replace_all_updates_the_active_chapter_on_disk_with_a_reference_pane_focused() {
    // Data-integrity guard (#1167 follow-up): a story-bible entry opened in a
    // split and focused makes `self.buffer` the bible, not the active chapter. A
    // project-wide replace touching the active chapter must still rewrite the
    // chapter's *file* (through the on-disk path), never the focused bible
    // buffer/file.
    let (mut app, dir) = two_chapter_app("psearch-replace-ref-pane");
    fs::create_dir_all(dir.join("bible")).unwrap();
    let bible = dir.join("bible").join("mara.md");
    fs::write(&bible, "# Mara\n\na sailor\n").unwrap();

    // Open the bible entry in a pane and focus it (`Ctrl-w w`).
    app.on_key(ctrl('g')); // chapter browser
    app.on_key(key(KeyCode::Char('f'))); // reference files
    app.on_key(key(KeyCode::Enter)); // open the (only) entry in a pane
    app.on_key(ctrl('w'));
    app.on_key(key(KeyCode::Char('w'))); // focus the bible pane
    assert!(
        !app.focus_shows_active_chapter(),
        "the focused pane is the bible, not the active chapter"
    );

    // Replace "chapter" → "section" across the whole manuscript.
    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Char('r'))); // → Replacement
    type_str(&mut app, "section");
    app.on_key(key(KeyCode::Enter)); // → Step
    app.on_key(key(KeyCode::Char('a'))); // → Confirm/Preview
    app.on_key(key(KeyCode::Char('y'))); // replace all

    // The active chapter's file was rewritten on disk, via the disk path — not
    // misrouted into the focused bible buffer.
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "first section\n",
        "the active chapter's file updated on disk"
    );
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "second section\n",
    );

    // The focused bible is untouched — buffer and file both unchanged.
    let focused = app.buffer.text();
    assert!(
        focused.contains("a sailor") && !focused.contains("section"),
        "the focused bible buffer is unchanged: {focused:?}"
    );
    assert_eq!(
        fs::read_to_string(&bible).unwrap(),
        "# Mara\n\na sailor\n",
        "the bible file on disk is untouched"
    );
}

#[test]
fn replace_refreshes_cached_word_counts() {
    let (mut app, _dir) = two_chapter_app("psearch-recount");
    // two.md (non-active) is "second chapter" — two words.
    assert_eq!(app.project.chapter_words(1), 2);

    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('r')));
    type_str(&mut app, "epic tale");
    app.on_key(key(KeyCode::Enter)); // Step
    app.on_key(key(KeyCode::Char('a'))); // Confirm
    app.on_key(key(KeyCode::Char('y'))); // replace all

    // "second epic tale" — three words; the non-active chapter's cache refreshed.
    assert_eq!(app.project.chapter_words(1), 3);
}

#[test]
fn front_matter_is_not_searched_or_replaced() {
    let (mut app, dir) = two_chapter_app("psearch-frontmatter");
    // Chapter two's front matter mentions the query term; its body mentions it once.
    fs::write(
        dir.join("two.md"),
        "---\npov: chapter\n---\nsecond chapter\n",
    )
    .unwrap();

    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter));
    // The front-matter "chapter" is not a match — one body match per chapter.
    assert_eq!(app.project_search().matches.len(), 2);

    app.on_key(key(KeyCode::Char('r')));
    type_str(&mut app, "scene");
    app.on_key(key(KeyCode::Enter)); // Step
    app.on_key(key(KeyCode::Char('a'))); // Confirm
    app.on_key(key(KeyCode::Char('y'))); // replace all

    let two = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(
        two.contains("pov: chapter"),
        "front matter preserved: {two}"
    );
    assert!(two.contains("second scene"), "body replaced: {two}");
}

// --- #570: case-sensitive and whole-word toggles, manuscript-wide ----------

/// A two-chapter fixture seeded with name-lookalike prose for the toggle tests:
/// each chapter mixes the character "Will", the verb "will", and "willing".
/// Built from scratch (not derived from `two_chapter_app`) so the active
/// chapter's buffer matches the seeded prose on open.
fn will_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "Will was willing.\n").unwrap();
    fs::write(dir.join("two.md"), "will he, Will?\n").unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn project_default_is_case_insensitive_substring() {
    // Both toggles off: "Will" matches Will/will and the "will" in "willing"
    // across both chapters — four substrings.
    let (mut app, _dir) = will_app("psearch-default");
    app.open_project_find();
    type_str(&mut app, "Will");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.project_search().matches.len(), 4);
}

#[test]
fn project_case_sensitive_toggle() {
    let (mut app, _dir) = will_app("psearch-case");
    app.open_project_find();
    type_str(&mut app, "Will");
    app.on_key(ctrl('c')); // case-sensitive, before running
    app.on_key(key(KeyCode::Enter));
    // Only the capitalized "Will" in each chapter (still substrings of nothing
    // else here): one per chapter.
    assert_eq!(app.project_search().matches.len(), 2);
}

#[test]
fn project_whole_word_toggle() {
    let (mut app, _dir) = will_app("psearch-word");
    app.open_project_find();
    type_str(&mut app, "will");
    app.on_key(ctrl('w')); // whole-word
    app.on_key(key(KeyCode::Enter));
    // "Will" + "will" + "Will" = three standalone words; the "will" inside
    // "willing" is excluded.
    assert_eq!(app.project_search().matches.len(), 3);
}

#[test]
fn project_case_sensitive_and_whole_word_together() {
    let (mut app, _dir) = will_app("psearch-both");
    app.open_project_find();
    type_str(&mut app, "Will");
    app.on_key(ctrl('c'));
    app.on_key(ctrl('w'));
    app.on_key(key(KeyCode::Enter));
    // Exact case AND standalone: the leading "Will" in chapter one and the
    // trailing "Will?" in chapter two — two matches.
    assert_eq!(app.project_search().matches.len(), 2);
}

#[test]
fn project_toggle_after_results_re_runs_the_search() {
    // Toggling a mode while results are listed updates the list in place.
    let (mut app, _dir) = will_app("psearch-retoggle");
    app.open_project_find();
    type_str(&mut app, "Will");
    app.on_key(key(KeyCode::Enter)); // Results: 4 substring matches
    assert_eq!(app.project_search().matches.len(), 4);
    app.on_key(ctrl('w')); // whole-word re-runs in place
    assert!(
        matches!(app.mode, Mode::ProjectSearch(_)),
        "overlay stays open"
    );
    assert_eq!(app.project_search().matches.len(), 3, "drops 'willing'");
}

#[test]
fn project_replace_honors_whole_word_and_case() {
    // The headline rename across the manuscript: "Will" → "Bill", leaving the
    // verb "will" and "willing" untouched in every chapter.
    let (mut app, dir) = will_app("psearch-replace-toggles");
    app.open_project_find();
    type_str(&mut app, "Will");
    app.on_key(ctrl('c')); // case-sensitive
    app.on_key(ctrl('w')); // whole-word
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Char('r'))); // → Replacement
    type_str(&mut app, "Bill");
    app.on_key(key(KeyCode::Enter)); // → Step
    app.on_key(key(KeyCode::Char('a'))); // → Confirm
    app.on_key(key(KeyCode::Char('y'))); // replace all

    let _ = app.flush_active();
    let one = fs::read_to_string(dir.join("one.md")).unwrap();
    let two = fs::read_to_string(dir.join("two.md")).unwrap();
    assert_eq!(one, "Bill was willing.\n", "only the name renamed: {one}");
    assert_eq!(two, "will he, Bill?\n", "only the name renamed: {two}");
}

// --- #891: regex mode, manuscript-wide -------------------------------------

/// A two-chapter fixture seeded with numbered chapter references, for the regex
/// tests: each chapter carries a `Chap. N` string a literal search can't target
/// as a class. Built from scratch so the active chapter's buffer matches on open.
fn chap_ref_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "See Chap. 3 for more.\n").unwrap();
    fs::write(dir.join("two.md"), "Back in Chap. 12 again.\n").unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn project_regex_matches_across_chapters() {
    let (mut app, _dir) = chap_ref_app("psearch-regex-find");
    app.open_project_find();
    type_str(&mut app, r"Chap\. \d+");
    app.on_key(ctrl('r')); // regex mode
    app.on_key(key(KeyCode::Enter));

    let ps = app.project_search();
    assert_eq!(ps.matches.len(), 2, "one Chap. N per chapter");
    assert_eq!(ps.matches[0].chapter, 0);
    assert_eq!(ps.matches[1].chapter, 1);
    assert_eq!(ps.chapter_span(), 2);
}

#[test]
fn project_invalid_regex_is_reported_not_a_panic() {
    let (mut app, _dir) = chap_ref_app("psearch-regex-invalid");
    app.open_project_find();
    type_str(&mut app, "Chap. ("); // unbalanced group
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Enter));

    // The overlay stays open on the query (not a panic, not a silent close), and
    // the error is surfaced.
    assert!(
        matches!(app.mode, Mode::ProjectSearch(_)),
        "overlay stays open to fix the pattern"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Invalid regex"),
        "{:?}",
        app.message
    );
}

#[test]
fn project_regex_replace_across_the_manuscript() {
    // Normalize the numbered references: `Chap. N` → `Chapter N`, capturing the
    // number with a backreference in the replacement.
    let (mut app, dir) = chap_ref_app("psearch-regex-replace");
    app.open_project_find();
    type_str(&mut app, r"Chap\. (\d+)");
    app.on_key(ctrl('r')); // regex mode
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Char('r'))); // → Replacement
    type_str(&mut app, "Chapter N"); // literal replacement (no capture expansion)
    app.on_key(key(KeyCode::Enter)); // → Step
    app.on_key(key(KeyCode::Char('a'))); // → Confirm
    app.on_key(key(KeyCode::Char('y'))); // replace all

    assert!(
        app.message.as_deref().unwrap_or("").contains("2 chapters"),
        "{:?}",
        app.message
    );
    let one = fs::read_to_string(dir.join("one.md")).unwrap();
    let two = fs::read_to_string(dir.join("two.md")).unwrap();
    assert_eq!(one, "See Chapter N for more.\n");
    assert_eq!(two, "Back in Chapter N again.\n");
}

#[test]
fn project_regex_composes_with_case_sensitivity() {
    let (mut app, _dir) = will_app("psearch-regex-case");
    app.open_project_find();
    type_str(&mut app, "will."); // regex `.` matches any char after "will"
    app.on_key(ctrl('r')); // regex
    app.on_key(ctrl('c')); // case-sensitive
    app.on_key(key(KeyCode::Enter));
    // Case-sensitive `will.`: "will " (one.md willing→"will" + "i"), "will " and
    // "will," in two.md — lowercase "will" followed by any char. The capital
    // "Will" is excluded by case. Chapter one: "willing" → "willi" matches once.
    // Chapter two: "will he" and nothing else lowercase-will. So 2 total.
    let n = app.project_search().matches.len();
    assert_eq!(n, 2, "lowercase will+any-char in each chapter");
}

// --- #977: grouped results pane --------------------------------------------

/// A three-chapter fixture with an uneven distribution of the query term "mist":
/// chapter one has it twice, chapter two once, chapter three not at all — so the
/// grouped counts (2, 1) and the two-chapter span are unambiguous.
fn mist_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n\n\
         [[chapters]]\ntitle = \"Three\"\nfile = \"three.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "mist over the mist.\n").unwrap();
    fs::write(dir.join("two.md"), "a lone mist.\n").unwrap();
    fs::write(dir.join("three.md"), "clear skies.\n").unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

// --- #1014: replace-all preview --------------------------------------------

/// Drive the replace flow up to the preview stage: search `query`, start a
/// replace, type `replacement`, then open the replace-all preview (`a`).
fn open_replace_preview(app: &mut App, query: &str, replacement: &str) {
    app.open_project_find();
    type_str(app, query);
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Char('r'))); // → Replacement
    type_str(app, replacement);
    app.on_key(key(KeyCode::Enter)); // → Step
    app.on_key(key(KeyCode::Char('a'))); // → Preview
}

#[test]
fn preview_reports_per_chapter_counts_with_context() {
    // The uneven "mist" fixture: chapter one has two, chapter two one, chapter
    // three none — so the per-chapter breakdown is unambiguous.
    let (mut app, _dir) = mist_app("psearch-preview-counts");
    open_replace_preview(&mut app, "mist", "fog");

    let ps = app.project_search();
    assert!(
        matches!(ps.stage, ProjectSearchStage::Preview),
        "on the preview stage before any write"
    );
    let rows = ps.preview_rows();
    assert_eq!(rows.len(), 2, "only the two chapters with matches");
    assert_eq!(rows[0].title, "One");
    assert_eq!(rows[0].count, 2, "chapter one has two matches");
    assert!(
        rows[0].context.contains("mist"),
        "the row carries a context snippet: {:?}",
        rows[0].context
    );
    assert_eq!(rows[1].title, "Two");
    assert_eq!(rows[1].count, 1, "chapter two has one match");
    assert!(rows[1].context.contains("mist"), "{:?}", rows[1].context);
}

#[test]
fn cancelling_the_preview_changes_nothing() {
    let (mut app, dir) = mist_app("psearch-preview-cancel");
    open_replace_preview(&mut app, "mist", "fog");
    // Esc from the preview backs out to stepping, writing nothing.
    app.on_key(key(KeyCode::Esc));

    assert!(
        matches!(app.project_search().stage, ProjectSearchStage::Step),
        "cancel returns to the step stage, overlay still open"
    );
    // Every chapter is byte-for-byte what it was — the sweep never ran.
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "mist over the mist.\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "a lone mist.\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("three.md")).unwrap(),
        "clear skies.\n"
    );
}

#[test]
fn confirming_the_preview_applies_exactly_the_previewed_matches() {
    let (mut app, dir) = mist_app("psearch-preview-confirm");
    open_replace_preview(&mut app, "mist", "fog");
    // Snapshot exactly what the preview promised: total, per-chapter counts.
    let previewed_total: usize = app
        .project_search()
        .preview_rows()
        .iter()
        .map(|r| r.count)
        .sum();
    assert_eq!(previewed_total, 3, "the preview showed 3 matches");

    app.on_key(key(KeyCode::Char('y'))); // confirm the sweep

    // The message reflects exactly the previewed total and chapter span.
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Replaced 3 occurrences across 2 chapters"),
        "{:?}",
        app.message
    );
    // Every previewed match — and only those — was rewritten; the match-free
    // chapter is untouched.
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "fog over the fog.\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "a lone fog.\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("three.md")).unwrap(),
        "clear skies.\n",
        "the match-free chapter is left alone"
    );
}

#[test]
fn preview_confirms_on_enter_too() {
    // Enter is a confirm alias, so the author isn't forced to reach for `y`.
    let (mut app, dir) = mist_app("psearch-preview-enter");
    open_replace_preview(&mut app, "mist", "fog");
    app.on_key(key(KeyCode::Enter));

    assert!(
        app.message.as_deref().unwrap_or("").contains("Replaced 3"),
        "{:?}",
        app.message
    );
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "a lone fog.\n"
    );
}

#[test]
fn grouped_view_reports_per_chapter_counts_and_total() {
    let (mut app, _dir) = mist_app("psearch-grouped-counts");
    app.open_project_find();
    type_str(&mut app, "mist");
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Char('g'))); // → Grouped

    let ps = app.project_search();
    assert!(matches!(ps.stage, ProjectSearchStage::Grouped));
    // Total: 3 matches across 2 chapters (the third has none).
    assert_eq!(ps.matches.len(), 3, "3 matches total");
    assert_eq!(ps.chapter_span(), 2, "across 2 chapters");

    let groups = ps.grouped();
    assert_eq!(groups.len(), 2, "only the two chapters with matches");
    assert_eq!(groups[0].chapter, 0);
    assert_eq!(groups[0].count, 2, "chapter one has two");
    assert_eq!(groups[1].chapter, 1);
    assert_eq!(groups[1].count, 1, "chapter two has one");
}

#[test]
fn grouped_view_jumps_to_the_selected_chapters_first_match() {
    let (mut app, _dir) = mist_app("psearch-grouped-jump");
    assert_eq!(app.project.active_index(), 0);

    app.open_project_find();
    type_str(&mut app, "mist");
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Char('g'))); // → Grouped
    app.on_key(key(KeyCode::Down)); // select chapter two's group
    app.on_key(key(KeyCode::Enter)); // jump to its first match

    assert_eq!(
        app.project.active_index(),
        1,
        "jumped to the selected chapter"
    );
    assert!(
        app.buffer.text().contains("a lone mist"),
        "opened chapter two"
    );
}

#[test]
fn grouped_view_toggles_back_to_the_flat_list() {
    let (mut app, _dir) = mist_app("psearch-grouped-toggle");
    app.open_project_find();
    type_str(&mut app, "mist");
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Char('g'))); // → Grouped
    assert!(matches!(
        app.project_search().stage,
        ProjectSearchStage::Grouped
    ));
    app.on_key(key(KeyCode::Char('g'))); // back → Results
    assert!(matches!(
        app.project_search().stage,
        ProjectSearchStage::Results
    ));
    // The flat step-through still works from Results (n advances the match).
    app.on_key(key(KeyCode::Char('n')));
    assert_eq!(app.project_search().selected, 1, "n still steps matches");
}

#[test]
fn grouped_view_options_reshape_the_groups() {
    // Whole-word toggled on from the grouped pane re-runs the search and the
    // grouped counts track it (#570/#891 options apply to the grouped results).
    let (mut app, _dir) = will_app("psearch-grouped-options");
    app.open_project_find();
    type_str(&mut app, "Will");
    app.on_key(key(KeyCode::Enter)); // Results: 4 substring matches
    app.on_key(key(KeyCode::Char('g'))); // → Grouped
                                         // Before: "Will"/"will"(in willing) in one.md = 2; "will"/"Will" in two.md = 2.
    let before = app.project_search().grouped();
    assert_eq!(before.iter().map(|g| g.count).sum::<usize>(), 4);
    assert_eq!(before[0].count, 2);
    assert_eq!(before[1].count, 2);

    app.on_key(ctrl('w')); // whole-word, re-runs in place
    assert!(
        matches!(app.project_search().stage, ProjectSearchStage::Grouped),
        "stays in the grouped pane"
    );
    let after = app.project_search().grouped();
    // Whole-word drops the "will" inside "willing": one.md → 1 ("Will"),
    // two.md → 2 ("will", "Will").
    assert_eq!(after.iter().map(|g| g.count).sum::<usize>(), 3);
    assert_eq!(after[0].count, 1, "willing no longer counts");
    assert_eq!(after[1].count, 2);
}
