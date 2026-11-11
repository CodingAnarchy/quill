//! The author's own pronunciations (#1328) and the verse spellings that scan as
//! written.

use super::lexicon::{set_line, Entry, Lexicon};
use super::meter::Meter;
use super::pronounce::{pronounce, pronounce_with, Stress};
use super::rhyme::{check_rhyme_with, RhymeKind, RhymeScheme};
use super::scan::{scan_meter, scan_meter_with};

const U: Stress = Stress::Unstressed;
const P: Stress = Stress::Primary;

fn lexicon(text: &str) -> Lexicon {
    let (lexicon, errors) = Lexicon::parse(text);
    assert!(errors.is_empty(), "{errors:?}");
    lexicon
}

#[test]
fn an_entry_reads_stress_digits_and_a_rhyme() {
    let e = Entry::parse("Hermione", " 0-1 0 0  Rhymes with  Tony ").expect("parses");
    assert_eq!(e.stresses, [U, P, U, U]);
    assert_eq!(e.rhymes_with.as_deref(), Some("tony"));
    assert_eq!(e.value(), "0100 rhymes with tony");

    assert!(Entry::parse("two words", "01").is_err());
    assert!(Entry::parse("word", "").is_err());
    assert!(Entry::parse("word", "0x1").unwrap_err().contains("'x'"));
    assert!(Entry::parse("word", "01 rhymes with").is_err());
}

#[test]
fn a_file_skips_comments_and_reports_bad_lines() {
    let (lexicon, errors) = Lexicon::parse(
        "# names\n\nHermione = 0100 rhymes with Tony\nsometime = 01\nbroken line\nZed = 9\n",
    );
    assert_eq!(lexicon.len(), 2);
    assert_eq!(
        lexicon.get("HERMIONE").map(|e| e.stresses.len()),
        Some(4),
        "case doesn't matter"
    );
    assert_eq!(errors.len(), 2);
    assert!(errors[0].starts_with("line 5:"), "{errors:?}");
    assert!(errors[1].starts_with("line 6:"), "{errors:?}");
}

#[test]
fn the_authors_pronunciation_comes_before_the_dictionary() {
    let lex = lexicon("sometime = 01\nZorvathine = 102\n");
    let sometime = pronounce_with("Sometime", &lex);
    assert_eq!(sometime.prons.len(), 1);
    assert_eq!(sometime.prons[0].stresses, [U, P]);
    let name = pronounce_with("Zorvathine", &lex);
    assert!(!name.guessed, "an author's word is never a guess");
    assert_eq!(name.prons[0].stresses.len(), 3);

    // Shakespeare's some-TIME, once the author says so, scans.
    let line = "And every fair from fair sometime declines,";
    let meter = Meter::parse("iambic pentameter").expect("a meter");
    assert!(!scan_meter(line, &meter)[0].fits());
    assert!(scan_meter_with(line, &meter, &lex)[0].fits());
}

#[test]
fn rhymes_with_lends_a_word_its_sound() {
    let text = "We walked the road to Quessaly\nand sat beside the grey sea\n";
    let scheme = RhymeScheme::parse("AA").expect("a scheme");
    let plain = check_rhyme_with(text, &scheme, &Lexicon::default());
    assert!(!plain[1].fits(), "{plain:?}");
    let lex = lexicon("Quessaly = 102 rhymes with tea\n");
    let heard = check_rhyme_with(text, &scheme, &lex);
    assert!(heard[1].fits(), "{heard:?}");
    assert_eq!(heard[1].partner, Some((0, RhymeKind::Perfect)));
}

#[test]
fn verse_spellings_scan_as_written() {
    // A grave accent sounds the -ed.
    let beloved = pronounce("belov\u{e8}d");
    assert!(!beloved.guessed);
    assert!(
        beloved.prons.iter().all(|p| p.stresses == [U, P, U]),
        "{beloved:?}"
    );
    assert_eq!(pronounce("beloved").prons[0].stresses.len(), 2);
    // Other accents are read past.
    assert!(!pronounce("na\u{ef}ve").guessed);
    // 'd is a silent -ed.
    let dimmed = pronounce("dimm'd");
    assert!(!dimmed.guessed);
    assert_eq!(dimmed.prons[0].stresses, [P]);
    // Elisions count their letters as written, and aren't guesses.
    for (word, syllables) in [("heav'n", 1), ("o'er", 1), ("ev'ry", 2), ("wand'ring", 2)] {
        let sound = pronounce(word);
        assert!(!sound.guessed, "{word}");
        assert_eq!(sound.prons[0].stresses.len(), syllables, "{word}");
    }
    // A possessive of a made-up name is still a guess.
    assert!(pronounce("Zorvathine's").guessed);
}

#[test]
fn an_apostrophe_outside_the_word_is_kept_for_an_elision() {
    let meter = Meter::parse("iambic tetrameter").expect("a meter");
    let line = &scan_meter("Th' expense of spirit, 'tis a waste", &meter)[0];
    let words: Vec<&str> = line
        .words
        .iter()
        .map(|w| &line.text[w.range.clone()])
        .collect();
    assert_eq!(words[0], "Th'");
    assert!(words.contains(&"'tis"), "{words:?}");
    assert_eq!(line.guessed().count(), 0, "{line:?}");
    // A quotation mark around a word doesn't cost it its dictionary entry.
    let quoted = &scan_meter("She said, 'Remember me', and went", &meter)[0];
    assert_eq!(quoted.guessed().count(), 0, "{quoted:?}");
}

#[test]
fn a_line_is_set_replaced_and_removed_in_place() {
    let entry = |w: &str, v: &str| Entry::parse(w, v).expect("an entry");
    let text = set_line("", "Hermione", Some(&entry("Hermione", "0100")));
    assert!(text.starts_with("# How words sound"), "{text}");
    assert!(text.ends_with("Hermione = 0100\n"), "{text}");

    let text = format!("{text}# keep me\nsometime = 01\n");
    let replaced = set_line(
        &text,
        "HERMIONE",
        Some(&entry("Hermione", "0101 rhymes with tony")),
    );
    assert!(
        replaced.contains("Hermione = 0101 rhymes with tony\n# keep me\nsometime = 01\n"),
        "{replaced}"
    );
    assert_eq!(replaced.matches("Hermione =").count(), 1);

    let removed = set_line(&replaced, "hermione", None);
    assert!(!removed.contains("Hermione ="), "{removed}");
    assert!(removed.contains("# keep me\nsometime = 01\n"));
}
