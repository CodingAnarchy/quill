Quill — User's Guide

Quill is a terminal editor for writing novels. It keeps you in the prose: a
centered, fixed-width page, a live word count, and a small set of keystrokes.
Press F1 any time to reopen this guide; scroll with the arrow keys, and press
any other key to close it.


WRITING

Just type. Text flows into a centered column sized like a printed page; the
status bar shows the live word count and your position. Editing is Unicode-
correct — movement and deletion work on whole characters. Ctrl-Z undoes a burst
of typing and Ctrl-Y redoes it. Ctrl-+ and Ctrl-- widen and narrow the column.

Markdown is understood as you write: emphasis (*italic*, **bold**) renders
softly, with the markers hidden until you edit the span, and Ctrl-O toggles an
in-chapter outline pane.

Fence a block in ``` to set it apart verbatim — a log readout, a transmission,
a letter, anything whose line breaks and spacing should be preserved exactly. It
renders monospaced and unstyled, and carries through to every export. (Smart
typography still applies inside: these blocks hold prose, not source code.)


CHAPTERS & NAVIGATION

A project is a folder of chapters (one Markdown file each) plus a book.toml
manifest. Ctrl-N and Ctrl-P step to the next and previous chapter. Ctrl-G opens
the chapter browser: arrow keys move, Enter opens the selected chapter, "o"
opens it in a side pane, and "f" lists the project's reference files (the story
bible and loose notes). Press "/" to filter the list as you type (by title,
status, or POV) and quick-jump to a chapter; Enter opens the highlighted match,
Esc keeps the filter for the browser's commands to act on, and a second Esc
clears it. From the browser you can also create, rename, reorder, and delete
chapters, and set each chapter's status, summary, and section.

Ctrl-L toggles the continuous-manuscript view, which flows across chapter
boundaries and writes edits back to the right files, while never holding more
than a few chapters in memory. Faint part and front/back-matter section headers
mark each structural transition as you flow through it (render-only, like the
chapter dividers), so you can tell which part you're in.

Ctrl-W begins a window command for split panes: split the editor, open another
chapter or a bible entry or a docked report beside the prose, switch focus,
resize, and close. (See docs/windows.md for the full set.)


PROJECTS

Ctrl-B opens the project browser so you can move between books without quitting.
It lists your recently-opened projects, newest first; Enter switches to one in
place (saving the current project first, and confirming if there are unsaved
changes). Press "b" to browse the filesystem for a project that isn't listed
yet, or "n" to create a brand-new one — type its path and Quill makes the
folder, manifest, and first chapter, then opens it.

Point Quill at the folder where you keep your books, in
~/.config/quill/config.toml, for a complete library view:

    [projects]
    home = "~/Writing"

Every project under that folder then appears in the browser, ready to open.

STARTING UP WITHOUT A PATH

A bare `quill` launch (no path) follows where you are. Inside an existing project,
it opens that one straight away. From a folder that isn't a project — when you
have books to return to — it shows a chooser: "Create a new project" or "Open an
existing project". Choosing to open lists your recent projects together with every
project under your projects home (the same library the Ctrl-B browser shows);
arrow keys move, Enter opens the highlighted one, Esc steps back to the chooser.
Choosing to create asks for a name and files the new project in your projects home
rather than in the current directory.

When there's nothing to open yet — no recent projects and no projects home set —
the chooser would be empty, so Quill skips it and goes straight to naming a new
book. With no projects home configured, that new book lands in a `Quill` folder in
your home directory, and Quill says so (set `[projects] home` to choose where
books live). Passing an explicit path — `quill path/to/book` — always opens or
creates that project directly, with no chooser.

To make the project's folder match its title, run "Rename project folder" from
the Ctrl-K menu: it's prefilled with the book title, the folder moves without
quitting, and the project keeps saving to the new location. Renaming the folder
is separate from the title (set with "t" in the chapter browser).

A project can hold a series of books. When it does, Quill opens the first book;
run "Switch book" from the run-action menu (Ctrl-K) to move to another — the
editor, chapter list, and reports follow the book you pick, and the chapter
you're leaving is saved first. A standalone novel is just a one-book project, so
the command says there's only one book.

"Project tree" (also in the Ctrl-K menu) opens a navigator for the whole
project: the series, its books, their parts, and their chapters, as an indented
tree with a word count at every level. Arrow keys move; → expands a book and ←
collapses it; Enter opens the selected chapter (switching books if it's in
another) or expands a book. A standalone novel shows a one-book tree. The
progress report (Ctrl-R) leads with the series total when a project holds more
than one book.

When you create a project, a short setup wizard offers to scaffold it: writing
goals, a starter cast and setting (drafted by the AI when it's enabled), and a
first note. Every step is skippable and the whole wizard is dismissible with
Esc — turn it off with "setup_wizard = false" under [projects].


SAVING & SAFETY

Ctrl-S saves the active chapter (or the continuous window). Saves are atomic: an
interrupted save never corrupts a file, and the previous version always
survives. If Quill exits unexpectedly, the next launch offers back any unsaved
work. Ctrl-Q quits, prompting first if there are unsaved changes.

The status bar shows a save-state marker beside the chapter name: ✓ when the
buffer is on disk, ● when you have unsaved changes. Set [editor]
autosave_interval_secs = N in your config to save automatically after N seconds
of idle (0, the default, is off); autosave uses the same atomic path as Ctrl-S
but doesn't run on_save workflows.


REPORTS

Ctrl-R opens the progress report: completion against your goals, a projected
final length and finish date from your writing pace, and today's word delta
(set goals and a start date in book.toml). Ctrl-T opens the analysis report:
per-chapter readability, pacing, sentence lengths, and overused words and
phrases. Both are scrollable; any other key closes them.


EXPORT

Ctrl-E opens the export menu: Markdown (one compiled document), Word (.docx in
standard manuscript format), Scrivener (a .scriv bundle), or Vellum (a .docx
tuned for Vellum import). Files land under export/. For a multi-book series, each
format is offered twice — the whole series (books in reading order, delimited per
format) or just the active book.


AI ASSISTANCE (opt-in)

With AI enabled in your config, Ctrl-A opens the AI command palette. The
assistant is a helper, never a co-writer: it tracks, outlines, marks, and
observes, but it never writes or rewrites your manuscript prose. It can build a
story bible (characters, timeline, setting), flag continuity and naming issues,
observe style, brainstorm ideas, draft a starter cast or setting from a concept,
and produce off-manuscript material (a synopsis, query letter, blurbs, social
posts). Revision triage collects every finding into one list to work through.

Each pass runs off the editor thread, so the editor never freezes, and a pass
that would cost more than your threshold asks before spending.

Your API key is kept in the OS keychain, not in plaintext in your config file, so
a synced dotfile or a backup of the config never carries it. Run "Set Claude API
key" (or "Set image API key") from the Ctrl-K menu, type the key once, and Quill
stores it in the system keychain. If you already had a plaintext "api_key" in the
config, it offers to remove it. The key is resolved as: the environment variable
(ANTHROPIC_API_KEY / FAL_KEY, for headless or CI use) first, then the keychain,
then any leftover plaintext "api_key". This guards against the key leaking by
accident — a synced dotfile, a backup, a casual look at the file; it does not stop
anyone who can already run Quill as you (the system unlocks the keychain for your
session). On a headless Linux box with no keychain daemon, set the env var.

Stuck on a word? "Word search" is an in-context thesaurus: it prefills the word
under your cursor — add constraints in plain words ("2 syllables, Anglo-Saxon,
common; 8 results") if you like — and returns candidate words or short phrases
with definitions, using the current line only to judge fit. It hands you options
to choose from; it never rewrites your sentence.

Want an editor's read? "Editorial read" flags issues across three lenses —
structural (pacing, stakes), continuity (contradictions between chapters), and
line (grammar, usage, typos) — into the revision-triage list, each tagged by
lens. It points out what to look at; it never suggests the fix or rewrites your
prose — you decide what to change.

Already have notes? "Import notes" (in the Ctrl-K menu) takes a Markdown file —
or a folder of them — preserves the original under bible/sources/, and folds
whatever it finds (characters, setting and world systems, the timeline, an
outline) into the matching bible documents in Quill's format. To start from
scratch, "New project from a folder of notes" creates a project and imports the
folder in one step.

Writing a series? "Refresh series bible" (in the Ctrl-K menu, for multi-book
projects) brings the characters, timeline, and setting up to date across every
book — one book at a time, in reading order, so the bible spans the whole series.
Only changed books re-run, progress shows per book, and Esc cancels. Because each
book is checked against the running bible, the timeline and setting findings catch
cross-book contradictions too; they land in Revision triage tagged with their
book, and jumping to one switches books for you.


ACTIONS & WORKFLOWS

Ctrl-K opens the run-action menu: every invokable action — save, each export,
the reports, the AI passes, and this guide — grouped under collapsible headings
(Project, Export, Reports, AI, Editing, View, Help), each run through the same
path its shortcut uses. Navigate with ↑↓; on a heading, → expands and ← collapses
(Enter toggles it), so you can fold away the groups you're not using — handy for
the large block of AI passes. Enter runs the selected action. An action that
can't run right now stays listed with the reason.

Ctrl-X runs a workflow: a named sequence of actions you define in config.toml,
so a single command can, say, save, refresh the character bible, flag
continuity, and export. Workflows can also fire on save or from a Vim key.

Snippets are fixed text you insert often — a scene break, a tricky name —
defined once in a [snippets] table in config.toml (name = "literal text"). Run
"Insert snippet" from the Ctrl-K menu to pick one by name; its text drops in at
the cursor verbatim, as a single undo.


KEY REFERENCE

  F1              this guide
  Ctrl-S          save
  Ctrl-Q          quit (prompts if unsaved)
  Ctrl-Z / Ctrl-Y undo / redo
  Ctrl-F          find (then "r" to replace)
  Ctrl-O          toggle the chapter / outline pane
  Ctrl-L          toggle the continuous manuscript
  Ctrl-N / Ctrl-P next / previous chapter
  Ctrl-G          browse chapters
  Ctrl-B          browse / switch projects
  Ctrl-W          window panes (split, focus, resize, close)
  Ctrl-+ / Ctrl-- widen / narrow the text column
  Ctrl-R          progress report
  Ctrl-T          analysis report
  Ctrl-E          export menu
  Ctrl-A          AI command palette
  Ctrl-K          run action
  Ctrl-X          run workflow

Smart typography is on by default: as you type, straight quotes become curly,
-- becomes an em-dash, and ... becomes an ellipsis. For plain ASCII punctuation,
set [editor] smart_typography = false in your config.

Vim-style modal editing is available, off by default; enable it with
[editor] vim = true in your config.
