//! Prompt resolutions and metadata-field selectors: what a
//! [`Mode::Prompt`](super::Mode) confirms to ([`Pending`]), the new-project setup
//! wizard's steps ([`WizardStep`]), and the metadata fields the setter prompts
//! write ([`MetaField`] / [`BookField`]). Split out of `mode.rs` to keep the core
//! enums under the ~1,000-line convention; re-exported from there, so callers
//! (`crate::mode::{Pending, …}`) are unaffected. Pure move.

use crate::ai::keychain;
use crate::ai::namegen;
use crate::project::GoalField;

use super::{AiAction, SettingTextKnob, StyleTextKnob};

/// A step in the new-project setup wizard (#282), offered in turn after a
/// project is created. Each is individually skippable; the AI steps are skipped
/// outright when AI is off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WizardStep {
    /// Writing goals → the manifest (drives the progress report).
    Goals,
    /// Draft a starter cast from a concept (AI) → the character bible.
    Characters,
    /// Draft the setting from a concept (AI) → the setting bible.
    Setting,
    /// A free-form note → `notes.md`.
    Notes,
}

impl WizardStep {
    /// The step after this one, or `None` when the wizard is finished.
    pub(crate) const fn next(self) -> Option<Self> {
        match self {
            Self::Goals => Some(Self::Characters),
            Self::Characters => Some(Self::Setting),
            Self::Setting => Some(Self::Notes),
            Self::Notes => None,
        }
    }

    /// Whether this step needs the AI backend (so it's skipped when AI is off).
    pub(crate) const fn needs_ai(self) -> bool {
        matches!(self, Self::Characters | Self::Setting)
    }
}

/// What a [`Mode::Prompt`](super::Mode) resolves to when confirmed. `at` is the
/// chapter index the action applies to.
#[derive(Clone, Copy)]
pub enum Pending {
    /// y/N: quit despite unsaved changes.
    Quit,
    /// y/N: delete the chapter at `at`.
    Delete { at: usize },
    /// y/N: merge the chapter at `at` into the one above it (#585).
    Merge { at: usize },
    /// Text input: title for the new tail chapter when splitting the active
    /// chapter at the cursor (#585); the chapter being split is at `at`.
    SplitChapter { at: usize },
    /// Text input: title for the copy when duplicating the chapter at `at` (#585).
    Duplicate { at: usize },
    /// Text input: a chapter to jump to by number (1-based) or by a title
    /// substring (#588) — a direct go-to that skips scrolling the browser. A
    /// number selects that chapter; otherwise the first chapter whose title
    /// matches (case-insensitive) is opened.
    GoToChapter,
    /// Numeric input: a 1-based line number to move the cursor to in the active
    /// chapter (#1010) — a direct go-to-line outside Vim's `:N`. Clamped to the
    /// buffer's last content line, landing on its first non-blank column like Vim's
    /// `{count}G`; a blank or non-numeric entry is a no-op.
    GoToLine,
    /// Text input: title for a new chapter to insert at `at`.
    NewChapter { at: usize },
    /// Text input: new title for the chapter at `at`.
    Rename { at: usize },
    /// Text input: a question to ask the AI about the manuscript.
    AskAi,
    /// Text input: a query for semantic "find similar scenes" (#924) — the passages
    /// most similar to this text, ranked with scores + jumpable citations. Prefilled
    /// with the visual selection when one is active, so the author can find scenes
    /// like the one they highlighted.
    SemanticFind,
    /// Text input: a word/phrase (and optional filters) for the thesaurus (#290).
    Thesaurus,
    /// Text input: constraints (origin, syllables, tone, era) for the name
    /// generator (#610). The `Kind` (character / place / term) was chosen from the
    /// picker before the prompt opened.
    NameGen(namegen::Kind),
    /// Text input: a message for a version-control commit (#131).
    Commit,
    /// Text input: the name for a manuscript milestone — an annotated tag over the
    /// whole project (#833). On Enter, Quill asks for an optional note (#966)
    /// before creating the milestone on the current HEAD.
    Milestone,
    /// Text input: an optional free-text note for the milestone being created
    /// (#966), asked right after its name. The name waits in
    /// `App::pending_milestone_name` (kept off the `Copy` `Pending` enum); on
    /// Enter the milestone is created carrying this note as its annotation. A
    /// blank note is fine — the milestone is created without one.
    MilestoneNote,
    /// Text input: a new note for an existing milestone (#966), edited from the
    /// history view and prefilled with the current note. The milestone's name
    /// waits in `App::pending_milestone_name`; on Enter the tag is re-annotated
    /// in place, leaving the revision it marks unchanged.
    EditMilestoneNote,
    /// Text input: the name for a new draft — a branch off the current state (#965).
    /// On Enter the draft is created and switched to (the working copy is already the
    /// current state, so switching is a no-op checkout). A blank name cancels; an
    /// existing name is refused. Drafts track alt endings / POV splits as first-class
    /// versions.
    CreateDraft,
    /// Confirm running an AI action whose estimated cost exceeds the threshold.
    /// `force` is whether to force a full bible rebuild (#176); `offer_local` adds
    /// "run locally instead" as a third choice when the hybrid router (#372) found
    /// a usable local backend (`y` Claude · `l` local · anything else cancels).
    ConfirmAi {
        action: AiAction,
        force: bool,
        offer_local: bool,
    },
    /// Confirm the all-bibles refresh (#655) — one combined cost confirm gating
    /// the four bible passes over every book. `force` rebuilds each bible from the
    /// whole manuscript (#176). `y` runs the batch, anything else cancels.
    ConfirmRefreshBibles { force: bool },
    /// Confirm a durable pass queue (#923) — one aggregate cost confirm gating the
    /// whole bible-suite run. `force` rebuilds each bible from the whole manuscript
    /// (#176). `y` runs the queue, anything else cancels.
    ConfirmQueuePasses { force: bool },
    /// y/Esc: resume an interrupted pass queue found at launch (#923) — a crashed
    /// run picks up its remaining passes rather than losing the completed ones. `y`
    /// resumes; anything else discards the persisted queue.
    ResumePassQueue,
    /// Confirm a background bible update (#950) — one aggregate, batch-discounted
    /// cost confirm before submitting the full-manuscript rebuilds to the async
    /// Message Batches API. `y` submits (and returns immediately); anything else
    /// cancels.
    ConfirmBatchUpdate,
    /// y/N: generate character portraits (#238) — each image is billed, so the
    /// action confirms before spending.
    ConfirmPortraits,
    /// y/N: generate a scene illustration of the active chapter (#239).
    ConfirmScene,
    /// y/N: restore the current chapter to its last committed version (#135).
    ConfirmRestore,
    /// y/N: restore the *whole project* to a revision (#833) — a milestone or a
    /// past commit chosen in the history view, or HEAD from the palette. Destructive
    /// to uncommitted changes across the manuscript, so it always asks first. The
    /// target revision waits in `App::pending_restore_rev` (kept off the `Copy`
    /// `Pending` enum), taken when the restore runs.
    ConfirmRestoreProject,
    /// y/N: restore a local backup archive over the open project (#934) — chosen in
    /// the backup browser, or the latest offered when the project looks damaged on
    /// open. Destructive to the current working state, so it always asks first; the
    /// working project is itself archived before the swap. The target archive waits
    /// in `App::pending_restore_backup` (kept off the `Copy` `Pending` enum), taken
    /// when the restore runs.
    ConfirmRestoreBackup,
    /// Crash recovery (#263): unsaved work from a previous session was found.
    /// `y`/`r` restores it, `d` discards the sidecar, Esc keeps it for later.
    ConfirmRecovery,
    /// y/N: there are unsaved changes — save the current project and switch to
    /// the one chosen in the project browser (#270). The target waits in
    /// `App::pending_switch`.
    ConfirmSwitch,
    /// Text input: the path (location + name) for a new project to create from
    /// the project browser (#273). Created and opened on Enter.
    NewProject,
    /// Text input: the new folder name for the open project (#424). On Enter the
    /// project directory is renamed and reopened at the new path.
    RenameProject,
    /// Text input: a new folder name for a project chosen in the browser (#612).
    /// The target project's path waits in `App::pending_manage`; on Enter its
    /// directory is renamed (moving the folder, updating recents) without opening
    /// it. Distinct from [`Pending::RenameProject`], which renames the open one.
    RenameBrowserProject,
    /// y/N: delete the project chosen in the browser (#612), removing its folder
    /// from disk. The target's path waits in `App::pending_manage`; `y` removes
    /// it and drops it from the list, anything else cancels. Guarded — the
    /// currently-open project is refused before the prompt ever opens.
    ConfirmDeleteProject,
    /// Text input: the series title when converting a standalone novel into a
    /// multi-book series (#648). The current book becomes book one; a blank entry
    /// uses the book's own title as the series name. On Enter the `[[books]]`
    /// manifest is written and the add-a-book prompt chains after it.
    ConvertToSeries,
    /// Text input: the title for a new book appended to the series (#648). On Enter
    /// the book is created with a starter chapter and made active. Blank re-prompts.
    AddBook,
    /// y/N: remove book `index` from the series (#648), retiring its chapters to
    /// `unused/`. `y` removes it (and re-activates a neighbour if it was active);
    /// anything else cancels. Guarded — a series always keeps at least one book.
    ConfirmRemoveBook { index: usize },
    /// y/N: offer the new-project setup wizard (#282). `y` starts it at the first
    /// step; anything else lands in the editor.
    WizardStart,
    /// Text input for one setup-wizard step (#282): blank skips the step, Esc
    /// dismisses the rest of the wizard.
    Wizard(WizardStep),
    /// Text input: a path to a Markdown notes file (or a folder of them) to
    /// import into the story bible (#283).
    ImportNotes,
    /// Text input: the folder of notes to start a new project from (#286).
    NewFromNotesSource,
    /// Text input: the path for the new project created from notes (#286).
    NewFromNotesDest,
    /// Text input: a Markdown manuscript file to import as a new project (#298).
    ImportManuscriptSource,
    /// Text input: the path for the new project created from a manuscript (#298).
    ImportManuscriptDest,
    /// Text input: a marked-up `.docx` to import review comments from (#886). On
    /// Enter the Word comments are recovered, resolved against the active chapter,
    /// and the review-import preview opens.
    ImportReviewComments,
    /// Text input: the destination path for a project archive (#594). On Enter a
    /// timestamped `.zip` of the whole project is written there.
    ArchiveProjectDest,
    /// Text input: the `.zip` archive to restore a project from (#594). On Enter
    /// the archive is validated and the new-project destination is asked for.
    RestoreArchiveSource,
    /// Text input: the folder to unpack a restored archive into (#594). On Enter
    /// the archive is unpacked there and the new project is opened.
    RestoreArchiveDest,
    /// Text input: how many minutes a writing sprint should run (#299).
    SprintDuration,
    /// Text input: the per-session word target (#1011) — how many words to write
    /// this session. A blank entry clears any running target; zero is rejected.
    SessionTarget,
    /// Text input: the remote URL to back the project up to (#134).
    SetRemote,
    /// Text input: a Vim Ex command (`:w`, `:q`, `:wq`, …) typed after `:` (#207).
    ExCommand,
    /// Text input: a premise / idea to brainstorm story ideas from (#255).
    Brainstorm,
    /// Text input: a concept to draft a starter character cast from (#256).
    DraftCharacters,
    /// Text input: a concept to draft a starter setting bible from (#257).
    DraftSetting,
    /// Text input: the concern (period / place / domain, e.g. "1850s London") to run
    /// a cultural & historical accuracy check against (#973). Author-invoked;
    /// accuracy, not sensitivity / representation.
    AccuracyConcern,
    /// Text input: the character pair (e.g. "Alice / Bob") whose relationship
    /// trajectory to track across the book (#1046). Author-invoked; findings only.
    RelationshipPair,
    /// Text input: a plain-language description of a workflow to generate (#232).
    DescribeWorkflow,
    /// Text input: confirm or edit the name for the workflow the AI proposed
    /// (#232); the steps wait in `App::pending_workflow`. Enter saves, Esc cancels.
    NameNewWorkflow,
    /// Text input: a plain-language description of a configuration change (#303).
    DescribeConfig,
    /// y/N: apply the configuration changes the AI proposed (#303); they wait in
    /// `App::pending_config`. `y` writes them, anything else cancels.
    ConfirmConfig,
    /// Text input: the note for an inline annotation (#302) anchored to the byte
    /// range `[start, end)` of the active chapter. Enter saves it, Esc cancels.
    Annotate { start: usize, end: usize },
    /// Text input: a reply to add to the annotation at `index` in `App::annotations`
    /// (#889). Enter appends it to the thread and re-opens the review list; an empty
    /// reply (or Esc) cancels, re-opening the list unchanged.
    AnnotateReply { index: usize },
    /// Text input: the one-line synopsis for a scene on the corkboard (#636 / #701).
    /// The target is resolved when the prompt opens — `chapter` is the card's
    /// chapter on the manuscript-wide board (`None` on the per-chapter board), and
    /// `scene` is its within-chapter scene index — so the write lands on the right
    /// file even after the mode has left `Corkboard` for the prompt. `manuscript`
    /// records which board to re-open. Enter persists it to the chapter's `scenes:`
    /// front matter (a blank entry clears it) and re-opens the board; Esc returns
    /// without a change.
    SetSceneSynopsis {
        chapter: Option<usize>,
        scene: usize,
        manuscript: bool,
    },
    /// y/N: remove the downloaded local model with this id from disk (#382). The
    /// id is a catalog `&'static str`, so [`Pending`] stays `Copy`.
    ConfirmRemoveModel(&'static str),
    /// Text input editing a metadata field (chapter front matter or book
    /// manifest). One variant for every text-valued metadata field, so adding a
    /// field is a new [`MetaField`] case rather than new prompt control flow.
    SetMeta(MetaField),
    /// Numeric input setting one project goal (#550): target words, target
    /// chapters, the daily target, or words-per-page. Persisted to the `[goals]`
    /// table of `book.toml`. The goal prompts chain (`goals:set` walks the
    /// required fields then offers the optional daily target); a blank entry
    /// clears the optional daily goal and is rejected for the required ones.
    SetGoal(GoalField),
    /// Text input setting one book-details field (#571): title, subtitle, author,
    /// or series. Persisted to the manifest through the shared metadata setter, so
    /// it reads back identical to the chapter-browser single-letter keys. The
    /// prompts chain (`book:details` walks title → subtitle → author → series);
    /// Esc stops the chain. The required title keeps its current value on blank;
    /// the optional fields clear on blank.
    SetBookField(BookField),
    /// Text input setting one free-text `[style]` knob (#569): the scene-break
    /// glyph or a title/part ornament. Persisted to the `[style]` table of
    /// `book.toml`; a blank entry clears the key. On save the setter re-opens the
    /// style menu so more knobs can be tweaked in one sitting.
    SetStyleText(StyleTextKnob),
    /// Text input setting one numeric / list Settings knob (#592): the column
    /// width, the autosave interval (both `[editor]`, validated + spliced via the
    /// shared config machinery), or the analysis stopwords (`[analysis]
    /// stopwords_add` in `book.toml`). On save the Settings menu re-opens so more
    /// knobs can be tweaked in one sitting; the `[editor]` knobs apply live.
    SetSettingText(SettingTextKnob),
    /// Text input: an API key to store in the OS keychain (#530), for the given
    /// provider. On Enter the key is written to the keychain (never the config
    /// file); a blank entry cancels. The provider determines the keychain entry
    /// and which plaintext field, if any, is then offered for removal.
    SetApiKey(keychain::Key),
    /// y/N: a plaintext `api_key` is now redundant (the key is in the keychain) —
    /// strip it from the config file (#530). `y` removes it; anything else leaves
    /// it (where it stays the lowest-precedence fallback).
    StripPlaintextKey(keychain::Key),
    /// y/N: AI is off — enable it in-app (#602). `y` writes `[ai] enabled = true`
    /// via the config splicer, applies it live, and chains into the Set-key flow
    /// (Claude with no key needs one); anything else leaves AI off. `resume` is the
    /// AI action the author was reaching for, re-run once AI is enabled and keyed,
    /// or `None` for the plain "Enable AI assistance" action.
    ConfirmEnableAi { resume: Option<AiAction> },
}

/// Which metadata field a [`Pending::SetMeta`] prompt writes. Chapter fields
/// carry the chapter index; book fields are project-level.
#[derive(Clone, Copy)]
pub enum MetaField {
    Summary {
        at: usize,
    },
    Pov {
        at: usize,
    },
    /// The chapter's word-count goal (#146).
    Goal {
        at: usize,
    },
    /// The file's section role (#179): front/back-matter type, or blank for body.
    Role {
        at: usize,
    },
    ChapterEpigraph {
        at: usize,
    },
    ChapterEpigraphBy {
        at: usize,
    },
    BookTitle,
    Subtitle,
    Author,
    Series,
    BookEpigraph,
    BookEpigraphBy,
    /// Publishing metadata (#583): ISBN, publisher, publication date, language tag,
    /// and the rights/copyright line — all optional, clearing on blank.
    Isbn,
    Publisher,
    PubDate,
    Language,
    Rights,
}

impl MetaField {
    /// Whether this field is an epigraph — the quotation or its attribution, at
    /// chapter or book level. These are display prose that ships in the export, so
    /// they get the same smart-typography pass as the body (#519); other metadata
    /// (titles, POV, summaries, goals) is stored verbatim.
    pub(crate) const fn is_epigraph(self) -> bool {
        matches!(
            self,
            Self::ChapterEpigraph { .. }
                | Self::ChapterEpigraphBy { .. }
                | Self::BookEpigraph
                | Self::BookEpigraphBy
        )
    }
}

/// One book-level field the "Book details…" run-action (#571) walks in turn —
/// title, subtitle, author, series. Each maps to the [`MetaField`] the manifest
/// stores it under, so the chained-prompt flow and the chapter-browser
/// single-letter keys persist (and read back) through the one shared setter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BookField {
    Title,
    Subtitle,
    Author,
    Series,
    /// Publishing metadata (#583), appended to the chained flow after the title
    /// fields: ISBN, publisher, publication date, language, and the rights line.
    Isbn,
    Publisher,
    PubDate,
    Language,
    Rights,
}

impl BookField {
    /// The [`MetaField`] this book field is stored as — the single mapping that
    /// keeps the run-action flow and the single-letter keys writing to the same
    /// manifest slot.
    pub(crate) const fn meta(self) -> MetaField {
        match self {
            Self::Title => MetaField::BookTitle,
            Self::Subtitle => MetaField::Subtitle,
            Self::Author => MetaField::Author,
            Self::Series => MetaField::Series,
            Self::Isbn => MetaField::Isbn,
            Self::Publisher => MetaField::Publisher,
            Self::PubDate => MetaField::PubDate,
            Self::Language => MetaField::Language,
            Self::Rights => MetaField::Rights,
        }
    }

    /// The prompt label for this field (matching the chapter-browser key labels).
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Title => "Book title",
            Self::Subtitle => "Subtitle",
            Self::Author => "Author",
            Self::Series => "Series",
            Self::Isbn => "ISBN",
            Self::Publisher => "Publisher",
            Self::PubDate => "Publication date (YYYY or YYYY-MM-DD)",
            Self::Language => "Language (BCP-47 tag, e.g. en, fr)",
            Self::Rights => "Rights / copyright line",
        }
    }
}
