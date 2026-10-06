//! Modal overlay render tests: the action palette scrolling to keep its
//! selection visible, the model-manager catalog, the modal backdrop dim + drop
//! shadow, and the import preview. Pure move (#792).

use super::*;
use crate::annotation::{Live, Reply};
use crate::app::BackupEntry;
use crate::mode::{
    AnnoShow, AnnoSort, BackupBrowser, ImportPreview, Mode, ProjectMatch, ProjectSearch,
    ProjectSearchStage, QuickSwitch, QuickTarget, ReviewImport,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::time::{Duration, SystemTime};

#[test]
fn a_long_palette_scrolls_to_keep_the_selection_visible() {
    // The Ctrl-K action palette has ~50 entries; on a short terminal it overflows.
    let mut app = app_with("text\n");
    app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert!(matches!(app.mode, Mode::Palette(_)), "palette open");
    // Drive the selection well past a short viewport.
    for _ in 0..30 {
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    let screen = render(&app, 100, 16);

    let Mode::Palette(p) = &app.mode else {
        unreachable!()
    };
    // `selected` indexes the visible rows (headers + items); resolve its label.
    let rows = p.rows();
    let selected = match &rows[p.selected] {
        crate::mode::PaletteRow::Header { group, .. } => group.to_string(),
        crate::mode::PaletteRow::Item(i) => p.items[*i].label.clone(),
    };
    // The selected row scrolled into view; the first item ("Save") scrolled off.
    assert!(
        screen.contains(&selected),
        "selected row '{selected}' is visible:\n{screen}"
    );
    assert!(
        !screen.contains("Save"),
        "the first item scrolled off the top:\n{screen}"
    );
}

#[test]
fn the_action_palette_shows_a_shortcut_for_actions_that_have_one() {
    // The run-action palette doubles as a live cheat sheet (#1103): an action with a
    // dedicated key binding shows it beside the label, and a palette-only action
    // shows none. Expand every group so the items render.
    let mut app = app_with("text\n");
    app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    let Mode::Palette(p) = &mut app.mode else {
        panic!("Ctrl-K opens the run-action palette");
    };
    p.collapsed.clear();
    let screen = render(&app, 120, 50);

    // Save has a binding (Ctrl-S) — it renders as a bracketed key hint on that row.
    assert!(
        screen.contains("Save") && screen.contains("[Ctrl-S]"),
        "the Save row shows its Ctrl-S shortcut:\n{screen}"
    );
    // A palette-only action (Rename project) has no binding, so no bracket follows it.
    let rename_line = screen
        .lines()
        .find(|l| l.contains("Rename project"))
        .unwrap_or_default();
    assert!(
        !rename_line.contains('['),
        "a binding-less action shows no shortcut: {rename_line:?}"
    );
}

#[test]
fn the_model_manager_lists_the_catalog_with_characteristics() {
    let mut app = app_with("text\n");
    app.mode = Mode::ModelManager { selected: 0 };
    let screen = render(&app, 100, 24);
    assert!(screen.contains("Local AI models"), "{screen}");
    // Models appear as the Qwen3 dense family (#789) with their context window.
    assert!(
        screen.contains("Qwen3-8B"),
        "the catalog is listed:\n{screen}"
    );
    // Qwen3's native window is 40k across the family (#791) — no 1M tier.
    assert!(
        screen.contains("40k ctx"),
        "the native context window shows:\n{screen}"
    );
    // The mixed-quant catalog surfaces the quant (Q8_0 for the smallest, #789).
    assert!(screen.contains("Q8_0"), "the quant is shown:\n{screen}");
    assert!(
        screen.contains("Largest"),
        "descriptions are shown:\n{screen}"
    );
}

#[test]
fn an_open_modal_dims_the_editor_and_casts_a_shadow() {
    let mut app = app_with("# Heading one\n\nThe night was dark and long.\n");

    let render = |app: &App| {
        let mut t = Terminal::new(TestBackend::new(80, 20)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        t.backend().buffer().clone()
    };

    // No modal open: nothing behind is dimmed.
    let plain = render(&app);
    let plain_dim = (0..80u16)
        .flat_map(|x| (0..20u16).map(move |y| (x, y)))
        .filter(|&(x, y)| plain[(x, y)].modifier.contains(Modifier::DIM))
        .count();
    assert_eq!(plain_dim, 0, "nothing is dimmed without a modal");

    // Palette open: the editor behind dims, and the box casts a shadow.
    app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert!(
        matches!(app.mode, Mode::Palette(_)),
        "Ctrl-K opened the palette"
    );
    let buf = render(&app);
    let mut dimmed = 0usize;
    let mut shadow = false;
    for y in 0..20u16 {
        for x in 0..80u16 {
            if buf[(x, y)].modifier.contains(Modifier::DIM) {
                dimmed += 1;
            }
            if buf[(x, y)].symbol() == "▓" {
                shadow = true;
            }
        }
    }
    assert!(dimmed > 0, "the editor behind the modal is dimmed");
    assert!(shadow, "the modal casts a drop shadow");
}

#[test]
fn import_preview_lists_the_chapters_count_and_level() {
    // The import preview (#702) shows the split before writing: the chapter
    // count, the guessed heading level, the book title, and the chapter titles.
    let mut app = app_with("body\n");
    app.mode = Mode::ImportPreview(Box::new(ImportPreview {
        source: std::path::PathBuf::from("/tmp/book.md"),
        dest: std::path::PathBuf::from("/tmp/proj"),
        markdown: Some(String::new()),
        title: Some("The Long Road".into()),
        author: Some("A. Writer".into()),
        chapters: vec![
            ("One".into(), "a\n".into()),
            ("Two".into(), "b\n".into()),
            ("Three".into(), "c\n".into()),
        ],
        level: Some(2),
        levels: vec![1, 2, 3],
        warning: String::new(),
        scroll: 0,
    }));

    let screen = render(&app, 80, 24);
    assert!(
        screen.contains("3 chapters will be created"),
        "the chapter count is shown before writing:\n{screen}"
    );
    assert!(
        screen.contains("heading level 2"),
        "the guessed split level is shown:\n{screen}"
    );
    assert!(
        screen.contains("The Long Road") && screen.contains("A. Writer"),
        "the mined title/author are shown:\n{screen}"
    );
    for title in ["One", "Two", "Three"] {
        assert!(
            screen.contains(title),
            "chapter {title} is listed:\n{screen}"
        );
    }
    assert!(
        screen.contains("enter import") && screen.contains("esc cancel"),
        "the confirm/cancel affordance is shown:\n{screen}"
    );
}

#[test]
fn review_import_lists_matches_and_dropped_comments() {
    // The review-import preview (#886) shows what the reviewer's comments resolved
    // to: the matches that will become annotations and the unmatched ones (the
    // prose drifted), reported rather than silently dropped.
    let mut app = app_with("The sea was grey.\n");
    app.mode = Mode::ReviewImport(Box::new(ReviewImport {
        source: std::path::PathBuf::from("/tmp/book-review.docx"),
        chapter: "one.md".into(),
        matched: vec![(12, 16, "vivid word".into(), None)],
        dropped: vec![("gray waves".into(), "check this".into())],
        scroll: 0,
    }));

    let screen = render(&app, 80, 24);
    assert!(
        screen.contains("1 comment will be added as annotations"),
        "the match count is shown before merging:\n{screen}"
    );
    assert!(
        screen.contains("1 unmatched"),
        "the dropped-with-reason count is surfaced:\n{screen}"
    );
    assert!(
        screen.contains("vivid word"),
        "the matched note is listed:\n{screen}"
    );
    assert!(
        screen.contains("check this") && screen.contains("gray waves"),
        "the dropped comment and its phrase are reported:\n{screen}"
    );
    assert!(
        screen.contains("enter merge") && screen.contains("esc cancel"),
        "the merge/cancel affordance is shown:\n{screen}"
    );
}

#[test]
fn the_annotation_list_shows_threads_and_the_reply_resolve_keys() {
    // The review list (#302/#889) renders a resolved note distinctly (a ✓ + a reply
    // count), and its help line advertises the reply / resolve / filter keys.
    let mut app = app_with("The night was long.\n");
    app.annotations = vec![
        Live {
            chapter: "one.md".into(),
            start: 0,
            end: 3,
            note: "still thinking".into(),
            replies: Vec::new(),
            resolved: false,
            reviewer: None,
        },
        Live {
            chapter: "one.md".into(),
            start: 4,
            end: 9,
            note: "fixed the tense".into(),
            replies: vec![Reply {
                author: "author".into(),
                text: "done".into(),
            }],
            resolved: true,
            reviewer: None,
        },
    ];
    app.mode = Mode::Annotations {
        selected: 0,
        filter: String::new(),
        typing: false,
        show: AnnoShow::All,
        sort: AnnoSort::default(),
        reviewer: None,
    };

    let screen = render(&app, 90, 20);
    assert!(
        screen.contains("still thinking") && screen.contains("fixed the tense"),
        "both notes are listed:\n{screen}"
    );
    assert!(
        screen.contains("✓ fixed the tense"),
        "the resolved thread carries a check:\n{screen}"
    );
    assert!(
        screen.contains("(1 reply)"),
        "the reply count shows on the thread:\n{screen}"
    );
    // The thread keys and the triage keys (#933) are advertised in the help line.
    // The line wraps in this width, so check for tokens that don't straddle a wrap.
    assert!(
        screen.contains("reply")
            && screen.contains("resolve")
            && screen.contains("f open")
            && screen.contains("sort")
            && screen.contains("reviewer")
            && screen.contains("next/prev open"),
        "the thread + triage keys are advertised in the help line:\n{screen}"
    );
}

#[test]
fn the_backup_browser_lists_dates_sizes_and_the_restore_key() {
    // The backup browser (#934) lists each archive's date-ish label over its size
    // and name, with the restore/close keys in the help line.
    let mut app = app_with("body\n");
    let entry = |name: &str, size: u64, ago_secs: u64| BackupEntry {
        name: name.into(),
        path: std::path::PathBuf::from(name),
        size,
        modified: SystemTime::now() - Duration::from_secs(ago_secs),
    };
    app.mode = Mode::BackupBrowser(Box::new(BackupBrowser {
        entries: vec![
            entry("Book-2000-01-03-000000.zip", 3 * 1024 * 1024, 3600),
            entry("Book-2000-01-01-000000.zip", 2048, 3 * 24 * 3600),
        ],
        selected: 0,
    }));

    let screen = render(&app, 90, 20);
    assert!(
        screen.contains("Backups (2)"),
        "titled with the count:\n{screen}"
    );
    assert!(
        screen.contains("3.0 MB") && screen.contains("2.0 KB"),
        "human sizes shown:\n{screen}"
    );
    assert!(
        screen.contains("Book-2000-01-03-000000.zip"),
        "the archive name is listed:\n{screen}"
    );
    assert!(
        screen.contains("enter restore") && screen.contains("esc close"),
        "the restore/close keys are advertised:\n{screen}"
    );
}

#[test]
fn the_quick_switcher_lists_chapters_and_scenes_with_the_query_and_count() {
    // The fuzzy quick-switcher (#976) lists chapter and scene rows, marking scenes
    // with the ¶ tag and the chapter as dimmed context; the help line echoes the
    // typed query and the match count.
    let target = |title: &str, chapter_title: &str, is_scene: bool| QuickTarget {
        chapter: 0,
        byte: 0,
        title: title.into(),
        chapter_title: chapter_title.into(),
        haystack: format!("{title} {chapter_title}").to_lowercase(),
        is_scene,
    };
    let mut app = app_with("body\n");
    let mut switch = QuickSwitch::new(vec![
        target("Chapter One", "", false),
        target("The Arrival", "Chapter One", true),
        target("The Departure", "Chapter One", true),
    ]);
    switch.query = "the".into();
    app.mode = Mode::QuickSwitch(Box::new(switch));

    let screen = render(&app, 90, 20);
    assert!(screen.contains(" Go to "), "titled 'Go to':\n{screen}");
    assert!(
        screen.contains("The Arrival") && screen.contains("The Departure"),
        "the matching scene rows are listed:\n{screen}"
    );
    // The "the" query filtered to two scenes; only two rows show.
    assert!(
        screen.contains("2 matches"),
        "the chapter row was filtered out, leaving 2 scene rows:\n{screen}"
    );
    assert!(
        screen.contains("¶ The Arrival"),
        "scene rows carry the ¶ scene tag:\n{screen}"
    );
    assert!(
        screen.contains("the") && screen.contains("2 matches"),
        "the help line echoes the query and count:\n{screen}"
    );
}

#[test]
fn the_grouped_results_pane_shows_per_chapter_counts_and_a_total() {
    // The grouped project-find pane (#977): one row per chapter with its match
    // count, over a total ("N matches across M chapters").
    let mut app = app_with_chapters(&["Alpha", "Beta"], "body\n");
    let mut ps = ProjectSearch::new();
    ps.query = "mist".into();
    ps.stage = ProjectSearchStage::Grouped;
    let m = |chapter: usize, title: &str| ProjectMatch {
        chapter,
        start: 0,
        end: 4,
        title: title.into(),
        context: "the mist".into(),
    };
    // Two matches in Alpha, one in Beta: 3 across 2 chapters.
    ps.matches = vec![m(0, "Alpha"), m(0, "Alpha"), m(1, "Beta")];
    app.mode = Mode::ProjectSearch(Box::new(ps));

    let screen = render(&app, 100, 16);
    assert!(
        screen.contains("3 matches across 2 chapters"),
        "the total is shown:\n{screen}"
    );
    assert!(
        screen.contains("Alpha") && screen.contains("2 matches"),
        "the per-chapter count is shown:\n{screen}"
    );
    assert!(
        screen.contains("Beta") && screen.contains("1 match"),
        "the singular count is shown:\n{screen}"
    );
    assert!(
        screen.contains("enter jump") && screen.contains("g list"),
        "the jump/toggle keys are advertised:\n{screen}"
    );
}

#[test]
fn the_replace_all_preview_shows_a_per_chapter_breakdown_with_context() {
    // The replace-all preview (#1014): before the sweep, one row per affected
    // chapter with its match count and a context snippet, over the total and the
    // confirm/cancel keys.
    let mut app = app_with_chapters(&["Alpha", "Beta"], "body\n");
    let mut ps = ProjectSearch::new();
    ps.query = "Kate".into();
    ps.replacement = "Katherine".into();
    ps.stage = ProjectSearchStage::Preview;
    let m = |chapter: usize, title: &str, context: &str| ProjectMatch {
        chapter,
        start: 0,
        end: 4,
        title: title.into(),
        context: context.into(),
    };
    ps.matches = vec![
        m(0, "Alpha", "Kate opened the door"),
        m(0, "Alpha", "then Kate left"),
        m(1, "Beta", "Kate again"),
    ];
    app.mode = Mode::ProjectSearch(Box::new(ps));

    let screen = render(&app, 100, 16);
    assert!(
        screen.contains("Replace all 3 matches across 2 chapters"),
        "the total is shown:\n{screen}"
    );
    assert!(
        screen.contains("'Kate' → 'Katherine'") && screen.contains("y/enter confirm"),
        "the query/replacement and confirm keys are shown:\n{screen}"
    );
    assert!(
        screen.contains("Alpha") && screen.contains("2 matches"),
        "the per-chapter count is shown:\n{screen}"
    );
    assert!(
        screen.contains("Beta") && screen.contains("1 match"),
        "the singular count is shown:\n{screen}"
    );
    assert!(
        screen.contains("Kate opened the door"),
        "the first match's context snippet is shown:\n{screen}"
    );
}

/// Open the annotation prompt on the opening word ("Night") and type `note` into it.
fn annotate_with(app: &mut App, note: &str) {
    app.start_annotation_range(0, 5);
    assert!(matches!(app.mode, Mode::Prompt(_)), "the note prompt opens");
    for c in note.chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

#[test]
fn a_long_annotation_note_wraps_instead_of_running_off_screen() {
    // A note longer than the status line used to be clipped at the right edge, so
    // the author couldn't see what they were typing. It now word-wraps upward.
    let mut app = app_with("Night fell.\n");
    let words: Vec<String> = (1..=20).map(|i| format!("word{i}")).collect();
    annotate_with(&mut app, &words.join(" "));
    let mut terminal = Terminal::new(TestBackend::new(40, 16)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap();
    let screen = render(&app, 40, 16);

    for w in &words {
        assert!(screen.contains(w), "'{w}' is visible:\n{screen}");
    }
    let rows: Vec<&str> = screen.lines().collect();
    assert!(
        rows[15].contains("word20"),
        "the note ends on the bottom row"
    );
    assert!(
        rows[11].starts_with("Note on"),
        "the label heads the wrapped note:\n{screen}"
    );
    // The caret follows the end of the text onto the last wrapped row.
    let pos = terminal.get_cursor_position().unwrap();
    assert_eq!(pos.y, 15);
    assert_eq!(usize::from(pos.x), rows[15].trim_end().len());
    // The wrapped rows stay legible; the editor above them dims.
    let buf = terminal.backend().buffer().clone();
    assert!(!buf[(0, 11)].modifier.contains(Modifier::DIM));
    assert!(buf[(1, 1)].modifier.contains(Modifier::DIM));
}

#[test]
fn an_overlong_annotation_note_scrolls_to_keep_the_caret_in_view() {
    // Past the row cap the prompt stops growing and scrolls: the latest text and
    // the caret stay on screen, the opening rows scroll off the top.
    let mut app = app_with("Night fell.\n");
    let words: Vec<String> = (1..=80).map(|i| format!("w{i:02}")).collect();
    annotate_with(&mut app, &words.join(" "));
    let screen = render(&app, 30, 20);

    assert!(
        screen.contains("w80"),
        "the end of the note is visible:\n{screen}"
    );
    assert!(
        !screen.contains("Note on"),
        "the label scrolled off:\n{screen}"
    );
    let note_rows = screen.lines().filter(|l| l.starts_with('w')).count();
    assert_eq!(note_rows, 8, "the prompt stops at its row cap:\n{screen}");
    assert!(
        screen.contains("Night fell"),
        "the editor stays above it:\n{screen}"
    );
}

#[test]
fn a_short_prompt_stays_on_the_status_line() {
    let mut app = app_with("Night fell.\n");
    annotate_with(&mut app, "check this");
    let screen = render(&app, 60, 10);
    let rows: Vec<&str> = screen.lines().collect();
    assert!(rows[9].contains("Note on") && rows[9].contains("check this"));
    assert!(rows[8].trim().is_empty(), "no extra row:\n{screen}");
}
