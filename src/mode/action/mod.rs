//! The action / command / palette layer (#556): the unified run-action registry
//! ([`Action`]), the menu/command enums the palettes emit ([`Command`] and the
//! export/AI/git target enums), and the collapsible palette types ([`Palette`]).
//! Split out of `mode.rs` to keep the core mode enums under the ~1,000-line
//! convention; re-exported from there, so callers (`crate::mode::{Action, …}`)
//! are unaffected. Pure move.
//!
//! Grown past the ~1,000-line convention itself (#714), this is now a directory
//! module split into cohesive files — the palette types ([`palette`]), the
//! [`Command`] enum and its Settings/style pickers ([`command`]), the [`Action`]
//! registry with the export/git targets ([`registry`]), its row-per-action table
//! ([`table`]), the [`AiAction`] descriptor table ([`ai`]), and the boolean
//! `[editor]` knob descriptor table ([`setting`]) — each re-exported
//! here so the public surface, and every caller, is unchanged.

mod ai;
mod command;
mod palette;
mod registry;
mod setting;
mod table;

pub use ai::{AiAction, AiCost, ModelWeight, Needs};
pub use command::{
    Command, FontTarget, PortraitChoice, SettingPicker, SettingTextKnob, StoryCommand, StylePicker,
    StyleTextKnob,
};
pub use palette::PaletteRow;
pub use palette::{Palette, PaletteItem};
pub use registry::{Action, ExportFormat, ExportScope, GitAction};
pub use setting::{SettingBool, SettingPlacement};
