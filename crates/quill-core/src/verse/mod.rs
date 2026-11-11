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
//!
//! `docs/manuscript-analysis.md` explains the method and where it falls short.

pub mod meter;
pub mod pronounce;
pub mod scan;
#[cfg(test)]
mod tests;

pub use meter::{Foot, Meter, METER_EXAMPLES};
pub use scan::{
    scan_meter, summarize, Deviation, LineScan, MeterSummary, Variation, WordScan, BEAT, OFFBEAT,
};
