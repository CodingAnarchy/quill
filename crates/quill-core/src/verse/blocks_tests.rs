//! Tests for verse fenced inside prose (#1337).

use super::blocks::{fence_targets, shift_lines, verse_blocks, Targets};
use super::meter::Meter;
use super::rhyme::{check_rhyme, RhymeFault, RhymeScheme};
use super::scan::scan_meter;

fn targets(text: &str) -> Targets {
    Targets::parse(text).expect("the targets read")
}

#[test]
fn a_fence_line_is_verse_only_with_the_verse_word() {
    assert_eq!(fence_targets("```verse"), Some(""));
    assert_eq!(
        fence_targets("```verse iambic tetrameter"),
        Some("iambic tetrameter")
    );
    assert_eq!(fence_targets("  ``` Verse  abab  "), Some("abab"));
    assert_eq!(fence_targets("````verse abab"), Some("abab"));
    assert_eq!(fence_targets("```"), None);
    assert_eq!(fence_targets("```rust"), None);
    assert_eq!(fence_targets("```versed"), None);
    assert_eq!(fence_targets("verse abab"), None);
}

#[test]
fn targets_read_a_meter_a_scheme_or_both() {
    let both = targets("iambic tetrameter abab");
    assert_eq!(both.meter, Meter::parse("iambic tetrameter"));
    assert_eq!(both.rhyme, RhymeScheme::parse("abab"));

    let meter = targets("iambic tetrameter/trimeter");
    assert_eq!(meter.meter, Meter::parse("iambic tetrameter/trimeter"));
    assert!(meter.rhyme.is_none());

    let scheme = targets("ABAB CDCD EFEF GG");
    assert!(scheme.meter.is_none());
    assert_eq!(scheme.rhyme, RhymeScheme::parse("abab cdcd efef gg"));

    let named = targets("common meter / ballad stanza");
    assert_eq!(named.meter, Meter::parse("common meter"));
    assert_eq!(named.rhyme, RhymeScheme::parse("ballad stanza"));

    let strict = targets("strict iambic pentameter, slant abab");
    assert!(strict.meter.is_some_and(|m| m.strict));
    assert!(strict.rhyme.is_some_and(|r| r.slant));

    assert!(targets("").is_empty());
}

#[test]
fn a_misspelled_meter_isnt_read_as_a_scheme() {
    assert_eq!(Targets::parse("iambik tetrameter"), None);
    assert_eq!(Targets::parse("iambic tetrameter ffff"), None);
    assert_eq!(Targets::parse("ABBA"), Some(targets("abba")));
}

#[test]
fn targets_round_trip_through_their_key() {
    for text in [
        "iambic tetrameter abab",
        "common meter / ballad stanza",
        "aabb",
        "blank verse",
    ] {
        let t = targets(text);
        assert_eq!(Targets::parse(&t.key()), Some(t), "{text}");
    }
}

#[test]
fn a_block_names_its_own_targets_or_takes_the_chapters() {
    let chapter = targets("iambic pentameter");
    let own = targets("trochaic tetrameter");
    assert_eq!(own.or(&chapter), own);
    assert_eq!(Targets::default().or(&chapter), chapter);
}

#[test]
fn blocks_are_found_with_their_lines_and_targets() {
    let text = "She sang.\n\n```verse iambic tetrameter abab\nOne line\n\nTwo line\n```\n\nThen she stopped.\n```\ncode ```verse\n```\n```verse\nLast\n";
    let blocks = verse_blocks(text);
    assert_eq!(blocks.len(), 2);

    let first = &blocks[0];
    assert_eq!(&text[first.inner.clone()], "One line\n\nTwo line\n");
    assert_eq!(first.first_line, 3);
    assert!(text[first.span.clone()].starts_with("```verse"));
    assert!(text[first.span.clone()].ends_with("```\n"));
    assert_eq!(first.targets, Some(targets("iambic tetrameter abab")));

    // The code fence's own lines aren't read as a verse fence; the unclosed
    // block at the end runs to the end of the text.
    let last = &blocks[1];
    assert_eq!(&text[last.inner.clone()], "Last\n");
    assert_eq!(last.targets, Some(Targets::default()));
}

#[test]
fn unreadable_targets_check_nothing() {
    let blocks = verse_blocks("```verse iambik\nA line\n```\n");
    assert_eq!(blocks[0].targets, None);
    assert_eq!(blocks[0].written, "iambik");
    assert!(blocks[0]
        .targets_within(&targets("iambic pentameter"))
        .is_empty());
}

#[test]
fn a_passage_is_numbered_as_lines_of_the_chapter() {
    let poem = "The cat sat on the mat today\nThe dog ran off and far away\nA bird\n";
    let mut scans = scan_meter(poem, &Meter::parse("iambic tetrameter").expect("meter"));
    let mut rhymes = check_rhyme(poem, &RhymeScheme::parse("aab").expect("scheme"));
    shift_lines(&mut scans, &mut rhymes, 10);
    assert_eq!(
        scans.iter().map(|s| s.line).collect::<Vec<_>>(),
        [10, 11, 12]
    );
    assert_eq!(rhymes[1].line, 11);
    assert_eq!(rhymes[1].partner.map(|(w, _)| w), Some(10));
    assert!(matches!(
        rhymes[2].fault,
        None | Some(RhymeFault::Unwanted { .. })
    ));
}
