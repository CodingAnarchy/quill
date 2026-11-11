//! The scansion gutter and marks (#1326): each verse line's reading beside it,
//! level with its first row and left of the prose, faults and licences marked in
//! the text, and nothing drawn when it's off or there's no room.

use super::*;
use crate::verse::{lexicon::Lexicon, marks, Meter, RhymeScheme};
use ratatui::buffer::Buffer as Cells;
use ratatui::style::Modifier;
use ratatui::{backend::TestBackend, Terminal};

const VERSE: &str = "Shall I compare thee to a summer's day?\nA dog, happy, lay by the lamp at night\nTo be, or not to be, that is the question:\n";

/// An app over `VERSE` with the marks on for iambic pentameter rhymed ABC.
fn marked() -> App {
    let mut app = app_with(VERSE);
    app.scansion_on = true;
    app.scansion = marks(
        VERSE,
        0,
        Meter::parse("iambic pentameter").as_ref(),
        RhymeScheme::parse("ABC").as_ref(),
        &Lexicon::default(),
    );
    app
}

/// The screen row holding `needle`, and the column it starts at.
fn find(screen: &str, needle: &str) -> Option<(usize, usize)> {
    screen
        .lines()
        .enumerate()
        .find_map(|(y, l)| l.find(needle).map(|x| (y, l[..x].chars().count())))
}

#[test]
fn each_line_reads_beside_itself_left_of_the_prose() {
    let screen = render(&marked(), 140, 10);
    let (gy, gx) = find(
        &screen,
        "\u{2713} \u{2d8}/ \u{2d8}/ \u{2d8}/ \u{2d8}/ \u{2d8}/ 10 A",
    )
    .expect(&screen);
    let (py, px) = find(&screen, "Shall I compare").expect(&screen);
    assert_eq!(gy, py, "level with its line:\n{screen}");
    assert!(gx < px, "left of the prose:\n{screen}");
    // The faulty line's verdict, and the licence's.
    let (fy, _) = find(&screen, "\u{2717} \u{2d8}/ /\u{2d8}").expect(&screen);
    assert_eq!(fy, find(&screen, "A dog").expect(&screen).0);
    let (ly, _) = find(&screen, "~ ").expect(&screen);
    assert_eq!(ly, find(&screen, "To be").expect(&screen).0);
}

#[test]
fn faults_and_licences_are_marked_in_the_text() {
    let app = marked();
    let mut terminal = Terminal::new(TestBackend::new(140, 10)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap();
    let buf: Cells = terminal.backend().buffer().clone();
    let screen = render(&app, 140, 10);
    let style = |needle: &str| {
        let (y, x) = find(&screen, needle).expect(&screen);
        buf[(x as u16, y as u16)].style()
    };
    let theme = app.theme;
    assert_eq!(style("happy").fg, Some(theme.removed));
    assert!(style("happy").add_modifier.contains(Modifier::UNDERLINED));
    assert_eq!(style("question").fg, Some(theme.revised), "a licence");
    assert!(!style("question")
        .add_modifier
        .contains(Modifier::UNDERLINED));
    assert_ne!(
        style("compare").fg,
        Some(theme.removed),
        "a fitting word is plain"
    );
}

#[test]
fn nothing_is_drawn_when_off_or_without_room() {
    let mut app = marked();
    app.scansion_on = false;
    let screen = render(&app, 140, 10);
    assert!(!screen.contains("\u{2d8}/"), "off:\n{screen}");

    // A terminal no wider than the prose column has no gutter, and the prose
    // keeps its place.
    let narrow = render(&marked(), 60, 10);
    assert!(!narrow.contains("\u{2d8}/ \u{2d8}/"), "no room:\n{narrow}");
    assert!(narrow.contains("Shall I compare"), "{narrow}");
}
