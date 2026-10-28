//! The core's configuration ([`quill_core::config`]), re-exported, plus loading
//! the terminal color theme.

pub use quill_core::config::*;

mod theme;
pub use theme::load_theme;
