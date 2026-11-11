//! The in-editor scansion marks (#1326): the meter and rhyme checks for a span of
//! the edit buffer, turned into what the editor draws. A gutter row for each
//! verse line (its stress reading, syllables, and rhyme letter), and the byte
//! spans to mark in the text, faults apart from licences. The editor computes
//! these on the idle tick and draws from the cache; it never changes the text.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use super::lexicon::Lexicon;
use super::meter::Meter;
use super::rhyme::{check_rhyme_with, letter_name, RhymeScheme};
use super::scan::{scan_meter_with, Deviation, LineScan, Variation};
use crate::atomic;
use crate::synced::state_dir;

/// What the gutter shows beside one verse line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GutterLine {
    /// The line's start, as a byte offset in the buffer.
    pub line_start: usize,
    pub meter: Option<MeterCell>,
    pub rhyme: Option<RhymeCell>,
}

/// The meter half of a gutter line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeterCell {
    /// The stress reading, grouped into feet: `˘/ ˘/ ˘/ ˘/ ˘/`.
    pub marks: String,
    pub syllables: usize,
    pub expected: usize,
    pub fits: bool,
    /// Whether it fits by way of a licence.
    pub licence: bool,
}

/// The rhyme half of a gutter line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RhymeCell {
    /// The letter the line's ending earns.
    pub letter: String,
    pub fits: bool,
}

/// The marks for a span of the buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    /// Gutter lines, in buffer order.
    pub lines: Vec<GutterLine>,
    /// Text that breaks the meter or the scheme, start-sorted and disjoint.
    pub faults: Vec<(usize, usize)>,
    /// Text that takes a licence, start-sorted and disjoint.
    pub licences: Vec<(usize, usize)>,
}

impl Marks {
    /// Add another span's marks after these (the next chapter of the continuous
    /// view). The spans stay sorted because the spans come in buffer order.
    pub fn extend(&mut self, other: Self) {
        self.lines.extend(other.lines);
        self.faults.extend(other.faults);
        self.licences.extend(other.licences);
    }

    /// The start of every line that breaks the meter or the scheme, in order:
    /// where `]m` / `[m` step.
    pub fn fault_lines(&self) -> Vec<usize> {
        self.lines
            .iter()
            .filter(|l| {
                l.meter.as_ref().is_some_and(|m| !m.fits)
                    || l.rhyme.as_ref().is_some_and(|r| !r.fits)
            })
            .map(|l| l.line_start)
            .collect()
    }
}

/// The marks for `text`, which starts at byte `offset` of the buffer, against
/// the chapter's targets. Nothing to check gives no marks.
pub fn marks(
    text: &str,
    offset: usize,
    meter: Option<&Meter>,
    rhyme: Option<&RhymeScheme>,
    lexicon: &Lexicon,
) -> Marks {
    if meter.is_none() && rhyme.is_none() {
        return Marks::default();
    }
    let starts = line_starts(text);
    let at = |line: usize, range: &Range<usize>| {
        let base = offset + starts.get(line).copied().unwrap_or(0);
        (base + range.start, base + range.end)
    };
    let scans = meter.map_or_else(Vec::new, |m| scan_meter_with(text, m, lexicon));
    let rhymes = rhyme.map_or_else(Vec::new, |r| check_rhyme_with(text, r, lexicon));
    let mut faults = Vec::new();
    let mut licences = Vec::new();
    let mut lines: BTreeMap<usize, GutterLine> = BTreeMap::new();
    for scan in &scans {
        let (fault_spans, licence_spans) = meter_spans(scan);
        faults.extend(fault_spans.iter().map(|r| at(scan.line, r)));
        licences.extend(licence_spans.iter().map(|r| at(scan.line, r)));
        gutter_line(
            &mut lines,
            offset + starts.get(scan.line).copied().unwrap_or(0),
        )
        .meter = Some(MeterCell {
            marks: scan.marks.clone(),
            syllables: scan.syllables,
            expected: scan.expected,
            fits: scan.fits(),
            licence: !scan.variations.is_empty(),
        });
    }
    for rhyme in &rhymes {
        if !rhyme.fits() {
            faults.push(at(rhyme.line, &rhyme.range));
        }
        gutter_line(
            &mut lines,
            offset + starts.get(rhyme.line).copied().unwrap_or(0),
        )
        .rhyme = Some(RhymeCell {
            letter: letter_name(rhyme.heard),
            fits: rhyme.fits(),
        });
    }
    let lines: Vec<GutterLine> = lines.into_values().collect();
    Marks {
        lines,
        faults: merged(faults),
        licences: merged(licences),
    }
}

/// The gutter line starting at `start`, made empty if it isn't there yet.
fn gutter_line(lines: &mut BTreeMap<usize, GutterLine>, start: usize) -> &mut GutterLine {
    lines.entry(start).or_insert(GutterLine {
        line_start: start,
        meter: None,
        rhyme: None,
    })
}

/// The byte offset each line of `text` starts at.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// A scanned line's text to mark, within the line: the faults (the words on a
/// wrong beat, or the whole line when its length is wrong, and a licence the
/// strict meter refuses) and the licences it takes.
fn meter_spans(scan: &LineScan) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let whole = || match (scan.words.first(), scan.words.last()) {
        (Some(first), Some(last)) => Some(first.range.start..last.range.end),
        _ => None,
    };
    let mut faults: Vec<Range<usize>> = scan
        .words
        .iter()
        .filter(|w| w.off)
        .map(|w| w.range.clone())
        .collect();
    for deviation in &scan.deviations {
        match deviation {
            Deviation::ExtraSyllables(_) | Deviation::MissingSyllables(_) => faults.extend(whole()),
            Deviation::Licence(v) => faults.extend(licence_word(scan, *v)),
            Deviation::InvertedFoot { .. } | Deviation::MisplacedStress { .. } => {}
        }
    }
    let licences = scan
        .variations
        .iter()
        .filter_map(|v| licence_word(scan, *v))
        .collect();
    (faults, licences)
}

/// The word a licence sits on: the first word for one at the start of the line,
/// the last for one at the end. A substituted foot has no one word to point at.
fn licence_word(scan: &LineScan, v: Variation) -> Option<Range<usize>> {
    let word = match v {
        Variation::Headless | Variation::InitialInversion | Variation::Anacrusis => {
            scan.words.first()
        }
        Variation::FeminineEnding | Variation::Catalectic => scan.words.last(),
        Variation::Substitution => None,
    };
    word.map(|w| w.range.clone())
}

/// Spans sorted and merged where they touch or overlap.
fn merged(mut spans: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    spans.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (s, e) in spans {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// The file whose presence in the project's state folder means the author left
/// the scansion marks on for this project.
const SHOWN_FILE: &str = "scansion-marks";

fn shown_path(root: &Path) -> PathBuf {
    state_dir(root).join(SHOWN_FILE)
}

/// Whether the author left the scansion marks on for the project at `root`.
pub fn shown(root: &Path) -> bool {
    shown_path(root).exists()
}

/// Remember whether the scansion marks are on for the project at `root`. A view
/// preference, not manuscript data, so it lives in the state folder beside the
/// last cursor position.
pub fn set_shown(root: &Path, on: bool) -> io::Result<()> {
    let path = shown_path(root);
    if on {
        atomic::create_leaf_dir(&state_dir(root))?;
        atomic::write_hint(&path, b"")
    } else {
        match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}
