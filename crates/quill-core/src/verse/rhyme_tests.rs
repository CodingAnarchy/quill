//! Checking verse against a rhyme scheme (#1325).

use super::meter::Meter;
use super::rhyme::{
    check_rhyme, letter_name, summarize_rhyme, RhymeFault, RhymeKind, RhymeLine, RhymeScheme,
};

use crate::frontmatter::FrontMatter;

const SONNET_18: &str = "Shall I compare thee to a summer's day?
Thou art more lovely and more temperate:
Rough winds do shake the darling buds of May,
And summer's lease hath all too short a date;
Sometime too hot the eye of heaven shines,
And often is his gold complexion dimm'd;
And every fair from fair sometime declines,
By chance or nature's changing course untrimm'd;
But thy eternal summer shall not fade,
Nor lose possession of that fair thou ow'st;
Nor shall death brag thou wander'st in his shade,
When in eternal lines to time thou grow'st:
So long as men can breathe or eyes can see,
So long lives this, and this gives life to thee.
";

fn scheme(text: &str) -> RhymeScheme {
    RhymeScheme::parse(text).expect("a scheme")
}

/// Each line's heard and expected letters, as letters.
fn letters(lines: &[RhymeLine]) -> (String, String) {
    let heard = lines.iter().map(|l| letter_name(l.heard)).collect();
    let expected = lines.iter().map(|l| letter_name(l.expected)).collect();
    (heard, expected)
}

#[test]
fn a_scheme_reads_from_letters_or_a_name() {
    let s = scheme(" abab  cdcd efef gg ");
    assert_eq!((s.key().as_str(), s.slant), ("ABAB CDCD EFEF GG", false));

    let s = scheme("Shakespearean Sonnet");
    assert_eq!(s.key(), "shakespearean sonnet");
    assert_eq!(s.letters(), "ABAB CDCD EFEF GG");
    assert_eq!(scheme("english sonnet").key(), "shakespearean sonnet");

    let s = scheme("slant ABCB");
    assert_eq!((s.key().as_str(), s.slant), ("slant ABCB", true));
    assert_eq!(scheme("terza rima").key(), "terza rima");
    assert_eq!(scheme("AXAX").key(), "AXAX");
    assert_eq!(
        scheme("abcdefghijklmnopqrstuvwyz").key(),
        "ABCDEFGHIJKLMNOPQRSTUVWYZ"
    );

    for bad in ["", "slant", "AB1", "sonnet-ish", "A-B"] {
        assert_eq!(RhymeScheme::parse(bad), None, "{bad}");
    }
}

#[test]
fn a_sonnet_keeps_its_scheme_but_one_wrenched_rhyme() {
    let lines = check_rhyme(SONNET_18, &scheme("shakespearean sonnet"));
    assert_eq!(lines.len(), 14);
    let (heard, expected) = letters(&lines);
    assert_eq!(expected, "ABABCDCDEFEFGG");
    // "temperate" rhymes with "date" only by its spelling (Shakespeare wrenched
    // its last syllable onto the beat), so line 4 is heard as a new sound.
    assert_eq!(heard, "ABACDEDEFGFGHH");
    let faults: Vec<_> = lines.iter().filter_map(|l| l.fault).collect();
    assert_eq!(
        faults,
        [RhymeFault::Missing {
            with: 1,
            kind: RhymeKind::Eye
        }]
    );
    assert_eq!(lines[3].word, "date");
    assert_eq!(lines[2].partner, Some((0, RhymeKind::Perfect)));
    // Spelled-out elisions the dictionary lacks rhyme by their spelling, and
    // aren't guesses (#1328).
    assert!(!lines[7].guessed && lines[7].fits(), "{:?}", lines[7]);

    let summary = summarize_rhyme(&lines);
    assert_eq!((summary.lines, summary.fitting), (14, 13));
}

#[test]
fn slant_rhymes_count_only_when_asked() {
    let text = "The wind would shape
the things we keep
";
    let strict = check_rhyme(text, &scheme("couplets"));
    assert_eq!(
        strict[1].fault,
        Some(RhymeFault::Missing {
            with: 0,
            kind: RhymeKind::Slant
        })
    );
    let loose = check_rhyme(text, &scheme("slant couplets"));
    assert!(loose[1].fits());
    assert_eq!(loose[1].partner, Some((0, RhymeKind::Slant)));
}

#[test]
fn an_eye_rhyme_never_counts() {
    let lines = check_rhyme(
        "I gave you all my love\nand watched you fail to move\n",
        &scheme("slant couplets"),
    );
    assert_eq!(
        lines[1].fault,
        Some(RhymeFault::Missing {
            with: 0,
            kind: RhymeKind::Eye
        })
    );
}

#[test]
fn an_identical_rhyme_counts_and_is_named() {
    let lines = check_rhyme(
        "I wander through the night\nand you rite the night\n",
        &scheme("AA"),
    );
    assert!(lines[1].fits());
    assert_eq!(lines[1].partner, Some((0, RhymeKind::Identical)));
}

#[test]
fn a_ballad_scheme_repeats_down_each_stanza() {
    let text = "The wind was on the water\nthe gulls were in the sky\nthe boats came home at evening\nand no one wondered why\n\nShe walked along the jetty\nand sang a song of May\nthe lamps were lit behind her\nshe never looked away\n";
    let lines = check_rhyme(text, &scheme("ballad stanza"));
    let (heard, expected) = letters(&lines);
    assert_eq!(expected, "ABCBABCB");
    assert_eq!(heard, "ABCBABCB");
    assert!(lines.iter().all(RhymeLine::fits), "{lines:#?}");
    let summary = summarize_rhyme(&lines);
    assert_eq!(summary.stanzas, [(4, 4), (4, 4)]);
}

#[test]
fn a_rhyme_where_the_scheme_wants_a_new_sound_is_flagged() {
    let lines = check_rhyme(
        "The night was long\nthe song was strong\nthe dark came down\nand filled the sky\n",
        &scheme("ABCB"),
    );
    assert_eq!(lines[1].fault, Some(RhymeFault::Unwanted { with: 0 }));
    assert_eq!(
        lines[3].fault,
        Some(RhymeFault::Missing {
            with: 1,
            kind: RhymeKind::None
        })
    );
}

#[test]
fn an_unrhymed_line_is_free() {
    let lines = check_rhyme(
        "The ships went out\nupon the grey\nthe gulls went with them\ninto the day\n",
        &scheme("XAXA"),
    );
    assert!(lines.iter().all(RhymeLine::fits), "{lines:#?}");
    assert_eq!(letters(&lines).1, "XAXA");
}

#[test]
fn terza_rima_interlocks() {
    let text = "a\nb\nc\nd\ne\nf\ng\n";
    let lines = check_rhyme(text, &scheme("terza rima"));
    assert_eq!(letters(&lines).1, "ABABCBC");
}

#[test]
fn the_targets_round_trip_through_front_matter() {
    let mut fm = FrontMatter::from_raw("pov: Vey\n");
    fm.set_rhyme(Some(&scheme("slant abcb")));
    fm.set_meter(Meter::parse("Blank Verse").as_ref());
    assert_eq!(fm.rhyme(), Some(scheme("slant ABCB")));
    assert_eq!(fm.meter().map(|m| m.key()).as_deref(), Some("blank verse"));
    assert_eq!(fm.pov().as_deref(), Some("Vey"), "other keys are untouched");
    fm.set_rhyme(None);
    fm.set_meter(None);
    assert_eq!((fm.rhyme(), fm.meter()), (None, None));
}
