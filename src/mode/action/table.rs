//! The run-action registry table (#225): one [`ActionSpec`] row per [`Action`],
//! under the menu group it lists in. The single source the registry's `ALL`,
//! `name`, `group`, `label`, `shortcut`, `assistant_runnable`, and `from_name`
//! derive from — mirroring the [`AiAction::SPECS`] descriptor table, whose AI
//! rows it copies at compile time rather than re-encoding.

use super::{Action, AiAction, ExportFormat};

/// One row of the registry: everything the run-action menu, workflows, and the
/// assistant know about an [`Action`] besides how to dispatch it (that stays a
/// `match` arm in `dispatch_action`, since it's real behavior).
pub(super) struct ActionSpec {
    pub(super) action: Action,
    /// The stable name the run-action command matches and workflows (#89)
    /// reference in config — never change these without a migration.
    pub(super) name: &'static str,
    /// Human label for the run-action menu.
    pub(super) label: &'static str,
    /// The dedicated key binding, if any (see [`Action::shortcut`]).
    pub(super) shortcut: Option<&'static str>,
    /// Whether the assistant may run it as a tool (see
    /// [`Action::assistant_runnable`]).
    pub(super) assistant_runnable: bool,
}

/// A row with no shortcut that the assistant may not run — the common case;
/// [`key`](ActionSpec::key) and [`runnable`](ActionSpec::runnable) add the rest.
const fn row(action: Action, name: &'static str, label: &'static str) -> ActionSpec {
    ActionSpec {
        action,
        name,
        label,
        shortcut: None,
        assistant_runnable: false,
    }
}

/// An AI pass's row. Its name, label, and runnability are copied at compile time
/// from the pass's `AiAction::SPECS` descriptor (#563/#1069), so they stay written
/// once; every AI pass shares Ctrl-A, which opens the AI palette.
const fn ai(action: AiAction) -> ActionSpec {
    let specs = AiAction::SPECS;
    let mut i = 0;
    while i < specs.len() {
        let spec = &specs[i];
        if spec.action as u8 == action as u8 {
            return ActionSpec {
                action: Action::Ai(action),
                name: spec.slug,
                label: spec.label,
                shortcut: Some("Ctrl-A"),
                assistant_runnable: spec.assistant_runnable,
            };
        }
        i += 1;
    }
    // Evaluated in a const, so a missing descriptor is a compile error, not a panic.
    panic!("every AiAction has a descriptor in AiAction::SPECS");
}

/// An export target's row: every format shares the Ctrl-E export menu, and the
/// assistant may run any of them.
const fn export(format: ExportFormat, name: &'static str, label: &'static str) -> ActionSpec {
    row(Action::Export(format), name, label)
        .key("Ctrl-E")
        .runnable()
}

impl ActionSpec {
    /// Give the row its dedicated key binding.
    const fn key(self, shortcut: &'static str) -> Self {
        Self {
            shortcut: Some(shortcut),
            ..self
        }
    }

    /// Let the assistant run the action as a tool.
    const fn runnable(self) -> Self {
        Self {
            assistant_runnable: true,
            ..self
        }
    }
}

/// The registry (#225): every action, under the run-action menu group (#514) it
/// lists in, in menu order. The single place an action's name, label, shortcut,
/// and assistant-runnability are written — adding an action is one row here (plus
/// its `dispatch_action` arm), and each group is contiguous by construction, so
/// the menu emits one header per group.
const GROUPS: &[(&str, &[ActionSpec])] = &[
    (
        "Project",
        &[
            row(Action::Save, "save", "Save").key("Ctrl-S").runnable(),
            row(
                Action::ProjectFind,
                "find:project",
                "Find across manuscript",
            ),
            row(Action::SwitchBook, "book:switch", "Switch book"),
            row(Action::ManageBooks, "book:manage", "Manage books (series)…"),
            row(Action::SeriesTree, "tree", "Project tree"),
            row(Action::RenameProject, "project:rename", "Rename project"),
            row(Action::BookDetails, "book:details", "Book details…"),
            row(Action::SetGoals, "goals:set", "Set writing goals"),
            row(Action::ImportNotes, "import", "Import notes → story bible"),
            row(
                Action::NewProjectFromNotes,
                "new-from-notes",
                "New project from a folder of notes",
            ),
            row(
                Action::ImportManuscript,
                "import:manuscript",
                "Import a manuscript → new project",
            ),
            row(
                Action::ImportReviewComments,
                "import:review-comments",
                "Import review comments (.docx) → annotations",
            ),
            row(
                Action::Drafts,
                "drafts",
                "Drafts (alt versions / branches)…",
            ),
            row(
                Action::ArchiveProject,
                "archive",
                "Archive project… (.zip backup)",
            ),
            row(
                Action::RestoreArchive,
                "archive:restore",
                "Restore project from archive…",
            ),
            row(Action::BrowseBackups, "backups:browse", "Browse backups…"),
        ],
    ),
    (
        "Export",
        // Ctrl-E opens the export menu; the individual format is then chosen there,
        // so every export target shares the one export-menu key.
        &[
            export(
                ExportFormat::Markdown,
                "export:markdown",
                "Export → Markdown",
            ),
            export(ExportFormat::Docx, "export:docx", "Export → Word (.docx)"),
            export(ExportFormat::Scriv, "export:scriv", "Export → Scrivener"),
            export(
                ExportFormat::Vellum,
                "export:vellum",
                "Export → Vellum (.docx)",
            ),
            export(ExportFormat::Epub, "export:epub", "Export → EPUB"),
            export(ExportFormat::Pdf, "export:pdf", "Export → PDF"),
            export(
                ExportFormat::PdfPrint,
                "export:pdf-print",
                "Export → PDF (print interior)",
            ),
            row(
                Action::PdfWrapCover,
                "export:pdf-wrap-cover",
                "Export → PDF wrap cover (front + spine + back)",
            )
            .runnable(),
            row(
                Action::ReviewCopy,
                "export:review",
                "Export review copy (numbered lines + annotations as comments)",
            )
            .runnable(),
            row(
                Action::PreflightExport,
                "export:preflight",
                "Export preflight (publish health check)",
            )
            .runnable(),
            row(
                Action::PrepareForKdp,
                "export:kdp",
                "Prepare for KDP (preflight + print PDF + EPUB)",
            )
            .runnable(),
            row(
                Action::PrepareForIngramSpark,
                "export:ingramspark",
                "Prepare for IngramSpark (preflight + print PDF + EPUB)",
            )
            .runnable(),
            row(
                Action::ExportBible,
                "export:bible",
                "Export story bible → Markdown",
            )
            .runnable(),
            row(
                Action::RevisionPlan,
                "export:revision-plan",
                "Export revision plan (chapter-ordered, ranked)",
            ),
            row(
                Action::BrowseFonts,
                "fonts:browse",
                "Browse & download fonts…",
            ),
            row(Action::SetExportStyle, "style:set", "Set export style…"),
        ],
    ),
    (
        "Reports",
        &[
            row(Action::ProgressReport, "report:progress", "Progress report")
                .key("Ctrl-R")
                .runnable(),
            row(Action::AnalysisReport, "report:analysis", "Analysis report")
                .key("Ctrl-T")
                .runnable(),
            row(Action::WritingHistory, "report:history", "Writing history").runnable(),
            row(
                Action::HealthTrend,
                "report:health-trend",
                "Export health trend (across drafts)",
            ),
        ],
    ),
    (
        // AI passes and the AI-powered helpers gather under one heading, the large
        // block the grouping is meant to tame.
        "AI",
        &[
            // Ctrl-A opens the AI palette when AI is ready, or offers to enable it
            // when it's off — both reachable from the one key.
            row(Action::EnableAi, "ai:enable", "Enable AI assistance").key("Ctrl-A"),
            ai(AiAction::Ask),
            ai(AiAction::Characters),
            ai(AiAction::Continuity),
            ai(AiAction::Setting),
            ai(AiAction::Plot),
            ai(AiAction::Theme),
            ai(AiAction::Scaffold),
            ai(AiAction::Naming),
            ai(AiAction::Style),
            ai(AiAction::Brainstorm),
            ai(AiAction::DraftCharacters),
            ai(AiAction::DraftSetting),
            ai(AiAction::Voice),
            ai(AiAction::Submission),
            ai(AiAction::Marketing),
            ai(AiAction::Social),
            ai(AiAction::AdCreative),
            ai(AiAction::Portrait),
            ai(AiAction::Scene),
            ai(AiAction::SemanticFind),
            ai(AiAction::Echo),
            ai(AiAction::Triage),
            ai(AiAction::Dashboard),
            ai(AiAction::CoOccurrence),
            ai(AiAction::AskHistory),
            ai(AiAction::Thesaurus),
            ai(AiAction::NameGen),
            ai(AiAction::Editorial),
            ai(AiAction::Accuracy),
            ai(AiAction::Relationship),
            ai(AiAction::Foreshadow),
            ai(AiAction::FactCheck),
            ai(AiAction::Pacing),
            row(
                Action::RefreshBibles,
                "bible:refresh-all",
                "Refresh all bibles",
            ),
            row(
                Action::QueuePasses,
                "bible:queue",
                "Queue bible passes (resumable)",
            ),
            row(
                Action::BackgroundBibleUpdate,
                "bible:background-update",
                "Queue a background bible update (batch, ~24h)",
            ),
            row(
                Action::RefreshSeriesBible,
                "bible:series",
                "Refresh series bible",
            ),
            row(
                Action::DescribeConfig,
                "config:describe",
                "Configure by description",
            ),
            row(
                Action::SwitchBackend,
                "ai:backend",
                "Switch AI backend (Claude / local)",
            )
            .runnable(),
            row(Action::ManageModels, "ai:models", "Local AI models"),
            row(
                Action::SetClaudeKey,
                "ai:set-key",
                "Set Claude API key (keychain)",
            ),
            row(
                Action::SetImageKey,
                "image:set-key",
                "Set image API key (keychain)",
            ),
            row(Action::Assistant, "assistant", "Authorial assistant"),
        ],
    ),
    (
        // Reference views into the story: the bible browser and research notebook
        // (local reads/edits, not AI operations) and the corkboards.
        "Bible",
        &[
            // The story-bible browser has a dedicated function key (like the guide),
            // dispatched in `on_key` rather than `on_key_ctrl` (the Ctrl-letter
            // space is full), so the Ctrl-K "Browse story bible" row shows it.
            row(Action::BrowseBible, "bible:browse", "Browse story bible").key("F2"),
            row(
                Action::ViewResearch,
                "research:view",
                "View research notebook",
            ),
            row(
                Action::Corkboard,
                "view:corkboard",
                "Corkboard (scene overview)",
            ),
            row(
                Action::ManuscriptCorkboard,
                "view:corkboard-all",
                "Corkboard — whole manuscript",
            ),
        ],
    ),
    (
        "Editing",
        &[
            row(Action::GoToLine, "goto:line", "Go to line…"),
            row(
                Action::RecentEditBack,
                "goto:recent-back",
                "Go to a recent edit (back)",
            ),
            row(
                Action::RecentEditForward,
                "goto:recent-forward",
                "Go to a recent edit (forward)",
            ),
            row(
                Action::LastInsertion,
                "goto:last-insertion",
                "Go to last insertion",
            ),
            row(Action::AcceptWord, "dict:accept", "Add word to dictionary").runnable(),
            row(
                Action::ViewDictionary,
                "dict:view",
                "View project dictionary",
            ),
            row(Action::NextMisspelling, "spell:next", "Next misspelling").runnable(),
            row(
                Action::PrevMisspelling,
                "spell:prev",
                "Previous misspelling",
            )
            .runnable(),
            row(
                Action::StyleFlagSweep,
                "style:sweep",
                "Style-flag sweep (pick a category)",
            ),
            row(
                Action::ToggleProximityRepeats,
                "style:proximity-repeats",
                "Proximity repeats — mark nearby echoes (toggle)",
            ),
            row(
                Action::SplitChapter,
                "chapter:split",
                "Split chapter at cursor",
            ),
            row(
                Action::MergeChapter,
                "chapter:merge",
                "Merge chapter into the one above",
            ),
            row(Action::AddAnnotation, "annotate", "Annotate selection"),
            row(
                Action::ReviewAnnotations,
                "annotate:review",
                "Review annotations",
            ),
            row(
                Action::RemoveAnnotation,
                "annotate:remove",
                "Remove annotation at cursor",
            ),
            row(
                Action::ClearAnnotations,
                "annotate:clear",
                "Remove all annotations here…",
            ),
            row(Action::InsertSnippet, "snippet", "Insert snippet…"),
            row(
                Action::StartSprint,
                "sprint:start",
                "Start a writing sprint",
            ),
            row(Action::StopSprint, "sprint:stop", "Stop the writing sprint"),
            row(
                Action::SetSessionTarget,
                "session:target",
                "Set a session word target",
            ),
        ],
    ),
    (
        "View",
        &[
            row(
                Action::ToggleTypewriter,
                "view:typewriter",
                "Typewriter scrolling (toggle)",
            )
            .runnable(),
            row(Action::ToggleFocus, "view:focus", "Focus mode (toggle)").runnable(),
            row(
                Action::ToggleParagraphIndent,
                "view:paragraph-indent",
                "Paragraph indent (toggle)",
            )
            .runnable(),
            row(
                Action::ToggleAnnotationMargin,
                "view:annotation-margin",
                "Annotation margin (toggle)",
            )
            .runnable(),
            row(
                Action::ToggleDailyCue,
                "view:daily-cue",
                "Daily goal cue (toggle)",
            ),
            row(Action::FoldAll, "view:fold-all", "Fold all sections"),
            row(Action::ExpandAll, "view:expand-all", "Expand all sections"),
            row(
                Action::ToggleComposition,
                "view:composition",
                "Composition mode — distraction-free (toggle)",
            )
            .key("Ctrl-\\"),
            row(
                Action::ToggleGalley,
                "view:galley",
                "Proofread — galley reading mode (toggle)",
            ),
            row(Action::Settings, "settings", "Settings…"),
        ],
    ),
    (
        "Help",
        &[
            row(Action::UserGuide, "guide", "User's guide")
                .key("F1")
                .runnable(),
            row(
                Action::QuickStart,
                "quickstart",
                "Quick-start (first-run card)",
            ),
        ],
    ),
];

/// How many actions the registry holds — sizes the flattened [`Action::ALL`].
const ACTION_COUNT: usize = {
    let mut n = 0;
    let mut g = 0;
    while g < GROUPS.len() {
        n += GROUPS[g].1.len();
        g += 1;
    }
    n
};

/// [`GROUPS`] flattened to the action sequence, at compile time.
pub(super) const ALL_ACTIONS: [Action; ACTION_COUNT] = {
    let mut out = [Action::Save; ACTION_COUNT];
    let mut n = 0;
    let mut g = 0;
    while g < GROUPS.len() {
        let rows = GROUPS[g].1;
        let mut r = 0;
        while r < rows.len() {
            out[n] = rows[r].action;
            n += 1;
            r += 1;
        }
        g += 1;
    }
    out
};

/// Every registry row with the group it sits under, in menu order.
pub(super) fn rows() -> impl Iterator<Item = (&'static str, &'static ActionSpec)> {
    GROUPS
        .iter()
        .flat_map(|&(group, rows)| rows.iter().map(move |spec| (group, spec)))
}

#[cfg(test)]
mod tests {
    use super::{rows, GROUPS};
    use crate::mode::{Action, AiAction, ExportFormat};

    /// Lists every variant of an enum *and* matches on it exhaustively, from one
    /// token list: a variant added to the enum but not here fails to compile (the
    /// match is non-exhaustive), so the generated list can't fall behind the enum.
    macro_rules! every_variant {
        ($ty:ident { $($unit:ident),* $(,)? } $(, $other:pat)?) => {{
            #[allow(dead_code, reason = "exists only so the match is exhaustive")]
            const fn exhaustive(v: $ty) {
                match v {
                    $($ty::$unit)|* => {}
                    $($other => {})?
                }
            }
            [$($ty::$unit),*]
        }};
    }

    /// Every `Action` value the registry must hold, enumerated from the enum: each
    /// unit variant, each export format, and each AI pass (`AiAction::SPECS` is
    /// itself checked complete in `ai.rs`).
    fn every_action() -> Vec<Action> {
        let units = every_variant!(
            Action {
                Save,
                ProgressReport,
                AnalysisReport,
                WritingHistory,
                EnableAi,
                UserGuide,
                QuickStart,
                ImportNotes,
                NewProjectFromNotes,
                ImportManuscript,
                ArchiveProject,
                RestoreArchive,
                BrowseBackups,
                StartSprint,
                StopSprint,
                SetSessionTarget,
                SwitchBook,
                ManageBooks,
                SeriesTree,
                RenameProject,
                BookDetails,
                SetGoals,
                RefreshSeriesBible,
                RefreshBibles,
                QueuePasses,
                BackgroundBibleUpdate,
                ProjectFind,
                Drafts,
                DescribeConfig,
                AcceptWord,
                ViewDictionary,
                GoToLine,
                RecentEditBack,
                RecentEditForward,
                LastInsertion,
                NextMisspelling,
                PrevMisspelling,
                StyleFlagSweep,
                ToggleProximityRepeats,
                ToggleTypewriter,
                ToggleFocus,
                ToggleParagraphIndent,
                ToggleAnnotationMargin,
                ToggleDailyCue,
                FoldAll,
                ExpandAll,
                ToggleComposition,
                ToggleGalley,
                Corkboard,
                ManuscriptCorkboard,
                SplitChapter,
                MergeChapter,
                AddAnnotation,
                ReviewAnnotations,
                RemoveAnnotation,
                ClearAnnotations,
                SwitchBackend,
                ManageModels,
                SetClaudeKey,
                SetImageKey,
                Assistant,
                BrowseBible,
                ViewResearch,
                InsertSnippet,
                BrowseFonts,
                SetExportStyle,
                Settings,
                RevisionPlan,
                HealthTrend,
                ExportBible,
                PreflightExport,
                PrepareForKdp,
                PrepareForIngramSpark,
                ReviewCopy,
                ImportReviewComments,
                PdfWrapCover,
            },
            Action::Export(_) | Action::Ai(_)
        );
        let formats = every_variant!(ExportFormat {
            Markdown,
            Docx,
            Scriv,
            Vellum,
            Epub,
            Pdf,
            PdfPrint,
        });
        let mut all = units.to_vec();
        all.extend(formats.into_iter().map(Action::Export));
        all.extend(AiAction::SPECS.iter().map(|s| Action::Ai(s.action)));
        all
    }

    /// The registry table is complete and unambiguous: every `Action` value has
    /// exactly one row (so `Action::entry`'s lookup always resolves), no row is
    /// extra, and no name is shared (workflows address actions by name).
    #[test]
    fn every_action_has_exactly_one_registry_row() {
        let every = every_action();
        for action in &every {
            let count = rows().filter(|(_, spec)| spec.action == *action).count();
            assert_eq!(count, 1, "{action:?} must have exactly one registry row");
        }
        assert_eq!(
            rows().count(),
            every.len(),
            "the registry has a row for something that isn't an Action variant"
        );
        assert_eq!(Action::ALL.len(), every.len());
        let mut names: Vec<&str> = rows().map(|(_, spec)| spec.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), every.len(), "registry names must be unique");
    }

    /// Each group appears once in `GROUPS`, so its actions are contiguous in
    /// `Action::ALL` and the menu emits one header per group (#514).
    #[test]
    fn registry_groups_are_unique() {
        let mut groups: Vec<&str> = GROUPS.iter().map(|&(group, _)| group).collect();
        groups.sort_unstable();
        groups.dedup();
        assert_eq!(groups.len(), GROUPS.len(), "a group is listed twice");
    }
}
