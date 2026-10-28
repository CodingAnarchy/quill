//! The core's prose analysis ([`quill_core::analysis`]), re-exported, plus its
//! terminal rendering.

pub use quill_core::analysis::*;

mod render;
pub use render::*;
