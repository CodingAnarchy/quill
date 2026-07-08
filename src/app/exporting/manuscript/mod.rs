//! Manuscript export (#7): open the export palette, compile the live project
//! (active book or whole series #332) into the shared [`crate::export::Document`],
//! and write it out. Format-specific rendering lives in `crate::export`; here is
//! the palette wiring, the structure assembly, and the file-writing plumbing.
//!
//! Split by concern (#926) — every file extends the one `impl App`, re-exported
//! so callers are unaffected:
//! - [`palette`] — the export palette/dispatch and the scoped output naming.
//! - [`assembly`] — compiling the live project into the export [`Document`] tree.
//! - [`write`] — the file-writing plumbing and the print wrap-cover export.
//! - [`publish`] — the preflight and the one-click "Prepare for …" publish bundles.
//! - [`review`] — the annotated review-copy export.

mod assembly;
mod palette;
mod publish;
mod review;
mod write;

pub use publish::PublishTarget;
