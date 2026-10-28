//! The core's AI layer ([`quill_core::ai`]), re-exported, plus the two pieces
//! that belong to the terminal editor: rendering an answer for the overlay and
//! the assistant's tool catalog, which is built from the editor's actions.

pub use quill_core::ai::*;

mod render;
pub mod tools;

pub use render::answer_to_lines;
