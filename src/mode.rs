//! Editor modes: the transient state that drives key handling. Fields the
//! handlers read or write are `pub(crate)`; `label`/`input` are `pub` because
//! the status bar and the edit banner render them.

use std::path::PathBuf;

use ratatui::text::Line;

use crate::buffer::Buffer;

/// A full-screen, read-only overlay drawn above any mode: the progress report,
/// the analysis report, or an AI answer. Holds its pre-rendered lines, how to
/// render itself, and the current scroll offset. `App` keeps a single
/// `Option<Overlay>`, so a new overlay is a new value here — not a new field
/// plus a close branch plus a draw arm.
pub struct Overlay {
    pub lines: Vec<Line<'static>>,
    pub kind: OverlayKind,
    /// First visible row; 0 when freshly opened.
    pub scroll: u16,
}

/// How an [`Overlay`] titles and renders itself.
pub enum OverlayKind {
    /// A bordered report box; the label follows the book title in the bar
    /// (e.g. "progress", "analysis"). Not word-wrapped — it holds aligned tables.
    Report(&'static str),
    /// The word-wrapped AI answer / summary panel.
    AiPanel,
}

impl Overlay {
    /// A bordered report overlay labelled `label`, scrolled to the top.
    pub fn report(label: &'static str, lines: Vec<Line<'static>>) -> Self {
        Self { lines, kind: OverlayKind::Report(label), scroll: 0 }
    }

    /// The AI answer / summary overlay, scrolled to the top.
    pub fn ai(lines: Vec<Line<'static>>) -> Self {
        Self { lines, kind: OverlayKind::AiPanel, scroll: 0 }
    }
}

/// Editor modes. Each variant carries the transient state that only makes sense
/// while in that mode, so illegal combinations aren't representable.
pub enum Mode {
    /// Normal prose entry. Typed characters go straight into the buffer.
    Insert,
    /// A transient prompt at the bottom: a y/N confirmation or a line of text
    /// input.
    Prompt(PromptState),
    /// Browsing the chapter list: arrows move the `selected` chapter, Enter
    /// opens it.
    Nav { selected: usize },
    /// Editing a multi-line metadata field (an epigraph quotation) in the main
    /// editor: the full editing keys apply to a scratch buffer, Ctrl-S commits,
    /// Esc cancels. Boxed because it carries a whole [`Buffer`], far larger than
    /// the other variants.
    EditMeta(Box<MetaEdit>),
    /// Reviewing story-bible files whose generated sections were hand-edited
    /// before an AI pass would overwrite them: a diff of each, with overwrite /
    /// edit / keep choices. Boxed (carries the conflict queue and a scratch
    /// buffer while editing).
    BibleReview(Box<BibleReview>),
    /// Triaging continuity findings: a navigable list where each can be jumped to
    /// (open its chapter) or dismissed. Read-only over the prose. Boxed (carries
    /// the findings list).
    Findings(Box<Findings>),
    /// The AI command palette: a navigable list of AI actions, one of which runs
    /// on Enter. The single entry point for the AI features.
    Palette(Box<Palette>),
}

/// State for an open [`Mode::Palette`]: the available AI actions and the selected
/// row.
pub struct Palette {
    pub(crate) items: Vec<PaletteItem>,
    pub(crate) selected: usize,
}

/// One palette entry: a label, a one-line description, and the action it runs.
pub struct PaletteItem {
    pub(crate) label: &'static str,
    pub(crate) desc: &'static str,
    pub(crate) action: AiAction,
}

/// The AI actions reachable from the palette.
#[derive(Clone, Copy)]
pub(crate) enum AiAction {
    Ask,
    Characters,
    Continuity,
    Setting,
    Scaffold,
    Naming,
    Style,
    Submission,
    /// Open the accumulated revision-triage findings (local; no request).
    Triage,
}

/// State for an open [`Mode::Findings`]: the findings list and the selected row.
pub struct Findings {
    pub(crate) items: Vec<Finding>,
    pub(crate) selected: usize,
}

/// One triageable finding (a continuity contradiction, style note, …). UI-shaped
/// — the AI layer maps its typed findings into this for display.
#[derive(Clone, PartialEq)]
pub struct Finding {
    /// One-line summary shown in the list (e.g. "object: the locket …").
    pub(crate) summary: String,
    /// Chapters involved; the first is the jump target.
    pub(crate) chapters: Vec<String>,
}

/// State for an open [`Mode::BibleReview`]: the queue of conflicting bible files
/// (front of the queue is the one shown), and a scratch buffer while the author
/// edits the proposed version before saving.
pub struct BibleReview {
    pub(crate) conflicts: Vec<BibleConflict>,
    /// `Some` while editing the proposed version of the front conflict.
    pub(crate) editing: Option<Buffer>,
}

/// One bible file whose on-disk generated block differs from what a fresh pass
/// would write.
pub struct BibleConflict {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) proposed: String,
    /// Pre-rendered unified diff (current → proposed), each line prefixed with
    /// `+`/`-`/` `. Computed once when the conflict is staged so the overlay
    /// isn't re-diffing every frame.
    pub(crate) diff: Vec<String>,
}

/// State for an open [`Mode::EditMeta`]. `label` is public for the editor's
/// banner; the rest is the handler's business. The scratch `buffer` is reached
/// through `App::edit_buffer`.
pub struct MetaEdit {
    /// Which field is being edited (a quotation field).
    pub(crate) target: MetaField,
    /// Scratch buffer holding the field's text.
    pub(crate) buffer: Buffer,
    /// Banner label, e.g. "chapter epigraph".
    pub label: String,
    /// Chapter to reselect when editing finishes.
    pub(crate) nav_return: Option<usize>,
}

/// State for an open [`Mode::Prompt`]. `label`/`input` are public for the status
/// bar to render; the rest is the handler's business.
pub struct PromptState {
    /// What confirming the prompt does.
    pub(crate) pending: Pending,
    /// The prompt label shown in the status bar.
    pub label: String,
    /// Text typed so far (empty for y/N confirmations).
    pub input: String,
    /// Where to return when the prompt closes: `Some(selected)` back to the
    /// chapter browser, `None` to insert mode (e.g. the quit prompt).
    pub(crate) nav_return: Option<usize>,
}

/// What a [`Mode::Prompt`] resolves to when confirmed. `at` is the chapter index
/// the action applies to.
#[derive(Clone, Copy)]
pub(crate) enum Pending {
    /// y/N: quit despite unsaved changes.
    Quit,
    /// y/N: delete the chapter at `at`.
    Delete { at: usize },
    /// Text input: title for a new chapter to insert at `at`.
    NewChapter { at: usize },
    /// Text input: new title for the chapter at `at`.
    Rename { at: usize },
    /// Text input: a question to ask the AI about the manuscript.
    AskAi,
    /// Text input: a message for a version-control commit (#131).
    Commit,
    /// y/N: run an AI action whose estimated cost exceeds the confirm threshold.
    ConfirmAi(AiAction),
    /// Text input editing a metadata field (chapter front matter or book
    /// manifest). One variant for every text-valued metadata field, so adding a
    /// field is a new [`MetaField`] case rather than new prompt control flow.
    SetMeta(MetaField),
}

/// Which metadata field a [`Pending::SetMeta`] prompt writes. Chapter fields
/// carry the chapter index; book fields are project-level.
#[derive(Clone, Copy)]
pub(crate) enum MetaField {
    Summary { at: usize },
    Pov { at: usize },
    ChapterEpigraph { at: usize },
    ChapterEpigraphBy { at: usize },
    BookTitle,
    Subtitle,
    Author,
    Series,
    BookEpigraph,
    BookEpigraphBy,
}
