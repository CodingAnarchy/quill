//! Modal overlay render tests: the action palette scrolling to keep its
//! selection visible, the model-manager catalog, the modal backdrop dim + drop
//! shadow, and the import preview. Pure move (#792).

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};

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
    use crate::mode::ImportPreview;

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
    use crate::mode::ReviewImport;

    let mut app = app_with("The sea was grey.\n");
    app.mode = Mode::ReviewImport(Box::new(ReviewImport {
        source: std::path::PathBuf::from("/tmp/book-review.docx"),
        chapter: "one.md".into(),
        matched: vec![(12, 16, "vivid word".into())],
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
    use crate::annotation::{Live, Reply};
    use crate::mode::{AnnoShow, Mode};

    let mut app = app_with("The night was long.\n");
    app.annotations = vec![
        Live {
            chapter: "one.md".into(),
            start: 0,
            end: 3,
            note: "still thinking".into(),
            replies: Vec::new(),
            resolved: false,
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
        },
    ];
    app.mode = Mode::Annotations {
        selected: 0,
        filter: String::new(),
        typing: false,
        show: AnnoShow::All,
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
    assert!(
        screen.contains("a reply") && screen.contains("r resolve") && screen.contains("f open"),
        "the thread keys are advertised in the help line:\n{screen}"
    );
}
