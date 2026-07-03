//! Export orchestration (#7): assemble the live project into the shared
//! [`crate::export::Document`] and write it out. The structure assembly is here;
//! format-specific rendering lives in `crate::export`. Split out of `app/mod.rs`;
//! `App` methods via `use super::*`.
//!
//! Split by concern (#761):
//! - [`manuscript`] — the export palette, document/series assembly, file writing.
//! - [`style`] — designed-export styling, warnings, and the style preview.
//! - [`story_bible`] — the Markdown story-bible export.

mod manuscript;
mod story_bible;
mod style;
