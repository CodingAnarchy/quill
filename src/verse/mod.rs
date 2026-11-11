//! The core's verse analysis ([`quill_core::verse`]), re-exported, plus its
//! terminal rendering.

pub use quill_core::verse::*;

mod render;
pub use render::*;
