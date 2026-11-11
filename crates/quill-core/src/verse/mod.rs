//! Verse analysis (#1329): checking a chapter or story written in verse against
//! the meter its author is aiming for. Purely algorithmic and offline, from a
//! bundled pronouncing dictionary. It observes and marks; it never proposes a
//! word or a line, so the verse stays the author's (the analysis side of the AI
//! boundary in CLAUDE.md).
//!
//! - [`meter`]: the target meter, as the author names it in front matter.
//! - [`pronounce`]: how a word sounds: its syllables, their stress, its phonemes.
//! - [`scan`]: scanning each line against the target, with the licences and
//!   deviations named.
//! - [`rhyme`]: the target rhyme scheme, and checking each line's ending
//!   against it (#1325).
//! - [`lines`]: which lines are verse, shared by both.
//! - [`lexicon`]: the author's own pronunciations for the project (#1328).
//! - [`marks`]: the scans as the editor's gutter and in-text marks (#1326).
//!
//! `docs/manuscript-analysis.md` explains the method and where it falls short.

pub mod lexicon;
#[cfg(test)]
mod lexicon_tests;
pub mod lines;
pub mod marks;
#[cfg(test)]
mod marks_tests;
pub mod meter;
pub mod pronounce;
pub mod rhyme;
#[cfg(test)]
mod rhyme_tests;
pub mod scan;
#[cfg(test)]
mod tests;

pub use lexicon::{Entry, Lexicon, PRONUNCIATIONS_NAME};
pub use marks::{marks, GutterLine, Marks, MeterCell, RhymeCell};
pub use meter::{Foot, Meter, METER_EXAMPLES};
pub use rhyme::{
    check_rhyme, check_rhyme_with, letter_name, summarize_rhyme, RhymeFault, RhymeKind, RhymeLine,
    RhymeScheme, RhymeSummary, RHYME_EXAMPLES,
};
pub use scan::{
    scan_meter, scan_meter_with, summarize, Deviation, LineScan, MeterSummary, Variation, WordScan,
    BEAT, OFFBEAT,
};
