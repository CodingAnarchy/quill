//! Transient mode-state structs (#404): the overlay, search, and review state
//! that each [`Mode`](super::Mode) variant carries. Split out of `mode.rs` to keep
//! the core enums under the ~1,000-line convention, then further split by
//! overlay/mode family (each submodule is a pure move). Re-exported flat so every
//! state type stays reachable as `crate::mode::<Type>`, exactly as before.

mod browser;
mod findings;
mod history;
mod import;
mod input;
mod review;
mod search;

pub use browser::*;
pub use findings::*;
pub use history::*;
pub use import::*;
pub use input::*;
pub use review::*;
pub use search::*;
