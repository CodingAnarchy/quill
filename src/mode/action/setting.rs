//! The boolean `[editor]` knobs ([`SettingBool`]) and their descriptor table
//! ([`SettingBool::SPECS`]): one row per knob carrying its config field, label,
//! settings-vocabulary description, Settings-menu placement, optional palette
//! toggle, and accessor into [`EditorFlags`]. Mirrors [`AiAction::SPECS`] — the
//! Settings menu rows, the `:set` / describe-config vocabulary and validator, the
//! run-action registry's toggle rows, the toggle dispatch, and the docs guard all
//! derive from it, so a new knob is a field on `EditorFlags` plus one row here.
//!
//! [`AiAction::SPECS`]: super::AiAction::SPECS

use crate::config::EditorFlags;

/// A boolean `[editor]` knob (#592). Each maps to a dotted config key validated
/// through the shared vocabulary (`editor.vim`, …) and a field of
/// [`EditorFlags`]; everything else about it lives in its [`SPECS`](Self::SPECS)
/// row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingBool {
    /// Vim-style modal editing (`editor.vim`).
    Vim,
    /// Smart typography as you type (`editor.smart_typography`).
    SmartTypography,
    /// Auto-pair quotes and brackets as you type (`editor.auto_pair`).
    AutoPair,
    /// Typewriter scrolling (`editor.typewriter`).
    Typewriter,
    /// Focus mode (`editor.focus_mode`).
    FocusMode,
    /// First-line paragraph indent (`editor.paragraph_indent`).
    ParagraphIndent,
    /// The annotation margin beside the prose (`editor.annotation_margin`).
    AnnotationMargin,
    /// The below-daily-goal status-bar cue (`editor.daily_nudge`).
    DailyNudge,
}

/// Where a knob's row sits in the plain Settings menu (#592), relative to the
/// valued `[editor]` rows: the typing behaviors list after the column width, the
/// display / ambient toggles after the autosave interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingPlacement {
    /// How typed input behaves (smart typography, auto-pair, Vim).
    Typing,
    /// How the page looks or what it shows (typewriter, focus, margin, cue, …).
    Display,
}

/// A knob's run-action palette toggle: the registry row that flips it for the
/// session without persisting (the Settings menu is what writes the config).
pub struct ToggleSpec {
    /// The stable registry name workflows (#89) reference — never change these
    /// without a migration.
    pub name: &'static str,
    /// The run-action menu label.
    pub label: &'static str,
    /// Whether the assistant may run the toggle as a tool.
    pub assistant_runnable: bool,
}

/// One [`SettingBool`]'s descriptor.
pub struct SettingBoolSpec {
    pub knob: SettingBool,
    /// The `[editor]` field name (the config key without its `editor.` section).
    pub field: &'static str,
    /// A human label for the menu row, picker title, and status message.
    pub label: &'static str,
    /// The settings-vocabulary description (#303) the describe-config model maps
    /// against, ending in its allowed values.
    pub describe: &'static str,
    /// Where the Settings menu lists it.
    pub placement: SettingPlacement,
    /// The run-action toggle, for the knobs that have one.
    pub toggle: Option<ToggleSpec>,
    /// Read this knob's value.
    pub get: fn(&EditorFlags) -> bool,
    /// This knob's slot, for setting / flipping it.
    pub slot: fn(&mut EditorFlags) -> &mut bool,
}

impl SettingBool {
    /// Every knob's descriptor, in Settings-menu order (within each
    /// [`SettingPlacement`]) — also the order of the settings vocabulary.
    pub const SPECS: &'static [SettingBoolSpec] = &[
        SettingBoolSpec {
            knob: Self::SmartTypography,
            field: "smart_typography",
            label: "Smart typography",
            describe: "Smart curly quotes, em-dashes, and ellipses as you type — true or false",
            placement: SettingPlacement::Typing,
            toggle: None,
            get: |f| f.smart_typography,
            slot: |f| &mut f.smart_typography,
        },
        SettingBoolSpec {
            knob: Self::AutoPair,
            field: "auto_pair",
            label: "Auto-pair brackets",
            describe: "Auto-close quotes and brackets as you type, skipping over an existing \
                       closer — true or false",
            placement: SettingPlacement::Typing,
            toggle: None,
            get: |f| f.auto_pair,
            slot: |f| &mut f.auto_pair,
        },
        SettingBoolSpec {
            knob: Self::Vim,
            field: "vim",
            label: "Vim mode",
            describe: "Vim-style modal editing — true or false",
            placement: SettingPlacement::Typing,
            toggle: None,
            get: |f| f.vim,
            slot: |f| &mut f.vim,
        },
        SettingBoolSpec {
            knob: Self::Typewriter,
            field: "typewriter",
            label: "Typewriter scrolling",
            describe: "Typewriter scrolling — pin the cursor's line to the page centre — true \
                       or false",
            placement: SettingPlacement::Display,
            toggle: Some(ToggleSpec {
                name: "view:typewriter",
                label: "Typewriter scrolling (toggle)",
                assistant_runnable: true,
            }),
            get: |f| f.typewriter,
            slot: |f| &mut f.typewriter,
        },
        SettingBoolSpec {
            knob: Self::FocusMode,
            field: "focus_mode",
            label: "Focus mode",
            describe: "Focus mode — dim every paragraph but the one under the cursor — true or \
                       false",
            placement: SettingPlacement::Display,
            toggle: Some(ToggleSpec {
                name: "view:focus",
                label: "Focus mode (toggle)",
                assistant_runnable: true,
            }),
            get: |f| f.focus_mode,
            slot: |f| &mut f.focus_mode,
        },
        SettingBoolSpec {
            knob: Self::ParagraphIndent,
            field: "paragraph_indent",
            label: "Paragraph indent",
            describe: "Indent the first line of each prose paragraph in the editor \
                       (display-only) — true or false",
            placement: SettingPlacement::Display,
            toggle: Some(ToggleSpec {
                name: "view:paragraph-indent",
                label: "Paragraph indent (toggle)",
                assistant_runnable: true,
            }),
            get: |f| f.paragraph_indent,
            slot: |f| &mut f.paragraph_indent,
        },
        SettingBoolSpec {
            knob: Self::AnnotationMargin,
            field: "annotation_margin",
            label: "Annotation margin",
            describe: "Show annotations in the margin beside the prose, level with their text \
                       — true or false",
            placement: SettingPlacement::Display,
            toggle: Some(ToggleSpec {
                name: "view:annotation-margin",
                label: "Annotation margin (toggle)",
                assistant_runnable: true,
            }),
            get: |f| f.annotation_margin,
            slot: |f| &mut f.annotation_margin,
        },
        SettingBoolSpec {
            knob: Self::DailyNudge,
            field: "daily_nudge",
            label: "Daily goal cue",
            describe: "Show how far short of the daily word goal today is, in the status bar — \
                       true or false",
            placement: SettingPlacement::Display,
            toggle: Some(ToggleSpec {
                name: "view:daily-cue",
                label: "Daily goal cue (toggle)",
                assistant_runnable: false,
            }),
            get: |f| f.daily_nudge,
            slot: |f| &mut f.daily_nudge,
        },
    ];

    /// This knob's descriptor. A `const fn` so the registry table can copy a
    /// toggle's name and label at compile time.
    pub(crate) const fn spec(self) -> &'static SettingBoolSpec {
        let specs = Self::SPECS;
        let mut i = 0;
        while i < specs.len() {
            if specs[i].knob as u8 == self as u8 {
                return &specs[i];
            }
            i += 1;
        }
        // Reached only from a const context or a test-checked table, so a missing
        // row is a compile error / test failure, not a runtime panic in practice.
        panic!("every SettingBool has a descriptor in SettingBool::SPECS");
    }

    /// The `[editor]` field this knob writes.
    pub(crate) const fn field(self) -> &'static str {
        self.spec().field
    }

    /// The dotted config key this knob writes, matched by the shared validator.
    pub(crate) fn key(self) -> String {
        format!("editor.{}", self.field())
    }

    /// A human label for the menu row, picker title, and status message.
    pub(crate) const fn label(self) -> &'static str {
        self.spec().label
    }

    /// The knob a dotted config key (`editor.<field>`) names, if it's a boolean.
    pub(crate) fn from_key(key: &str) -> Option<Self> {
        let field = key.strip_prefix("editor.")?;
        Self::SPECS
            .iter()
            .find(|spec| spec.field == field)
            .map(|spec| spec.knob)
    }

    /// This knob's current value in `flags`.
    pub(crate) fn get(self, flags: &EditorFlags) -> bool {
        (self.spec().get)(flags)
    }

    /// Flip this knob in `flags`, returning its new value.
    pub(crate) fn toggle(self, flags: &mut EditorFlags) -> bool {
        let slot = (self.spec().slot)(flags);
        *slot = !*slot;
        *slot
    }
}

#[cfg(test)]
mod tests {
    use super::SettingBool;
    use crate::config::EditorFlags;
    use crate::project::extra_keys::field_names;

    /// The table is complete and one-to-one with `EditorFlags`: every knob has
    /// exactly one row, and the rows' fields are exactly the struct's serde field
    /// names — so a flag added to the struct without a row (and so missing from the
    /// Settings menu, vocabulary, and docs guard) fails here.
    #[test]
    fn specs_cover_every_knob_and_every_flag_field() {
        for spec in SettingBool::SPECS {
            let rows = SettingBool::SPECS
                .iter()
                .filter(|s| s.knob == spec.knob)
                .count();
            assert_eq!(rows, 1, "{:?} must have exactly one row", spec.knob);
            assert_eq!(spec.knob.spec().field, spec.field);
        }
        let mut fields: Vec<&str> = SettingBool::SPECS.iter().map(|s| s.field).collect();
        fields.sort_unstable();
        let mut struct_fields = field_names::<EditorFlags>().to_vec();
        struct_fields.sort_unstable();
        assert_eq!(fields, struct_fields);
    }

    /// Each row's accessors address its own field, and only that one: flipping a
    /// knob changes exactly one flag, and `get` reads it back.
    #[test]
    fn each_accessor_flips_exactly_its_own_flag() {
        for spec in SettingBool::SPECS {
            let before = EditorFlags::default();
            let mut after = before;
            let on = spec.knob.toggle(&mut after);
            assert_eq!(on, !spec.knob.get(&before), "{:?}", spec.knob);
            assert_eq!(spec.knob.get(&after), on, "{:?}", spec.knob);
            for other in SettingBool::SPECS.iter().filter(|s| s.knob != spec.knob) {
                assert_eq!(
                    other.knob.get(&after),
                    other.knob.get(&before),
                    "toggling {:?} changed {:?}",
                    spec.knob,
                    other.knob
                );
            }
            assert_eq!(SettingBool::from_key(&spec.knob.key()), Some(spec.knob));
        }
    }
}
