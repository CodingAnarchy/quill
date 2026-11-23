//! Ballad verse loosened by double offbeats (#1349).

use super::lexicon::Lexicon;
use super::marks::marks;
use super::meter::Meter;
use super::scan::{scan_meter, summarize, Deviation, LineScan, Variation};

/// Common meter with a loose line in each length.
const LOOSE: &str = "And she walked to the edge of the silent sea,\nAnd he rode to the gate of the town,\nThe king sat in Dunfermline town,\nDrinking the blood-red wine;\n";

fn meter(words: &str) -> Meter {
    Meter::parse(words).expect("a meter")
}

fn scan(line: &str, words: &str) -> LineScan {
    scan_meter(line, &meter(words))
        .pop()
        .expect("one line scanned")
}

#[test]
fn a_double_offbeat_is_a_licence() {
    let line = scan(
        "And she walked to the edge of the silent sea",
        "iambic tetrameter",
    );
    assert!(line.fits(), "{line:?}");
    assert_eq!(line.variations, [Variation::DoubleOffbeat]);
    assert_eq!(
        line.marks,
        "\u{2d8}\u{2d8}/ \u{2d8}\u{2d8}/ \u{2d8}\u{2d8}/ \u{2d8}/"
    );

    // Before the first beat, too: an anapestic opening.
    let line = scan("And he rode to the gate of the town", "iambic trimeter");
    assert!(line.fits(), "{line:?}");
    assert_eq!(line.variations, [Variation::DoubleOffbeat]);
}

#[test]
fn the_report_counts_loose_lines_among_the_fitting() {
    let lines = scan_meter(LOOSE, &meter("common meter"));
    assert!(lines.iter().all(LineScan::fits), "{lines:#?}");
    let summary = summarize(&lines);
    assert_eq!(summary.fitting, 4);
    assert!(summary.with_licences >= 2, "{summary:?}");
}

#[test]
fn a_strict_meter_counts_a_double_offbeat_against_the_line() {
    let line = scan(
        "And he rode to the gate of the town",
        "strict iambic trimeter",
    );
    assert!(!line.fits());
    assert_eq!(
        line.deviations,
        [Deviation::Licence(Variation::DoubleOffbeat)]
    );
}

#[test]
fn a_loose_reading_never_hides_an_extra_beat() {
    // Four beats, checked as three: no offbeat takes a stressed syllable.
    let line = scan("She heard the falcon in the hall", "iambic trimeter");
    assert!(!line.fits(), "{line:?}");
    // And a stressed monosyllable can't sit in the extra offbeat either.
    let line = scan("The wind blew cold on the long grey sea", "iambic trimeter");
    assert!(!line.fits(), "{line:?}");
}

#[test]
fn a_polysyllable_on_the_wrong_beat_is_still_a_fault() {
    let line = scan(
        "A dog, happy, lay by the lamp at night",
        "iambic pentameter",
    );
    assert_eq!(
        line.deviations,
        [Deviation::InvertedFoot {
            foot: 2,
            word: "happy".into()
        }]
    );
}

#[test]
fn the_editor_marks_no_fault_on_a_loose_line() {
    let m = marks(
        LOOSE,
        0,
        Some(&meter("common meter")),
        None,
        &Lexicon::default(),
    );
    assert!(m.fault_lines().is_empty(), "{:?}", m.faults);
    let loose = m.lines[0].meter.as_ref().expect("a meter cell");
    assert!(loose.fits && loose.licence);
}

#[test]
fn the_editor_colors_each_double_offbeats_extra_syllable() {
    let first = "And he rode to the gate of the town";
    let text = |(s, e): (usize, usize)| &first[s..e];
    let m = marks(
        first,
        0,
        Some(&meter("iambic trimeter")),
        None,
        &Lexicon::default(),
    );
    let licences: Vec<&str> = m.licences.iter().map(|&r| text(r)).collect();
    assert_eq!(licences, ["And", "to", "of"]);

    // Under strict, the same words are the faults.
    let m = marks(
        first,
        0,
        Some(&meter("strict iambic trimeter")),
        None,
        &Lexicon::default(),
    );
    let faults: Vec<&str> = m.faults.iter().map(|&r| text(r)).collect();
    assert_eq!(faults, ["And", "to", "of"]);
}
