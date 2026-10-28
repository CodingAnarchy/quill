//! Quill's editing core (#1229): everything a front end needs to open, edit,
//! save, analyze, export, and sync a novel project, with no terminal UI. The
//! `quill` terminal editor is one front end over it; the iPhone app (through
//! `quill-ffi`) is another.
//!
//! The rule that keeps it reusable: nothing here depends on `ratatui`,
//! `crossterm`, or the editor's modes. Drawing, key handling, and the color
//! theme live in the binary.

pub mod ai;
pub mod analysis;
pub mod annotation;
pub mod archive;
pub mod atomic;
pub mod bgwrite;
pub mod buffer;
pub mod config;
#[cfg(test)]
mod datapath_tests;
pub mod date;
pub mod export;
pub mod findings_store;
pub mod frontmatter;
pub mod history;
pub mod lastpos;
pub mod manuscript;
pub mod manuscript_import;
pub mod project;
pub mod recent;
pub mod recovery;
pub mod scenes;
pub mod spell;
pub mod syntax;
#[cfg(any(test, feature = "test-support"))]
pub mod testutil;
pub mod text;
pub mod vcs;
pub mod wrap;
