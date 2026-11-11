//! The in-editor scansion marks (#1326).

use super::lexicon::Lexicon;
use super::marks::marks;
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
