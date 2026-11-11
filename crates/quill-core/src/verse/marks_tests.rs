//! The in-editor scansion marks (#1326).

use super::blocks::Targets;
use super::lexicon::Lexicon;
use super::marks::{chapter_marks, marks};
use super::meter::Meter;
use super::rhyme::RhymeScheme;

const QUATRAIN: &str = "Shall I compare thee to a summer's day?\nThe cat sat on the mat\n\nTo be, or not to be, that is the question:\nA dog, happy, lay by the lamp at night\n";

fn meter() -> Meter {
    Meter::parse("iambic pentameter").expect("a meter")
}

#[test]
fn nothing_to_check_gives_no_marks() {
    let m = marks(QUATRAIN, 0, None, None, &Lexicon::default());
    assert!(m.lines.is_empty() && m.faults.is_empty() && m.licences.is_empty());
}

#[test]
fn each_verse_line_gets_a_gutter_row_at_its_buffer_offset() {
    let m = marks(QUATRAIN, 100, Some(&meter()), None, &Lexicon::default());
    let starts: Vec<usize> = m.lines.iter().map(|l| l.line_start).collect();
    let at = |line: &str| QUATRAIN.find(line).expect("the line") + 100;
    assert_eq!(
        starts,
        [at("Shall"), at("The cat"), at("To be"), at("A dog")]
    );
    let cell = m.lines[0].meter.as_ref().expect("a meter cell");
    assert!(cell.fits && !cell.licence);
    assert_eq!((cell.syllables, cell.expected), (10, 10));
    let feminine = m.lines[2].meter.as_ref().expect("a meter cell");
    assert!(feminine.fits && feminine.licence);
    assert_eq!(m.fault_lines(), [at("The cat"), at("A dog")]);
}

#[test]
fn faults_and_licences_mark_the_words_that_carry_them() {
    let m = marks(QUATRAIN, 0, Some(&meter()), None, &Lexicon::default());
    let text = |(s, e): (usize, usize)| &QUATRAIN[s..e];
    let faults: Vec<&str> = m.faults.iter().map(|&r| text(r)).collect();
    // A short line is wrong as a whole; an inverted foot marks its word.
    assert_eq!(faults, ["The cat sat on the mat", "happy"]);
    let licences: Vec<&str> = m.licences.iter().map(|&r| text(r)).collect();
    assert_eq!(
        licences,
        ["question"],
        "a feminine ending marks the last word"
    );
}

#[test]
fn a_rhyme_fault_marks_the_end_word_and_letters_the_gutter() {
    let text = "The night was long\nthe song was strong\nthe dark came down\nand filled the sky\n";
    let scheme = RhymeScheme::parse("AABB").expect("a scheme");
    let m = marks(text, 0, None, Some(&scheme), &Lexicon::default());
    let letters: Vec<&str> = m
        .lines
        .iter()
        .map(|l| l.rhyme.as_ref().expect("a rhyme cell").letter.as_str())
        .collect();
    assert_eq!(letters, ["A", "A", "B", "C"]);
    let faults: Vec<&str> = m.faults.iter().map(|&(s, e)| &text[s..e]).collect();
    assert_eq!(faults, ["sky"]);
    assert!(m.lines.iter().all(|l| l.meter.is_none()));
}

/// Prose with a pentameter quatrain fenced inside it, then a block with no
/// targets of its own (#1337).
const STORY: &str = "She sang it slowly, the way her mother had.\n\n```verse iambic pentameter\nShall I compare thee to a summer's day?\nThe cat sat on the mat\n```\n\nThen silence.\n\n```verse\nShall I compare thee to a summer's day?\n```\n";

#[test]
fn a_fenced_block_is_marked_against_its_own_targets_and_prose_isnt() {
    let m = chapter_marks(STORY, 50, &Targets::default(), &Lexicon::default());
    let at = |line: &str| STORY.find(line).expect("the line") + 50;
    // The block with no targets of its own takes the chapter's, which are none.
    let starts: Vec<usize> = m.lines.iter().map(|l| l.line_start).collect();
    assert_eq!(starts, [at("Shall"), at("The cat")]);
    assert_eq!(m.fault_lines(), [at("The cat")]);
    assert!(m.faults.iter().all(|&(s, _)| s >= at("The cat")));
}

#[test]
fn a_block_with_no_targets_takes_the_chapters() {
    let chapter = Targets {
        meter: Some(Meter::parse("trochaic tetrameter").expect("a meter")),
        rhyme: None,
    };
    let m = chapter_marks(STORY, 0, &chapter, &Lexicon::default());
    let second = STORY.rfind("Shall").expect("the second block");
    let cell = m
        .lines
        .iter()
        .find(|l| l.line_start == second)
        .and_then(|l| l.meter.as_ref())
        .expect("the second block is checked");
    assert!(!cell.fits, "pentameter against the chapter's tetrameter");
    // The prose lines are checked against the chapter's meter, as before, and
    // the lines stay in buffer order.
    assert!(m.lines.iter().any(|l| l.line_start == 0));
    assert!(m
        .lines
        .windows(2)
        .all(|w| w[0].line_start < w[1].line_start));
}
