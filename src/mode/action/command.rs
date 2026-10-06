//! The [`Command`] a palette emits on Enter, and the small picker/knob enums the
//! Settings and export-style menus branch on ([`SettingPicker`], [`SettingBool`],
//! [`SettingTextKnob`], [`StylePicker`], [`StyleTextKnob`], [`FontTarget`],
//! [`PortraitChoice`]). Split out of `action.rs` to keep each file under the
//! ~1,000-line convention; re-exported from the module root, so callers are
//! unaffected. Pure move.

use super::{Action, AiAction, ExportFormat, ExportScope, GitAction};

use crate::analysis::FlagCategory;

/// A command reachable from a palette — the AI menu (Ctrl-A), the version-control
/// menu (Ctrl-V), and the export menu (Ctrl-E) share one palette mechanism.
#[derive(Clone)]
pub enum Command {
    Ai(AiAction),
    Git(GitAction),
    Export(ExportFormat, ExportScope),
    /// Open a styled sample EPUB of the current `[style]` settings in the system
    /// reader (#483), so an author can see a preset + overrides before exporting.
    PreviewStyle,
    /// Open a sample print-interior PDF (#1107) — the sample chapter rendered through
    /// the print PDF writer at the current `[print]` trim/bleed and `[style]` — in the
    /// system PDF viewer, so an author can see the drop cap, heading alignment, trim,
    /// and margins before running a full print export.
    PreviewPrintInterior,
    /// Run a registry [`Action`] from the unified run-action menu (#225). This is
    /// the going-forward path; `Ai`/`Export` above remain as their own variants
    /// only because the AI and export palettes still emit them directly — folding
    /// those palettes onto the registry is follow-up work (#226).
    Run(Action),
    /// Run the author workflow at this index in `App::workflows` (#226).
    RunWorkflow(usize),
    /// Start building a workflow from a plain-language description (#232).
    NewWorkflowFromPrompt,
    /// Generate portraits for the chosen character(s) (#244).
    Portrait(PortraitChoice),
    /// Make the book at this index the active book (#329).
    SwitchBook(usize),
    /// Add a new book to the series (#648) — the manage-books palette's entry that
    /// prompts for the new book's title.
    AddBook,
    /// Remove the book at this index from the series (#648) — opens a y/N confirm
    /// before its chapters are retired to `unused/`.
    RemoveBook(usize),
    /// Move the book at this index one place up (`up`) or down in the series order
    /// (#648) — a pure reorder of the `[[books]]` list, no files move.
    MoveBook {
        index: usize,
        up: bool,
    },
    /// Create the pending new project (its path stashed in `App::pending_manage`)
    /// with the structure template at this index in [`crate::project::TEMPLATES`]
    /// (#611).
    NewProjectTemplate(usize),
    /// Insert the author-defined snippet at this index in `App::snippets` (#411).
    InsertSnippet(usize),
    /// Run the name generator for the chosen kind (#610) — picked from the small
    /// kind menu, this opens the constraints prompt.
    PickNameKind(crate::ai::namegen::Kind),
    /// Place the chosen candidate name at the cursor (#610) — the pick step of the
    /// name generator's option list, mirroring [`Command::InsertSnippet`].
    InsertName(String),
    /// Place the chosen candidate word/phrase at the cursor (#653) — the pick step
    /// of the thesaurus / word-search option list, mirroring
    /// [`Command::InsertName`]. The author singled out one slot and chose one
    /// candidate; this inserts that word verbatim, nothing else.
    InsertWord(String),
    /// Replace the misspelled word in the flagged byte range `[start, end)` with the
    /// chosen correction (#647) — the pick step of the spell-suggestions list. The
    /// author singled out the flagged word and chose from the offline candidates;
    /// this swaps that one word, nothing else. `word` is the original, for the
    /// status line.
    ReplaceMisspelling {
        start: usize,
        end: usize,
        word: String,
        replacement: String,
    },
    /// Start the style-flag sweep for the chosen category (#1001) — the pick step
    /// of the sweep's category menu. Turns on the underline and jumps to the first
    /// occurrence; `]r` / `[r` then step through them.
    PickStyleFlag(FlagCategory),
    /// Turn the style-flag sweep off (#1001) — clears the active category and its
    /// underlines. The "(off)" entry at the top of the sweep's category menu.
    ClearStyleFlags,
    /// Open the role picker for the curated font family at this index in
    /// [`crate::fonts::FAMILIES`] (#494) — body, heading, or both.
    PickFontRole(usize),
    /// Download the font family at this index and set it for `target` in the
    /// project's `[style]` (#494).
    ApplyFont(usize, FontTarget),
    /// Open one of the export-style sub-pickers (#569) — preset, heading
    /// alignment, drop caps, or print trim — from the style setter menu.
    OpenStylePicker(StylePicker),
    /// Set (or clear) the `[style]` preset (#569): the index into the preset
    /// library, or `None` to clear it back to no preset.
    SetStylePreset(Option<usize>),
    /// Set (or clear) `[style] heading_align` (#569): the index into the
    /// alignment allowlist, or `None` for the default (centered).
    SetStyleAlign(Option<usize>),
    /// Set (or clear) `[style] drop_cap` (#569): `Some` to write it, `None` to
    /// clear back to the default (off).
    SetStyleDropCap(Option<bool>),
    /// Set the `[print] trim` size (#569): the index into the trim-size list.
    SetStyleTrim(usize),
    /// Open the text prompt for a free-text `[style]` knob (#569) — the scene-break
    /// glyph or a title/part ornament.
    EditStyleText(StyleTextKnob),
    /// Open one of the plain Settings menu's sub-pickers (#592) — the theme name
    /// or a boolean toggle — from the Settings menu.
    OpenSettingPicker(SettingPicker),
    /// Set a boolean `[editor]` knob from the Settings menu (#592): Vim mode,
    /// smart typography, typewriter scrolling, or focus mode.
    SetSettingBool(SettingBool, bool),
    /// Set the `[theme] name` from the Settings menu (#592): the index into the
    /// built-in theme list.
    SetSettingTheme(usize),
    /// Open the text prompt for a free-text / numeric Settings knob (#592) — the
    /// column width, the autosave interval, or the analysis stopwords.
    EditSettingText(SettingTextKnob),
}

/// One of the plain Settings menu's discrete sub-pickers (#592) — a small list the
/// menu opens, each emitting a `Set…` command on Enter. (The numeric / list knobs
/// use a prompt instead; see [`SettingTextKnob`].)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingPicker {
    /// The built-in color-theme list (dark / light).
    Theme,
    /// An on/off picker for one boolean `[editor]` knob.
    Bool(SettingBool),
    /// The per-field `[theme]` color overrides (#1109): a row per color field,
    /// each showing its current value and opening a text prompt to override it.
    ThemeColors,
    /// The Vim `[editor.keys]` key bindings (#1109): a row per remappable action
    /// showing its current key, each opening a prompt to remap it.
    KeyBindings,
}

/// A boolean `[editor]` knob the plain Settings menu toggles (#592). Each maps to a
/// dotted config key validated through the shared vocabulary (`editor.vim`, …).
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

impl SettingBool {
    /// The dotted config key this knob writes, matched by the shared validator.
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Vim => "editor.vim",
            Self::SmartTypography => "editor.smart_typography",
            Self::AutoPair => "editor.auto_pair",
            Self::Typewriter => "editor.typewriter",
            Self::FocusMode => "editor.focus_mode",
            Self::ParagraphIndent => "editor.paragraph_indent",
            Self::AnnotationMargin => "editor.annotation_margin",
            Self::DailyNudge => "editor.daily_nudge",
        }
    }

    /// A human label for the menu row, picker title, and status message.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Vim => "Vim mode",
            Self::SmartTypography => "Smart typography",
            Self::AutoPair => "Auto-pair brackets",
            Self::Typewriter => "Typewriter scrolling",
            Self::FocusMode => "Focus mode",
            Self::ParagraphIndent => "Paragraph indent",
            Self::AnnotationMargin => "Annotation margin",
            Self::DailyNudge => "Daily goal cue",
        }
    }
}

/// A numeric / free-text Settings knob the plain menu edits through a prompt (#592):
/// the centered column width, the autosave interval, and the analysis stopwords.
/// The two `[editor]` knobs validate through the shared vocabulary and persist via
/// the config splicer; the stopwords splice into `book.toml`'s `[analysis]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingTextKnob {
    /// The centered text column width (`editor.column_width`).
    ColumnWidth,
    /// The autosave interval in seconds (`editor.autosave_interval_secs`).
    AutosaveSecs,
    /// The words added to the analysis report's stopword list
    /// (`[analysis] stopwords_add` in `book.toml`).
    Stopwords,
    /// One `[theme]` color field's override (#1109), keyed by its config field
    /// name (a `theme::COLOR_FIELDS` entry, e.g. `"heading"`). Validated as a
    /// color and spliced into `[theme]`; a blank entry clears the override back to
    /// the base theme's value.
    ThemeColor(&'static str),
    /// One Vim `[editor.keys]` remap (#1109), keyed by the action name (a
    /// `dispatch::VIM_ACTIONS` entry, e.g. `"down"`). Validated as a single key and
    /// spliced into `[editor.keys]`; a blank entry clears the remap back to the
    /// action's default key.
    KeyRemap(&'static str),
}

impl SettingTextKnob {
    /// A human label for the menu row, prompt, and status message.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::ColumnWidth => "column width",
            Self::AutosaveSecs => "autosave interval (seconds)",
            Self::Stopwords => "analysis stopwords",
            Self::ThemeColor(field) => field,
            Self::KeyRemap(action) => action,
        }
    }
}

/// One of the export-style setter's discrete sub-pickers (#569) — a small list the
/// menu opens, each emitting a `Set…` command on Enter. (The free-text knobs use a
/// prompt instead; see [`StyleTextKnob`].)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StylePicker {
    /// The built-in preset library (or "(none)").
    Preset,
    /// The heading-alignment allowlist (or "(default)").
    Align,
    /// On / off / default for drop caps.
    DropCap,
    /// The supported print trim sizes.
    Trim,
}

/// A free-text `[style]` knob the in-app setter edits through a prompt (#569) —
/// the divider glyph, the title/part ornaments, and the cover-image path (#582),
/// each a short string the writers render / embed verbatim. (The font/decorative
/// image paths have their own browser, #494.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleTextKnob {
    /// The scene-break divider glyph (`scene_break`).
    SceneBreak,
    /// The title-page ornament (`title_ornament`).
    TitleOrnament,
    /// The part-divider ornament (`part_ornament`).
    PartOrnament,
    /// The book cover-image path (`cover_image`, #582).
    CoverImage,
    /// The cover image's alt text / description (`cover_image_alt`, #818).
    CoverImageAlt,
}

impl StyleTextKnob {
    /// The `[style]` key this knob is stored under in `book.toml`.
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::SceneBreak => "scene_break",
            Self::TitleOrnament => "title_ornament",
            Self::PartOrnament => "part_ornament",
            Self::CoverImage => "cover_image",
            Self::CoverImageAlt => "cover_image_alt",
        }
    }

    /// A human label for the menu row, prompt, and status message.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::SceneBreak => "scene-break glyph",
            Self::TitleOrnament => "title ornament",
            Self::PartOrnament => "part ornament",
            Self::CoverImage => "cover image",
            Self::CoverImageAlt => "cover image description",
        }
    }
}

/// Which `[style]` font slot(s) a chosen curated font fills (#494) — picked after
/// the family, before the download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontTarget {
    /// Set `body_font` only.
    Body,
    /// Set `heading_font` only.
    Heading,
    /// Set both `body_font` and `heading_font` to this family.
    Both,
}

/// Which character(s) a portrait pass should generate (#244), chosen from the
/// character-portrait picker.
#[derive(Clone, Copy)]
pub enum PortraitChoice {
    /// Every character in the bible.
    All,
    /// The character at this index in the (sorted) bible list.
    One(usize),
}
