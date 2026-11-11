//! Scanning verse against a target meter (#1324).
//!
//! Each line is matched against templates built from the meter: the strict
//! pattern (`˘/˘/˘/˘/˘/` for iambic pentameter) with the usual licences as
//! optional slots: a feminine ending's extra offbeat, a headless line's missing
//! first offbeat, an inverted first foot, a catalectic ending. A small dynamic
//! program picks, for every word, the pronunciation and the stress reading that
//! fit best, the way a reader resolves an ambiguous line. A polysyllable's
//! dictionary stress is fixed, so putting it on an offbeat is a real fault. A
//! monosyllable, a function word, or a secondary stress can go either way, which
//! is what keeps *the* and *of* from failing every line.
//!
//! A line that fits shows the reading that fit. A line no template can hold
//! (too many or too few syllables) or that needs a fixed stress on the wrong beat
//! is a deviation, named for the report. Quill only reports. It never offers a
//! word or a line that would fit.

use std::fmt;
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use super::lines::verse_lines;
use super::meter::{Foot, Meter};
use super::pronounce::{is_function_word, pronounce, Sound, Stress};

/// A licence: a variation from the strict pattern that verse in the meter
/// commonly takes. Accepted unless the meter is strict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Variation {
    /// An extra unstressed syllable at the end of a rising line.
    FeminineEnding,
    /// A rising line missing its first unstressed syllable.
    Headless,
    /// An iambic line opening on a trochee.
    InitialInversion,
    /// A falling line missing its last unstressed syllable(s).
    Catalectic,
    /// An extra unstressed syllable before a falling line's first stress.
    Anacrusis,
    /// A triple foot with one of its unstressed syllables left out.
    Substitution,
}

impl fmt::Display for Variation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::FeminineEnding => "feminine ending",
            Self::Headless => "headless line",
            Self::InitialInversion => "inverted first foot",
            Self::Catalectic => "catalectic ending",
            Self::Anacrusis => "anacrusis",
            Self::Substitution => "substituted foot",
        })
    }
}

/// How a line breaks the meter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Deviation {
    /// More syllables than any reading of the meter allows; the count over the
    /// strict pattern.
    ExtraSyllables(usize),
    /// Fewer syllables than any reading allows; the count under the pattern.
    MissingSyllables(usize),
    /// A foot (counted from 1) whose fixed stresses read the foot backwards.
    InvertedFoot { foot: usize, word: String },
    /// A foot (from 1) with a word's fixed stress on the wrong syllable.
    MisplacedStress { foot: usize, word: String },
    /// A licence, counted as a fault because the meter is strict.
    Licence(Variation),
}

impl Deviation {
    /// The deviation's kind, for tallying the most common one.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::ExtraSyllables(_) => "extra syllables",
            Self::MissingSyllables(_) => "missing syllables",
            Self::InvertedFoot { .. } => "inverted foot",
            Self::MisplacedStress { .. } => "misplaced stress",
            Self::Licence(_) => "licence (strict)",
        }
    }
}

impl fmt::Display for Deviation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        match self {
            Self::ExtraSyllables(n) => write!(f, "{n} extra syllable{}", plural(*n)),
            Self::MissingSyllables(n) => write!(f, "{n} syllable{} short", plural(*n)),
            Self::InvertedFoot { foot, word } => write!(f, "foot {foot} inverted ({word})"),
            Self::MisplacedStress { foot, word } => {
                write!(f, "stress out of place in foot {foot} ({word})")
            }
            Self::Licence(v) => write!(f, "{v} (strict)"),
        }
    }
}

/// One word of a scanned line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordScan {
    /// The word's bytes within the line.
    pub range: Range<usize>,
    /// Each syllable's stress in the reading shown, `true` for stressed.
    pub stresses: Vec<bool>,
    /// Whether the word's pronunciation was estimated.
    pub guessed: bool,
    /// Whether the word is where the line breaks the meter.
    pub off: bool,
}

/// One line of verse, scanned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineScan {
    /// The line's number in the scanned text, from 0.
    pub line: usize,
    /// The stanza it belongs to, from 0.
    pub stanza: usize,
    pub text: String,
    /// Syllables in the reading shown.
    pub syllables: usize,
    /// Syllables in the strict pattern for this line.
    pub expected: usize,
    /// The reading as stress marks, grouped into feet when the line fits the
    /// meter's shape: `˘/ ˘/ ˘/ ˘/ ˘/`.
    pub marks: String,
    pub words: Vec<WordScan>,
    /// Licences the line takes, when the meter accepts them.
    pub variations: Vec<Variation>,
    pub deviations: Vec<Deviation>,
}

impl LineScan {
    /// Whether the line keeps to the meter.
    pub const fn fits(&self) -> bool {
        self.deviations.is_empty()
    }

    /// The words whose pronunciation Quill had to guess.
    pub fn guessed(&self) -> impl Iterator<Item = &str> {
        self.words
            .iter()
            .filter(|w| w.guessed)
            .map(|w| &self.text[w.range.clone()])
    }
}

/// The unstressed and stressed marks.
pub const OFFBEAT: char = '\u{02D8}';
pub const BEAT: char = '/';

/// Scan every line of verse in `text` (a chapter body, front matter already
/// removed) against `meter`. Headings, scene breaks, fenced code, and blank
/// lines are skipped; a blank line or a break ends a stanza (see
/// [`verse_lines`]).
pub fn scan_meter(text: &str, meter: &Meter) -> Vec<LineScan> {
    verse_lines(text)
        .into_iter()
        .map(|l| {
            let feet = meter.feet_for_line(l.position);
            scan_line(l.line, l.stanza, l.text, &words(l.text), meter, feet)
        })
        .collect()
}

/// A word in a line, with how it sounds.
struct Word {
    range: Range<usize>,
    sound: Sound,
    function: bool,
}

/// The line's words: Unicode word segmentation, keeping only tokens with a
/// letter in them (a bare number isn't scanned).
fn words(line: &str) -> Vec<Word> {
    line.unicode_word_indices()
        .filter(|(_, w)| w.chars().any(char::is_alphabetic))
        .map(|(start, w)| Word {
            range: start..start + w.len(),
            sound: pronounce(w),
            function: is_function_word(w),
        })
        .collect()
}

/// How a syllable weighs in a line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Weight {
    /// A polysyllable's primary stress (`true`) or unstressed syllable: putting
    /// it on the other kind of beat breaks the meter.
    Fixed(bool),
    /// A syllable a line can read either way; `true` when it leans stressed.
    Free(bool),
}

impl Weight {
    const fn lean(self) -> bool {
        match self {
            Self::Fixed(s) | Self::Free(s) => s,
        }
    }
}

/// Each syllable's weight in one pronunciation of a word. A monosyllable, a
/// function word, or a guessed word can be read either way. In a polysyllable
/// the primary stress is fixed, and so is an unstressed syllable beside it; an
/// unstressed syllable two away from it may take a beat (*tem-per-ATE*), as
/// verse commonly promotes one, and a secondary stress goes either way.
fn weights(word: &Word, stresses: &[Stress]) -> Vec<Weight> {
    let free = word.sound.guessed || word.function || stresses.len() == 1;
    let beside_primary = |i: usize| {
        let before = i.checked_sub(1).and_then(|b| stresses.get(b));
        let after = stresses.get(i + 1);
        before == Some(&Stress::Primary) || after == Some(&Stress::Primary)
    };
    stresses
        .iter()
        .enumerate()
        .map(|(i, s)| match (free, s) {
            (true, _) if stresses.len() == 1 => Weight::Free(!word.function),
            (true, s) => Weight::Free(*s == Stress::Primary),
            (false, Stress::Primary) => Weight::Fixed(true),
            (false, Stress::Unstressed) if beside_primary(i) => Weight::Fixed(false),
            (false, Stress::Unstressed) => Weight::Free(false),
            (false, Stress::Secondary) => Weight::Free(true),
        })
        .collect()
}

/// What filling, or leaving empty, a template slot means.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The pattern needs a syllable here.
    Required,
    /// An extra slot: empty is the strict pattern, filled is the licence.
    Extra(Variation),
    /// A slot the licence leaves empty.
    Droppable(Variation),
}

#[derive(Clone, Copy)]
struct Slot {
    stressed: bool,
    /// The foot it belongs to, from 0.
    foot: usize,
    kind: Kind,
}

/// One reading the meter allows, and the licence that reading itself takes.
struct Template {
    slots: Vec<Slot>,
    licence: Option<Variation>,
}

/// The templates for a line of `feet` feet.
fn templates(foot: Foot, feet: usize) -> Vec<Template> {
    let pattern = foot.pattern();
    let base: Vec<Slot> = (0..feet)
        .flat_map(|f| {
            pattern.iter().map(move |&stressed| Slot {
                stressed,
                foot: f,
                kind: Kind::Required,
            })
        })
        .collect();
    let last_foot = feet.saturating_sub(1);
    let offbeat = |foot, kind| Slot {
        stressed: false,
        foot,
        kind,
    };
    let feminine = offbeat(last_foot, Kind::Extra(Variation::FeminineEnding));
    let anacrusis = offbeat(0, Kind::Extra(Variation::Anacrusis));
    let mut slots = base.clone();
    let mut out = Vec::new();
    match foot {
        Foot::Iamb => {
            if let Some(first) = slots.first_mut() {
                first.kind = Kind::Droppable(Variation::Headless);
            }
            slots.push(feminine);
            // The inverted first foot is its own reading.
            let mut inverted = base;
            if inverted.len() >= 2 {
                inverted[0].stressed = true;
                inverted[1].stressed = false;
            }
            inverted.push(feminine);
            out.push(Template {
                slots: inverted,
                licence: Some(Variation::InitialInversion),
            });
        }
        Foot::Anapest => {
            for slot in slots.iter_mut().step_by(3) {
                slot.kind = Kind::Droppable(Variation::Substitution);
            }
            if let Some(first) = slots.first_mut() {
                first.kind = Kind::Droppable(Variation::Headless);
            }
            slots.push(feminine);
        }
        Foot::Trochee => {
            if let Some(last) = slots.last_mut() {
                last.kind = Kind::Droppable(Variation::Catalectic);
            }
            slots.insert(0, anacrusis);
        }
        Foot::Dactyl => {
            for slot in slots.iter_mut().skip(2).step_by(3) {
                slot.kind = Kind::Droppable(Variation::Substitution);
            }
            let len = slots.len();
            for slot in slots.iter_mut().skip(len.saturating_sub(2)) {
                slot.kind = Kind::Droppable(Variation::Catalectic);
            }
            slots.insert(0, anacrusis);
        }
        Foot::Amphibrach => {
            if let Some(first) = slots.first_mut() {
                first.kind = Kind::Droppable(Variation::Headless);
            }
            if let Some(last) = slots.last_mut() {
                last.kind = Kind::Droppable(Variation::Catalectic);
            }
        }
        Foot::Spondee => {}
    }
    out.insert(
        0,
        Template {
            slots,
            licence: None,
        },
    );
    out
}

/// What a reading costs: its faults (a fixed stress on the wrong beat), its
/// licences, and its strain (syllables read against their lean).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Cost {
    faults: u32,
    licences: u32,
    strain: u32,
}

impl Cost {
    /// The reading's weight, lower being the more natural reading. A fault
    /// weighs most, but not infinitely: a reading that fits only by straining
    /// every small word against its lean and stacking licences loses to the
    /// plain reading with one foot inverted, which is what a reader would hear.
    const fn score(self) -> (u32, u32) {
        (
            self.faults * 3 + self.licences * 2 + self.strain,
            self.faults,
        )
    }
}

/// A partial reading: how far into the template it has got and how.
#[derive(Clone)]
struct Reading {
    cost: Cost,
    /// The slot each syllable so far fills.
    slots: Vec<usize>,
    /// The pronunciation chosen for each word so far.
    prons: Vec<usize>,
    licences: Vec<Variation>,
}

impl Reading {
    fn licence(&mut self, v: Variation) {
        self.cost.licences += 1;
        self.licences.push(v);
    }
}

/// Keep the cheaper of two readings that reach the same slot.
fn keep(at: &mut Option<Reading>, reading: Reading) {
    if at
        .as_ref()
        .is_none_or(|r| reading.cost.score() < r.cost.score())
    {
        *at = Some(reading);
    }
}

/// The best reading of the words against one template, if the template can
/// hold them at all.
fn fit(words: &[Word], template: &Template) -> Option<Reading> {
    let slots = &template.slots;
    let mut start = Reading {
        cost: Cost::default(),
        slots: Vec::new(),
        prons: Vec::new(),
        licences: Vec::new(),
    };
    if let Some(v) = template.licence {
        start.licence(v);
    }
    let mut at: Vec<Option<Reading>> = vec![None; slots.len() + 1];
    at[0] = Some(start);
    for word in words {
        let mut next: Vec<Option<Reading>> = vec![None; slots.len() + 1];
        for (j, reading) in at.iter().enumerate() {
            let Some(reading) = reading else { continue };
            for (p, pron) in word.sound.prons.iter().enumerate() {
                // A reading at index j has filled the slots before j; each pass
                // places one more syllable.
                let mut cur: Vec<Option<Reading>> = vec![None; slots.len() + 1];
                cur[j] = Some(reading.clone());
                for weight in weights(word, &pron.stresses) {
                    let mut step: Vec<Option<Reading>> = vec![None; slots.len() + 1];
                    for (j, r) in cur.iter().enumerate() {
                        let Some(r) = r else { continue };
                        place(slots, j, r.clone(), weight, &mut step);
                    }
                    cur = step;
                }
                for (j, r) in cur.into_iter().enumerate() {
                    if let Some(mut r) = r {
                        r.prons.push(p);
                        keep(&mut next[j], r);
                    }
                }
            }
        }
        at = next;
    }
    // Whatever slots remain must be ones a reading can leave empty.
    let mut best: Option<Reading> = None;
    for (j, r) in at.into_iter().enumerate() {
        let Some(mut r) = r else { continue };
        if skip_to_end(slots, j, &mut r) {
            keep(&mut best, r);
        }
    }
    best
}

/// Place one syllable of weight `weight` at slot `j` or a later one, skipping
/// the slots between when they may be left empty.
fn place(
    slots: &[Slot],
    mut j: usize,
    mut r: Reading,
    weight: Weight,
    step: &mut [Option<Reading>],
) {
    while let Some(slot) = slots.get(j) {
        let mut filled = r.clone();
        match weight {
            Weight::Fixed(s) if s != slot.stressed => filled.cost.faults += 1,
            Weight::Free(s) if s != slot.stressed => filled.cost.strain += 1,
            _ => {}
        }
        if let Kind::Extra(v) = slot.kind {
            filled.licence(v);
        }
        filled.slots.push(j);
        keep(&mut step[j + 1], filled);
        match slot.kind {
            Kind::Required => return,
            Kind::Extra(_) => {}
            Kind::Droppable(v) => r.licence(v),
        }
        j += 1;
    }
}

/// Leave every slot from `j` empty, if they all may be; charge the licences.
fn skip_to_end(slots: &[Slot], j: usize, r: &mut Reading) -> bool {
    for slot in slots.iter().skip(j) {
        match slot.kind {
            Kind::Required => return false,
            Kind::Extra(_) => {}
            Kind::Droppable(v) => r.licence(v),
        }
    }
    true
}

/// Scan one line.
fn scan_line(
    line_no: usize,
    stanza: usize,
    line: &str,
    words: &[Word],
    meter: &Meter,
    feet: usize,
) -> LineScan {
    let expected = feet * meter.foot.pattern().len();
    let templates = templates(meter.foot, feet);
    let best = templates
        .iter()
        .filter_map(|t| fit(words, t).map(|r| (r, t)))
        .min_by_key(|(r, _)| r.cost.score());
    let mut scan = LineScan {
        line: line_no,
        stanza,
        text: line.to_string(),
        syllables: 0,
        expected,
        marks: String::new(),
        words: Vec::new(),
        variations: Vec::new(),
        deviations: Vec::new(),
    };
    match best {
        Some((reading, template)) => read_fit(&mut scan, words, &reading, template, meter),
        None => read_misfit(&mut scan, words, &templates),
    }
    scan
}

/// Fill in a line that a template holds: the reading's marks, its licences,
/// and any fixed stress it had to put on the wrong beat.
fn read_fit(
    scan: &mut LineScan,
    words: &[Word],
    reading: &Reading,
    template: &Template,
    meter: &Meter,
) {
    let slots = &template.slots;
    let mut syllable = 0;
    let mut last_foot = None;
    // The feet with a fixed stress on the wrong beat, and the first such word.
    let mut faulty: Vec<(usize, usize)> = Vec::new();
    for (w, word) in words.iter().enumerate() {
        let pron = &word.sound.prons[reading.prons[w]];
        let mut stresses = Vec::new();
        let mut off = false;
        for weight in weights(word, &pron.stresses) {
            let slot = slots[reading.slots[syllable]];
            let stressed = match weight {
                Weight::Fixed(s) if s != slot.stressed => {
                    off = true;
                    if !faulty.iter().any(|&(f, _)| f == slot.foot) {
                        faulty.push((slot.foot, w));
                    }
                    s
                }
                _ => slot.stressed,
            };
            if last_foot.is_some_and(|f| f != slot.foot) {
                scan.marks.push(' ');
            }
            last_foot = Some(slot.foot);
            scan.marks.push(if stressed { BEAT } else { OFFBEAT });
            stresses.push(stressed);
            syllable += 1;
        }
        scan.words.push(WordScan {
            range: word.range.clone(),
            stresses,
            guessed: word.sound.guessed,
            off,
        });
    }
    scan.syllables = syllable;
    for (foot, w) in faulty {
        let word = scan.text[words[w].range.clone()].to_string();
        // The foot's syllables as the words lean, against the foot reversed.
        let lean: Vec<bool> = (0..syllable)
            .filter(|&s| slots[reading.slots[s]].foot == foot)
            .filter_map(|s| syllable_weight(words, reading, s).map(Weight::lean))
            .collect();
        let reversed: Vec<bool> = meter.foot.pattern().iter().rev().copied().collect();
        scan.deviations
            .push(if lean == reversed && lean != meter.foot.pattern() {
                Deviation::InvertedFoot {
                    foot: foot + 1,
                    word,
                }
            } else {
                Deviation::MisplacedStress {
                    foot: foot + 1,
                    word,
                }
            });
    }
    let mut licences = reading.licences.clone();
    licences.dedup();
    if meter.strict {
        scan.deviations
            .extend(licences.into_iter().map(Deviation::Licence));
    } else {
        scan.variations = licences;
    }
}

/// The weight of the line's `s`th syllable in the chosen reading.
fn syllable_weight(words: &[Word], reading: &Reading, s: usize) -> Option<Weight> {
    let mut left = s;
    for (w, word) in words.iter().enumerate() {
        let pron = &word.sound.prons[reading.prons[w]];
        let ws = weights(word, &pron.stresses);
        if left < ws.len() {
            return ws.get(left).copied();
        }
        left -= ws.len();
    }
    None
}

/// Fill in a line no template can hold: the words as they lean, and whether it
/// runs long or short.
fn read_misfit(scan: &mut LineScan, words: &[Word], templates: &[Template]) {
    for word in words {
        let pron = &word.sound.prons[0];
        let stresses: Vec<bool> = weights(word, &pron.stresses)
            .into_iter()
            .map(Weight::lean)
            .collect();
        for &s in &stresses {
            scan.marks.push(if s { BEAT } else { OFFBEAT });
        }
        scan.syllables += stresses.len();
        scan.words.push(WordScan {
            range: word.range.clone(),
            stresses,
            guessed: word.sound.guessed,
            off: false,
        });
    }
    let longest = templates.iter().map(|t| t.slots.len()).max().unwrap_or(0);
    let deviation = if scan.syllables > longest {
        Deviation::ExtraSyllables(scan.syllables.saturating_sub(scan.expected).max(1))
    } else {
        Deviation::MissingSyllables(scan.expected.saturating_sub(scan.syllables).max(1))
    };
    scan.deviations.push(deviation);
}

/// Totals over a scanned piece.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeterSummary {
    pub lines: usize,
    /// Lines that keep to the meter, licences included.
    pub fitting: usize,
    /// Of those, the lines that take a licence.
    pub with_licences: usize,
    /// Each stanza's fitting lines and total lines, in order.
    pub stanzas: Vec<(usize, usize)>,
    /// The most common kind of deviation and how many lines show it.
    pub most_common: Option<(&'static str, usize)>,
}

/// Sum up a scan.
pub fn summarize(lines: &[LineScan]) -> MeterSummary {
    let mut summary = MeterSummary {
        lines: lines.len(),
        ..MeterSummary::default()
    };
    let mut kinds: Vec<(&'static str, usize)> = Vec::new();
    for line in lines {
        if summary.stanzas.len() <= line.stanza {
            summary.stanzas.resize(line.stanza + 1, (0, 0));
        }
        let stanza = &mut summary.stanzas[line.stanza];
        stanza.1 += 1;
        if line.fits() {
            stanza.0 += 1;
            summary.fitting += 1;
            if !line.variations.is_empty() {
                summary.with_licences += 1;
            }
        }
        let mut seen: Vec<&'static str> = Vec::new();
        for kind in line.deviations.iter().map(Deviation::kind) {
            if seen.contains(&kind) {
                continue;
            }
            seen.push(kind);
            match kinds.iter_mut().find(|(k, _)| *k == kind) {
                Some(entry) => entry.1 += 1,
                None => kinds.push((kind, 1)),
            }
        }
    }
    // Stanzas the scan never reached (all structure) aren't stanzas.
    summary.stanzas.retain(|&(_, total)| total > 0);
    summary.most_common = kinds.into_iter().max_by_key(|&(_, n)| n);
    summary
}
