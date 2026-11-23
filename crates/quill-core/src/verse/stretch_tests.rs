//! Stanzas with a stretchable middle (#1348): the ballad stanza extended by
//! lines before its closing rhyme, checked as written rather than wrapped into a
//! fixed cycle.

use super::blocks::{companion_meter, companion_rhyme, Targets};
use super::lines::stanza_slot;
use super::meter::Meter;
use super::rhyme::{check_rhyme, letter_name, RhymeFault, RhymeLine, RhymeScheme};
use super::scan::{scan_meter, LineScan};

/// A ballad quatrain, then a stanza stretched by two more tetrameter lines.
const EXTENDED: &str = "The king rode out upon the hill,
The night was cold and grey,
He heard the river in the dark,
And would not turn away.

The queen sat waiting by the fire,
Her hands upon her knee,
She watched the shadows on the wall,
She heard the falcon in the hall,
And saw the candles gutter, fall,
And waited for the sea.
";

fn meter(text: &str) -> Meter {
    Meter::parse(text).expect("a meter")
}

fn scheme(text: &str) -> RhymeScheme {
    RhymeScheme::parse(text).expect("a scheme")
}

fn expected_letters(lines: &[RhymeLine]) -> String {
    lines.iter().map(|l| letter_name(l.expected)).collect()
}

#[test]
fn a_stretch_slot_fills_the_middle_of_any_length_of_stanza() {
    // ABC+B: four slots, the third repeating.
    let slots =
        |len: usize| -> Vec<usize> { (0..len).map(|p| stanza_slot(p, len, 4, Some(2))).collect() };
    assert_eq!(slots(4), [0, 1, 2, 3]);
    assert_eq!(slots(5), [0, 1, 2, 2, 3]);
    assert_eq!(slots(7), [0, 1, 2, 2, 2, 2, 3]);
    // Too short to hold a middle line: the slots in order from the start.
    assert_eq!(slots(3), [0, 1, 2]);
    // Without a stretch the pattern cycles, as before.
    assert_eq!(
        (0..6)
            .map(|p| stanza_slot(p, 6, 2, None))
            .collect::<Vec<_>>(),
        [0, 1, 0, 1, 0, 1]
    );
}

#[test]
fn a_meter_can_stretch_one_length_through_the_middle() {
    let m = meter("Iambic Tetrameter/Trimeter/Tetrameter+/Trimeter");
    assert_eq!(
        (m.lengths.as_slice(), m.stretch),
        (&[4, 3, 4, 3][..], Some(2))
    );
    assert_eq!(m.key(), "iambic tetrameter/trimeter/tetrameter+/trimeter");
    assert_eq!(Meter::parse(&m.key()), Some(m.clone()));
    let feet = |len: usize| -> Vec<usize> { (0..len).map(|n| m.feet_for_line(n, len)).collect() };
    assert_eq!(feet(4), [4, 3, 4, 3]);
    assert_eq!(feet(6), [4, 3, 4, 4, 4, 3]);

    // Only one length can stretch.
    assert_eq!(Meter::parse("iambic tetrameter+/trimeter+"), None);
    assert_eq!(Meter::parse("iambic tetrameter++"), None);
}

#[test]
fn the_extended_ballad_scans_as_written() {
    let scans = scan_meter(EXTENDED, &meter("extended ballad stanza"));
    let expected: Vec<usize> = scans.iter().map(|s| s.expected / 2).collect();
    assert_eq!(expected, [4, 3, 4, 3, 4, 3, 4, 4, 4, 3]);
    assert!(scans.iter().all(LineScan::fits), "{scans:#?}");

    // Common meter still cycles 4-3, so the stretched stanza's fourth line,
    // a tetrameter, is checked as a trimeter and flagged, as before.
    let common = scan_meter(EXTENDED, &meter("common meter"));
    assert!(common[..7].iter().all(LineScan::fits), "{common:#?}");
    assert!(!common[7].fits(), "{:#?}", common[7]);
}

#[test]
fn a_scheme_can_stretch_one_letter_through_the_middle() {
    let s = scheme("abc+b");
    assert_eq!((s.key().as_str(), s.letters().as_str()), ("ABC+B", "ABC+B"));
    assert_eq!(RhymeScheme::parse(&s.key()), Some(s));
    assert_eq!(scheme("slant ABX+B").key(), "slant ABX+B");
    for bad in ["a+b+c", "+ab", "abc+b defd", "ab+c d"] {
        assert_eq!(RhymeScheme::parse(bad), None, "{bad}");
    }
}

#[test]
fn the_extended_ballad_rhymes_as_written() {
    let lines = check_rhyme(EXTENDED, &scheme("ABC+B"));
    assert_eq!(expected_letters(&lines), "ABCBABCCCB");
    assert!(lines.iter().all(RhymeLine::fits), "{lines:#?}");

    // The plain ballad stanza wraps the long stanza into ABCB AB and pairs its
    // lines with the wrong partners, as before.
    let ballad = check_rhyme(EXTENDED, &scheme("ballad stanza"));
    assert_eq!(expected_letters(&ballad), "ABCBABCBAB");
    assert!(!ballad.iter().all(RhymeLine::fits));
}

#[test]
fn a_closing_line_that_breaks_the_shape_is_flagged() {
    let broken = EXTENDED.replace("And waited for the sea.", "And waited for the dawn.");
    let lines = check_rhyme(&broken, &scheme("extended ballad stanza"));
    let faults: Vec<usize> = lines.iter().filter(|l| !l.fits()).map(|l| l.line).collect();
    assert_eq!(faults, [10], "{lines:#?}");
    assert!(matches!(
        lines[9].fault,
        Some(RhymeFault::Missing { with: 6, .. })
    ));
}

#[test]
fn an_unrhymed_middle_needs_no_rhymes_between_its_lines() {
    let text = "The king rode out upon the hill,
The night was cold and grey,
He heard the river in the dark,
The owls were on the wing,
And would not turn away.
";
    let lines = check_rhyme(text, &scheme("ABX+B"));
    assert_eq!(expected_letters(&lines), "ABXXB");
    assert!(lines.iter().all(RhymeLine::fits), "{lines:#?}");
    // ABC+B asks the middle lines to rhyme with each other.
    let rhymed = check_rhyme(text, &scheme("ABC+B"));
    assert!(!rhymed[3].fits(), "{:#?}", rhymed[3]);
}

#[test]
fn the_extended_ballad_name_sets_meter_and_rhyme_together() {
    let t = Targets::parse("extended ballad stanza").expect("targets");
    assert_eq!(t.meter, Some(meter("extended ballad stanza")));
    assert_eq!(t.rhyme, Some(scheme("extended ballad stanza")));
    assert_eq!(t.key(), "extended ballad stanza");
    assert_eq!(Targets::parse(&t.key()), Some(t));

    let strict = Targets::parse("strict extended ballad").expect("targets");
    assert!(strict.meter.as_ref().is_some_and(|m| m.strict));
    assert_eq!(strict.key(), "strict extended ballad stanza");
    assert_eq!(Targets::parse(&strict.key()), Some(strict));

    // Spelled out, meter and scheme together.
    let spelled =
        Targets::parse("iambic tetrameter/trimeter/tetrameter+/trimeter abc+b").expect("targets");
    assert_eq!(spelled.meter.map(|m| m.stretch), Some(Some(2)));
    assert_eq!(spelled.rhyme.map(|r| r.key()).as_deref(), Some("ABC+B"));

    // A form that's only a meter, or only a scheme, brings no companion.
    assert_eq!(companion_rhyme(&meter("common meter")), None);
    assert_eq!(companion_meter(&scheme("ballad stanza")), None);
    let t = Targets::parse("common meter / ballad stanza").expect("targets");
    assert_eq!(t.key(), "common meter / ballad stanza");
}

#[test]
fn a_fenced_extended_ballad_takes_both_targets() {
    let text = format!("She sang.\n\n```verse extended ballad stanza\n{EXTENDED}```\n");
    let blocks = super::verse_blocks(&text);
    assert_eq!(blocks.len(), 1);
    let targets = blocks[0].targets.clone().expect("readable targets");
    assert!(targets.meter.is_some() && targets.rhyme.is_some());
}
