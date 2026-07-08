//! The motif / character co-occurrence report (#922): over sample bible data on
//! disk (rendered by the real character and theme writers), the report surfaces
//! the expected co-occurrence pairs and a notable absence, and jumping from a row
//! reaches the right chapter. Hermetic — no network, no real AI (#531).

use super::*;

use std::fmt::Write;

use crate::ai::character::{ArcBeat, Character};
use crate::ai::theme::{Motif, Theme as ThemeBible};
use crate::ai::{bible, character, theme as ai_theme};
use crate::mode::{CoOccurrenceRow, Mode};

/// A five-chapter project (One…Five) with a `bible/` seeded from the given
/// characters and motifs, rendered exactly as the trackers write them, so the
/// report reads real on-disk bible data.
fn cooccurrence_app(name: &str, chars: &[Character], motifs: &[Motif]) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    let mut manifest = String::from("title = \"Book\"\n\n");
    for title in ["One", "Two", "Three", "Four", "Five"] {
        let file = format!("{}.md", title.to_lowercase());
        write!(
            manifest,
            "[[chapters]]\ntitle = \"{title}\"\nfile = \"{file}\"\n\n"
        )
        .unwrap();
        fs::write(dir.join(&file), format!("{title} body\n")).unwrap();
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();

    let char_dir = dir.join("bible").join("characters");
    fs::create_dir_all(&char_dir).unwrap();
    for c in chars {
        let file = char_dir.join(format!("{}.md", bible::slug(&c.name)));
        fs::write(
            file,
            character::render_profile(c, "## Notes\n", "2026-06-19"),
        )
        .unwrap();
    }
    if !motifs.is_empty() {
        let theme = ThemeBible {
            motifs: motifs.to_vec(),
            findings: Vec::new(),
        };
        fs::write(
            dir.join("bible").join("theme.md"),
            ai_theme::render(&theme, "## Notes\n", "2026-06-19"),
        )
        .unwrap();
    }

    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

/// A character whose arc beats name the given chapters (the appearance data the
/// report reads).
fn character_in(name: &str, chapters: &[&str]) -> Character {
    Character {
        name: name.into(),
        arc: chapters
            .iter()
            .map(|c| ArcBeat {
                chapter: (*c).into(),
                state: "present".into(),
            })
            .collect(),
        ..Character::default()
    }
}

/// A motif that appears in the given chapters.
fn motif_in(name: &str, chapters: &[&str]) -> Motif {
    Motif {
        name: name.into(),
        status: "recurring".into(),
        appearances: chapters.iter().map(|c| (*c).to_string()).collect(),
        ..Motif::default()
    }
}

fn rows(app: &App) -> &[CoOccurrenceRow] {
    match &app.mode {
        Mode::CoOccurrence(c) => &c.rows,
        _ => panic!("expected the co-occurrence report to be open"),
    }
}

#[test]
fn report_lists_the_expected_clusters_and_a_notable_absence() {
    // Mara & Tomas share the opening two chapters (a tight pair). The storm motif
    // opens in One, then drops out across Two–Three–Four before returning in Five:
    // a notable (three-chapter) absence.
    let chars = vec![
        character_in("Mara", &["One", "Two"]),
        character_in("Tomas", &["One", "Two"]),
    ];
    let motifs = vec![motif_in("Storm", &["One", "Five"])];
    let (mut app, _dir) = cooccurrence_app("cooccur-report", &chars, &motifs);

    app.open_cooccurrence();
    let rows = rows(&app);

    // The Mara + Tomas cluster is present (a character↔character pair over the two
    // shared chapters).
    let pair = rows.iter().find_map(|r| match r {
        CoOccurrenceRow::Pair { line, .. } => Some(line.clone()),
        _ => None,
    });
    let pair = pair.expect("a co-occurring pair");
    assert!(pair.contains("Mara") && pair.contains("Tomas"), "{pair}");

    // The storm's mid-book gap (Two–Three–Four) is the notable absence.
    let absence = rows.iter().find_map(|r| match r {
        CoOccurrenceRow::Absence { line, chapter } => Some((line.clone(), chapter.clone())),
        _ => None,
    });
    let (line, chapter) = absence.expect("a notable absence");
    assert!(line.contains("Storm"), "{line}");
    assert!(
        line.contains("Two") && line.contains("Three") && line.contains("Four"),
        "{line}"
    );
    assert_eq!(chapter, "Two", "jumps to the first missing chapter");
}

#[test]
fn jumping_from_a_row_reaches_the_right_chapter() {
    let motifs = vec![motif_in("Storm", &["One", "Five"])];
    let chars = vec![
        character_in("Mara", &["One", "Five"]),
        character_in("Ada", &["One", "Five"]),
    ];
    let (mut app, _dir) = cooccurrence_app("cooccur-jump", &chars, &motifs);
    // Start away from chapter One so the jump is observable.
    app.activate_chapter(2);
    assert_eq!(app.project.active_title(), Some("Three"));

    app.open_cooccurrence();
    // Select the first jumpable row (a Mara+Ada / Mara+Storm pair, shared in One)
    // and press Enter.
    app.on_key_cooccurrence(key(KeyCode::Enter));
    assert_eq!(
        app.project.active_title(),
        Some("One"),
        "Enter jumps to the pair's shared chapter"
    );
    assert!(
        !matches!(app.mode, Mode::CoOccurrence(_)),
        "the report closes on jump"
    );
}

#[test]
fn reports_when_there_is_no_bible() {
    let (mut app, _dir) = two_chapter_app("cooccur-no-bible");
    app.open_cooccurrence();
    assert!(
        !matches!(app.mode, Mode::CoOccurrence(_)),
        "no overlay without a bible"
    );
    assert!(app.message.as_deref().unwrap_or_default().contains("bible"));
}

#[test]
fn saving_writes_a_report_to_export() {
    let motifs = vec![
        motif_in("Mirrors", &["One", "Two"]),
        motif_in("Locket", &["One", "Two"]),
    ];
    let (mut app, dir) = cooccurrence_app("cooccur-save", &[], &motifs);
    app.open_cooccurrence();
    app.on_key_cooccurrence(key(KeyCode::Char('s')));

    let export = dir.join("export");
    let saved = fs::read_dir(&export)
        .unwrap()
        .flatten()
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("co-occurrence-")
        })
        .expect("a co-occurrence report was written");
    let body = fs::read_to_string(saved.path()).unwrap();
    assert!(body.contains("# Motif & character co-occurrence"));
    assert!(body.contains("Mirrors"));
    assert!(body.contains("Locket"));
}
