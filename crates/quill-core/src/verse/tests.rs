//! Verse analysis (#1324): the target meter, the dictionary, and the scanner.

use super::meter::{Foot, Meter};
use super::pronounce::{pronounce, Stress};
use super::scan::{scan_meter, summarize, Deviation, Variation};

fn pentameter() -> Meter {
    Meter::parse("iambic pentameter").expect("a meter")
}

/// Scan one line of iambic pentameter.
fn scan(line: &str) -> super::LineScan {
    scan_meter(line, &pentameter())
        .into_iter()
        .next()
        .expect("one line scanned")
}

#[test]
fn a_meter_reads_from_the_authors_words() {
    let m = Meter::parse("  Iambic   Pentameter ").expect("parses");
    assert_eq!(
        (m.foot, m.lengths.as_slice(), m.strict),
        (Foot::Iamb, &[5][..], false)
    );
    assert_eq!(m.key(), "iambic pentameter");

    let m = Meter::parse("strict trochaic tetrameter").expect("parses");
    assert_eq!((m.foot, m.strict), (Foot::Trochee, true));
    assert_eq!(m.key(), "strict trochaic tetrameter");

    let m = Meter::parse("iambic tetrameter/trimeter").expect("parses");
    assert_eq!(m.lengths, [4, 3]);
    assert_eq!(
        (m.feet_for_line(0), m.feet_for_line(1), m.feet_for_line(2)),
        (4, 3, 4)
    );

    let m = Meter::parse("ballad meter").expect("an alias");
    assert_eq!(
        (m.key().as_str(), m.lengths.as_slice()),
        ("common meter", &[4, 3][..])
    );
    assert_eq!(
        Meter::parse("Blank Verse").map(|m| m.foot),
        Some(Foot::Iamb)
    );

    for bad in [
        "",
        "iambic",
        "pentameter",
        "iambic decameter",
        "loose iambs",
    ] {
        assert_eq!(Meter::parse(bad), None, "{bad}");
    }
}

#[test]
fn the_dictionary_gives_syllables_and_stress() {
    let record = pronounce("Record");
    assert!(!record.guessed);
    // Both the verb and the noun.
    let readings: Vec<_> = record.prons.iter().map(|p| p.stresses.clone()).collect();
    assert!(readings.contains(&vec![Stress::Unstressed, Stress::Primary]));
    assert!(readings.contains(&vec![Stress::Primary, Stress::Unstressed]));

    // A curly apostrophe reads as a straight one.
    assert!(!pronounce("summer\u{2019}s").guessed);
    // A possessive the dictionary lacks takes its base word, and a syllable
    // after a hiss.
    let lighthouse = pronounce("lighthouse's");
    assert!(!lighthouse.guessed);
    assert_eq!(lighthouse.prons[0].stresses.len(), 3);
    assert_eq!(pronounce("harness's").prons[0].stresses.len(), 3);
    assert_eq!(pronounce("sunset's").prons[0].stresses.len(), 2);

    let made_up = pronounce("Zorvathine");
    assert!(made_up.guessed);
    assert_eq!(made_up.prons[0].stresses.len(), 3);
}

#[test]
fn a_regular_line_fits_and_shows_its_feet() {
    let line = scan("Shall I compare thee to a summer's day?");
    assert!(line.fits(), "{line:?}");
    assert_eq!(line.marks, "\u{2d8}/ \u{2d8}/ \u{2d8}/ \u{2d8}/ \u{2d8}/");
    assert_eq!((line.syllables, line.expected), (10, 10));
    assert!(line.variations.is_empty());
    assert_eq!(line.guessed().count(), 0);
}

#[test]
fn a_promoted_syllable_fits() {
    // tem-per-ATE: the last syllable takes the beat.
    let line = scan("Thou art more lovely and more temperate:");
    assert!(line.fits(), "{line:?}");
}

#[test]
fn a_feminine_ending_is_a_licence() {
    let line = scan("To be, or not to be, that is the question:");
    assert!(line.fits(), "{line:?}");
    assert_eq!(line.variations, [Variation::FeminineEnding]);
    assert_eq!(line.syllables, 11);
}

#[test]
fn an_inverted_first_foot_is_a_licence() {
    let line = scan("Under the hill the river runs to sea");
    assert!(line.fits(), "{line:?}");
    assert_eq!(line.variations, [Variation::InitialInversion]);
    assert!(
        line.marks.starts_with("/\u{2d8} \u{2d8}/"),
        "{}",
        line.marks
    );
}

#[test]
fn a_strict_meter_counts_the_licence_against_the_line() {
    let strict = Meter::parse("strict iambic pentameter").expect("a meter");
    let line = scan_meter("To be, or not to be, that is the question:", &strict)
        .pop()
        .expect("a line");
    assert!(!line.fits());
    assert_eq!(
        line.deviations,
        [Deviation::Licence(Variation::FeminineEnding)]
    );
}

#[test]
fn an_inverted_foot_mid_line_is_named() {
    let line = scan("A dog, happy, lay by the lamp at night");
    assert_eq!(
        line.deviations,
        [Deviation::InvertedFoot {
            foot: 2,
            word: "happy".into()
        }],
        "{line:?}"
    );
    let happy = line
        .words
        .iter()
        .find(|w| &line.text[w.range.clone()] == "happy")
        .expect("the word");
    assert!(happy.off);
}

#[test]
fn a_long_or_short_line_counts_its_syllables() {
    let long = scan("The quick brown fox jumped over all the lazy dogs today");
    assert_eq!(long.deviations, [Deviation::ExtraSyllables(4)]);
    assert_eq!(long.syllables, 14);
    let short = scan("The cat sat on the mat");
    assert_eq!(short.deviations, [Deviation::MissingSyllables(4)]);
    assert_eq!(short.deviations[0].to_string(), "4 syllables short");
}

#[test]
fn a_guessed_word_is_marked() {
    let line = scan("The towers of Zorvathine are falling down");
    assert_eq!(line.guessed().collect::<Vec<_>>(), ["Zorvathine"]);
}

#[test]
fn structure_is_skipped_and_stanzas_counted() {
    let text = "# Sonnet\n\nShall I compare thee to a summer's day?\nThe cat sat on the mat\n\n***\n\nTo be, or not to be, that is the question:\n";
    let lines = scan_meter(text, &pentameter());
    assert_eq!(lines.len(), 3);
    assert_eq!(
        lines.iter().map(|l| (l.line, l.stanza)).collect::<Vec<_>>(),
        [(2, 0), (3, 0), (7, 1)]
    );
    let summary = summarize(&lines);
    assert_eq!(
        (summary.lines, summary.fitting, summary.with_licences),
        (3, 2, 1)
    );
    assert_eq!(summary.stanzas, [(1, 2), (1, 1)]);
    assert_eq!(summary.most_common, Some(("missing syllables", 1)));
}

#[test]
fn common_meter_alternates_line_lengths() {
    let meter = Meter::parse("common meter").expect("a meter");
    let text = "Because I could not stop for Death\nHe kindly stopped for me\n";
    let lines = scan_meter(text, &meter);
    assert_eq!(lines.iter().map(|l| l.expected).collect::<Vec<_>>(), [8, 6]);
    assert!(lines.iter().all(super::LineScan::fits), "{lines:?}");
}

#[test]
fn trochaic_lines_may_drop_the_last_offbeat() {
    let meter = Meter::parse("trochaic tetrameter").expect("a meter");
    let lines = scan_meter("Tyger Tyger, burning bright,\n", &meter);
    assert!(lines[0].fits(), "{lines:?}");
    assert_eq!(lines[0].variations, [Variation::Catalectic]);
}
